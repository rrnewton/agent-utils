//! Order and chunk queued changes into batches.
//!
//! The plan is bound to one exact base commit and one exact head per change. It is built from
//! three kinds of evidence, all probed locally: conflicts with the base (decided the way the
//! queue combines — a replay under `combine=rebase`, a plain `git merge-tree` under
//! `combine=merge`; exact changes are not merge-probed at all, because landing tests them
//! with `lands_exactly`, never a merge), pairwise conflicts between queued heads (merge-tree),
//! and ancestry (a change stacked on another must land after it).
//!
//! Without repair, two changes that textually conflict cannot both land this round: once one
//! lands, the other conflicts with the base and needs a rebase. The plan therefore picks a
//! conflict-free subset. Priority changes (the priority label) and the changes they depend on
//! come first; then the numeric priority decides; among equal priorities the change with fewer
//! conflicts wins (a greedy maximum-independent-set heuristic, which lands more changes per round
//! than first-come-first-served when one change conflicts with many), then submission time.
//!
//! A priority change is validated in a batch of its own: it reaches the base first, and a red
//! result touches no other change.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use pr_landing_planner::graph::connected_components;
use pr_landing_planner::model::ConflictEdge;

use crate::git::{CombineMode, Combined, Git, Regenerable};
use crate::model::{
    now, Conflict, Held, Item, ItemStatus, Plan, PlannedBatch, PlannedItem, PLAN_SCHEMA,
};
use crate::probe_cache::{probe_config_hash, BaseEntry, PairAncestry, PairEntry, ProbeCache};
use crate::state::{PriorityConfig, DEFAULT_PROBE_PARALLELISM};

/// How one probing run used its cache and its concurrency bound.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeStats {
    /// Lookups answered wholly from the probe cache (no git subprocess ran for them).
    pub cache_hits: usize,
    /// Lookups that ran at least one git subprocess.
    pub cache_misses: usize,
    /// Base-probe cache hits (one per live head at most).
    pub base_hits: usize,
    /// Base-probe cache misses: base probes that ran git subprocesses.
    pub base_misses: usize,
    /// Pairwise-probe cache hits (ancestry or conflicts known without git).
    pub pair_hits: usize,
    /// Pairwise-probe cache misses: pairs that ran git subprocesses.
    pub pair_misses: usize,
    /// Pairs settled without git and without the cache: stacked (one head's range contains the
    /// other) or sharing no file outside the regenerable paths, so no conflict is possible.
    pub pairs_skipped: usize,
    /// Pairs left unknown by a first-stage probe (see [`probe_first_stage`]):
    /// treated as clean with no ancestry so a first batch can be formed at
    /// once. A full probe always leaves this at zero.
    pub pairs_unknown: usize,
    /// Highest number of probe jobs observed running at once; never above the bound.
    pub peak_concurrency: usize,
}

/// One change's peripheral classification (see `state::PeripheralConfig`): whether the
/// consumer hook calls it peripheral, and whether that answer came from the cache.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PeripheralMark {
    /// The hook answered `peripheral`.
    pub peripheral: bool,
    /// The answer came from the probe cache, not a fresh hook call.
    pub cached: bool,
}

/// The peripheral classifier hook probing passes to [`probe_with_options`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeripheralHook {
    /// Shell command (see `state::PeripheralConfig::command`).
    pub command: String,
    /// Seconds one call may run before the change counts as binary-touching.
    pub timeout_secs: u64,
}

/// Locally probed relationships between the base and the queued heads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Probe {
    /// Current head per change; `None` when the head ref no longer resolves.
    pub heads: BTreeMap<i64, Option<String>>,
    /// Files each change touches relative to the base.
    pub files: BTreeMap<i64, BTreeSet<String>>,
    /// Paths each change conflicts on with the base (absent or empty: clean).
    pub base_conflicts: BTreeMap<i64, Vec<String>>,
    /// Pairwise conflicts between heads.
    pub conflicts: Vec<Conflict>,
    /// `(before, after)`: `before`'s head is an ancestor of `after`'s head.
    pub ancestry: Vec<(i64, i64)>,
    /// Holds decided outside this repository (unmet peer requirements).
    pub external: BTreeMap<i64, Held>,
    /// Commits each live head adds over the base (`rev-list BASE..HEAD`), so a change that
    /// carries an ejected or withdrawn change's commits can be held.
    pub ranges: BTreeMap<i64, BTreeSet<String>>,
    /// Cache and concurrency accounting for the run that produced this probe. Planning
    /// ignores it; it exists so callers can report what probing actually cost.
    pub stats: ProbeStats,
    /// Peripheral classification per change (absent: binary-touching, the fail-closed
    /// default, including when no hook is configured).
    pub peripheral: BTreeMap<i64, PeripheralMark>,
    /// Pairs the footprint prior marks as suspect (shared affected tests), whether or
    /// not the merge-tree probe found a textual conflict. A prior, never a verdict.
    pub prior: Vec<(i64, i64)>,
}

/// Knobs for [`probe_with_options`].
#[derive(Clone, Debug)]
pub struct ProbeOptions {
    /// State directory holding the persistent probe cache (`probe-cache.json` in it);
    /// `None` keeps the cache in memory for this call only.
    pub cache_dir: Option<PathBuf>,
    /// Maximum probe jobs (git subprocesses) running at once; at least 1.
    pub parallelism: usize,
    /// Hash of the merge configuration, from `probe_cache::probe_config_hash`. `None`
    /// derives it from the repository's Git settings alone.
    pub config_hash: Option<String>,
    /// Peripheral classifier hook; `None` classifies nothing (all binary-touching).
    pub peripheral: Option<PeripheralHook>,
    /// Path in the repository of the test-footprints JSON (`{test id: [paths]}`),
    /// read at the base; `None` disables the conflict prior.
    pub footprints: Option<String>,
    /// How the queue combines changes onto the base: the base-conflict verdict must
    /// predict that combine, so under `Rebase` it replays (a commit whose patch is
    /// already on the base drops out, as the combine step drops it) and under
    /// `Merge` it is the plain merge-tree verdict.
    pub combine: CombineMode,
    /// Changes submitted exact (`submit --exact`): never merge-probed against the
    /// base. Landing tests them with `lands_exactly` (first parent is the base, or
    /// a linear fast-forward) when their candidate is built, which ejects a moved
    /// base as `not-fast-forward`; a merge-tree verdict here would instead hold
    /// them `conflicts-with-base` for a merge that will never happen.
    pub exact: BTreeSet<i64>,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            parallelism: DEFAULT_PROBE_PARALLELISM,
            config_hash: None,
            peripheral: None,
            footprints: None,
            // The historical probe verdict; queue callers pass the configured mode.
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        }
    }
}

impl ProbeOptions {
    /// One job at a time and no persistent cache: the pre-cache probing behaviour, kept
    /// as the reference the cached parallel path is checked against.
    pub fn sequential_uncached() -> Self {
        Self {
            cache_dir: None,
            parallelism: 1,
            config_hash: None,
            peripheral: None,
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        }
    }
}

/// Probe the base and queued heads. `heads` maps each queued change to its current head.
/// Conflicts confined to `regenerable` paths are not conflicts: the queue regenerates them.
///
/// This is [`probe_with_options`] with bounded parallelism and an in-memory cache.
pub fn probe(
    git: &Git,
    base_sha: &str,
    heads: &BTreeMap<i64, Option<String>>,
    regenerable: Regenerable<'_>,
) -> Result<Probe, String> {
    probe_with_options(git, base_sha, heads, regenerable, ProbeOptions::default())
}

/// Run `count` independent jobs, at most `limit` at once, collecting results by job index
/// and the peak number running at once. This is the dagrun shape in miniature: the jobs
/// have no dependencies, so a flat bound is the whole schedule, and assembly afterwards in
/// index order keeps the result independent of completion order.
fn run_bounded<T, F>(count: usize, limit: usize, job: F) -> (Vec<Result<T, String>>, usize)
where
    T: Send,
    F: Fn(usize) -> Result<T, String> + Sync,
{
    if count == 0 {
        return (Vec::new(), 0);
    }
    let limit = limit.max(1).min(count);
    if limit == 1 {
        return ((0..count).map(&job).collect(), 1);
    }
    let results: Mutex<Vec<Option<Result<T, String>>>> =
        Mutex::new((0..count).map(|_| None).collect());
    let next = AtomicUsize::new(0);
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..limit {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::SeqCst);
                if index >= count {
                    break;
                }
                let running = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(running, Ordering::SeqCst);
                let result = job(index);
                active.fetch_sub(1, Ordering::SeqCst);
                results.lock().unwrap()[index] = Some(result);
            });
        }
    });
    let results = results
        .into_inner()
        .unwrap()
        .into_iter()
        .map(|result| result.expect("every bounded job ran"))
        .collect();
    (results, peak.load(Ordering::SeqCst))
}

/// [`probe`] with an explicit cache, concurrency bound and config hash.
///
/// Base probes run first (the pairwise file-disjointness skip needs their file sets),
/// then pairwise probes. Both phases run cache misses under the bound and assemble in
/// deterministic index order, so the resulting [`Probe`] equals the sequential uncached
/// one for the same inputs. Cache hits run no git subprocess at all. Cached conflict
/// paths are stored raw; `regenerable` filtering happens during assembly, on hits and
/// misses alike.
pub fn probe_with_options(
    git: &Git,
    base_sha: &str,
    heads: &BTreeMap<i64, Option<String>>,
    regenerable: Regenerable<'_>,
    options: ProbeOptions,
) -> Result<Probe, String> {
    probe_impl(git, base_sha, heads, regenerable, options)
}

/// First-stage probe for pipelined planning. It decides every pair, exactly like
/// [`probe_with_options`]: with batched `merge-tree` workers and pairs skipped when they share
/// no non-generated file, a full probe costs about what an optimistic first stage used to, and
/// an optimistic first batch (unknown pairs treated as clean) could put two conflicting changes
/// together and eject the second, where the completed plan would only hold it, or land a change
/// stacked on a held one. The continuation's finish is then all cache hits.
pub fn probe_first_stage(
    git: &Git,
    base_sha: &str,
    heads: &BTreeMap<i64, Option<String>>,
    regenerable: Regenerable<'_>,
    options: ProbeOptions,
) -> Result<Probe, String> {
    probe_impl(git, base_sha, heads, regenerable, options)
}

/// Ask the peripheral hook about one change: run its command in a temporary worktree of
/// `head`. Only a first output line of exactly `peripheral` is peripheral; a hook that
/// cannot run, answers anything else, or runs past its timeout is binary-touching
/// (fail closed), and that answer is what gets cached.
fn classify_peripheral(git: &Git, hook: &PeripheralHook, base: &str, head: &str, pr: i64) -> bool {
    let dir = std::env::temp_dir().join(format!(
        "merge-queue-peripheral-{}-{pr}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let added = git.run(&[
        "worktree",
        "add",
        "--detach",
        "--quiet",
        &dir.display().to_string(),
        head,
    ]);
    let answer = added.and_then(|_| {
        let mut command = std::process::Command::new("sh");
        command
            .arg("-c")
            .arg(&hook.command)
            .current_dir(&dir)
            .env("MQ_BASE", base)
            .env("MQ_HEAD", head)
            .env("MQ_PR", pr.to_string())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("cannot run peripheral.command: {error}"))?;
        let limit = std::time::Duration::from_secs(hook.timeout_secs.max(1));
        let started = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    let mut out = String::new();
                    if let Some(mut stdout) = child.stdout.take() {
                        use std::io::Read;
                        let _ = stdout.read_to_string(&mut out);
                    }
                    return Ok(out.lines().next().map(str::trim) == Some("peripheral"));
                }
                Ok(None) if started.elapsed() <= limit => {
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(false);
                }
                Err(error) => return Err(format!("cannot wait for peripheral.command: {error}")),
            }
        }
    });
    let _ = git.run(&["worktree", "remove", "--force", &dir.display().to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
    answer.unwrap_or(false)
}

fn probe_impl(
    git: &Git,
    base_sha: &str,
    heads: &BTreeMap<i64, Option<String>>,
    regenerable: Regenerable<'_>,
    options: ProbeOptions,
) -> Result<Probe, String> {
    // A conflict is recorded with only its non-regenerable paths: a conflict confined to
    // regenerable paths is not a conflict at all (the queue regenerates those files on the
    // candidate), and a mixed conflict blocks only on its real paths.
    let real = |paths: &[String]| -> Vec<String> {
        paths
            .iter()
            .filter(|path| !regenerable(path))
            .cloned()
            .collect()
    };
    let config_hash = options
        .config_hash
        .clone()
        .unwrap_or_else(|| probe_config_hash(git, ""));
    let mut cache = match &options.cache_dir {
        Some(dir) => ProbeCache::load(dir),
        None => ProbeCache::ephemeral(),
    };
    let mut probe = Probe {
        heads: heads.clone(),
        ..Probe::default()
    };
    let live: Vec<(i64, String)> = heads
        .iter()
        .filter_map(|(pr, head)| head.clone().map(|head| (*pr, head)))
        .collect();
    let mut stats = ProbeStats::default();

    // An exact head's cached entry must never answer for (or be answered by) the
    // same head probed as a combining change: the verdicts differ by construction.
    let base_key_for = |pr: i64, head: &str| {
        let key = ProbeCache::base_key(&config_hash, base_sha, head);
        if options.exact.contains(&pr) {
            format!("{key}:exact")
        } else {
            key
        }
    };

    // Phase 1, per head: files touched and the commits the head adds over the base (one
    // `rev-list BASE..HEAD`, which answers every pairwise ancestry question for that head).
    // Under merge combine the head's conflicts with the base join the merge batch below;
    // under rebase the fetch job replays the head onto the base for its verdict, and an
    // exact head resolves to no probe conflicts (build's `lands_exactly` decides).
    let mut entries: BTreeMap<i64, BaseEntry> = BTreeMap::new();
    let mut base_missing: Vec<(i64, String)> = Vec::new();
    for (pr, head) in &live {
        match cache.base(&base_key_for(*pr, head)) {
            Some(entry) => {
                stats.base_hits += 1;
                entries.insert(*pr, entry.clone());
            }
            None => {
                stats.base_misses += 1;
                base_missing.push((*pr, head.clone()));
            }
        }
    }
    let (fetched, base_peak) = run_bounded(base_missing.len(), options.parallelism, |index| {
        let (pr, head) = &base_missing[index];
        // A verdict decided here, without the merge batch below: an exact head is
        // never merge-probed (build's `lands_exactly` decides, ejecting a moved
        // base as not-fast-forward), and under rebase the verdict is the replay
        // itself, through the same code path that will combine the candidate.
        let verdict = if options.exact.contains(pr) {
            Some(Vec::new())
        } else if options.combine == CombineMode::Rebase {
            Some(
                match git.combine_with(CombineMode::Rebase, base_sha, head, &[], "", regenerable)? {
                    Combined::Conflict(paths) => paths,
                    Combined::Clean(_) | Combined::NeedsRegen(..) => Vec::new(),
                },
            )
        } else {
            None
        };
        Ok((
            git.changed_files(base_sha, head)?,
            git.range(base_sha, head)?,
            verdict,
        ))
    });
    let mut merges: Vec<(String, String)> = Vec::new();
    let mut merge_missing: Vec<(i64, String)> = Vec::new();
    for ((pr, head), result) in base_missing.iter().zip(fetched) {
        let (files, range, verdict) = result?;
        let entry = BaseEntry {
            files: files.into_iter().collect(),
            conflicts: verdict.clone().unwrap_or_default(),
            range: range.into_iter().collect(),
        };
        if verdict.is_some() {
            cache.put_base(base_key_for(*pr, head), entry.clone());
        } else {
            merges.push((base_sha.to_owned(), head.clone()));
            merge_missing.push((*pr, head.clone()));
        }
        entries.insert(*pr, entry);
    }

    // Phase 2, per pair: ancestry from the ranges (no git), then a merge only when the two
    // changes share a file outside the regenerable paths, a necessary condition for a textual
    // conflict - or when the footprint prior marks them suspect.
    let ranges: BTreeMap<i64, BTreeSet<String>> = entries
        .iter()
        .map(|(pr, entry)| (*pr, entry.range.iter().cloned().collect()))
        .collect();
    let files: BTreeMap<i64, BTreeSet<String>> = entries
        .iter()
        .map(|(pr, entry)| (*pr, entry.files.iter().cloned().collect()))
        .collect();
    // The footprint prior: tests whose footprint touches a change's files are that
    // change's affected tests; two changes sharing an affected test are suspect even
    // when their files are disjoint. The prior only widens and orders probing - the
    // merge-tree probe below remains the only conflict verdict.
    if let Some(path) = &options.footprints {
        if let Ok(text) = git.run(&["show", &format!("{base_sha}:{path}")]) {
            if let Ok(footprints) = serde_json::from_str::<BTreeMap<String, Vec<String>>>(&text) {
                let affected: BTreeMap<i64, BTreeSet<&str>> = live
                    .iter()
                    .map(|(pr, _)| {
                        let tests = footprints
                            .iter()
                            .filter(|(_, paths)| paths.iter().any(|p| files[pr].contains(p)))
                            .map(|(test, _)| test.as_str())
                            .collect();
                        (*pr, tests)
                    })
                    .collect();
                for (i, (a, _)) in live.iter().enumerate() {
                    for (b, _) in &live[i + 1..] {
                        if !affected[a].is_disjoint(&affected[b]) {
                            probe.prior.push((*a, *b));
                        }
                    }
                }
                probe.prior.sort_unstable();
            }
        }
    }

    let mut to_merge: Vec<(i64, i64, String, String)> = Vec::new();
    let mut cached_conflicts: Vec<(i64, i64, Vec<String>)> = Vec::new();
    for (i, (a, head_a)) in live.iter().enumerate() {
        for (b, head_b) in &live[i + 1..] {
            if head_a == head_b {
                continue;
            }
            if ranges[b].contains(head_a.as_str()) {
                probe.ancestry.push((*a, *b));
                stats.pairs_skipped += 1;
                continue;
            }
            if ranges[a].contains(head_b.as_str()) {
                probe.ancestry.push((*b, *a));
                stats.pairs_skipped += 1;
                continue;
            }
            if !files[a]
                .intersection(&files[b])
                .any(|path| !regenerable(path))
                && !probe.prior.contains(&(*a, *b))
            {
                stats.pairs_skipped += 1;
                continue;
            }
            let (key, _, _) = ProbeCache::pair_key(&config_hash, head_a, head_b);
            if let Some(conflicts) = cache.pair(&key).and_then(|entry| entry.conflicts.clone()) {
                stats.pair_hits += 1;
                cached_conflicts.push((*a, *b, conflicts));
                continue;
            }
            stats.pair_misses += 1;
            merges.push((head_a.clone(), head_b.clone()));
            to_merge.push((*a, *b, head_a.clone(), head_b.clone()));
        }
    }

    // Run every uncached merge on long-lived `git merge-tree --stdin` processes, one per
    // worker, each taking a contiguous share of the queries.
    let mut answers: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut merge_peak = 0;
    if !merges.is_empty() {
        let workers = options.parallelism.clamp(1, merges.len());
        let share = merges.len().div_ceil(workers);
        let chunks: Vec<&[(String, String)]> = merges.chunks(share).collect();
        let (results, peak) = run_bounded(chunks.len(), workers, |index| {
            let mut batch = git.merge_tree_batch()?;
            chunks[index]
                .iter()
                .map(|(left, right)| {
                    Ok(((left.clone(), right.clone()), batch.conflicts(left, right)?))
                })
                .collect::<Result<Vec<_>, String>>()
        });
        merge_peak = peak;
        for result in results {
            answers.extend(result?);
        }
    }

    // Record and assemble. Only merge-combine heads are still undecided here:
    // rebase replays and exact heads arrived with their verdicts (and cache rows).
    for (pr, head) in &merge_missing {
        let entry = entries.get_mut(pr).ok_or("probe lost a base entry")?;
        entry.conflicts = answers
            .get(&(base_sha.to_owned(), head.clone()))
            .cloned()
            .ok_or("probe lost a base merge")?;
        cache.put_base(base_key_for(*pr, head), entry.clone());
    }
    for (pr, entry) in &entries {
        let paths = real(&entry.conflicts);
        if !paths.is_empty() {
            probe.base_conflicts.insert(*pr, paths);
        }
    }
    let mut raw_pairs = cached_conflicts;
    for (a, b, head_a, head_b) in to_merge {
        let conflicts = answers
            .get(&(head_a.clone(), head_b.clone()))
            .cloned()
            .ok_or("probe lost a pair merge")?;
        let (key, _, _) = ProbeCache::pair_key(&config_hash, &head_a, &head_b);
        cache.put_pair(
            key,
            PairEntry {
                ancestry: PairAncestry::None,
                conflicts: Some(conflicts.clone()),
            },
        );
        raw_pairs.push((a, b, conflicts));
    }
    for (a, b, conflicts) in raw_pairs {
        let paths = real(&conflicts);
        if !paths.is_empty() {
            probe.conflicts.push(Conflict {
                a: a.min(b),
                b: a.max(b),
                paths,
            });
        }
    }
    probe.files = files;
    probe.ranges = ranges
        .iter()
        .map(|(pr, range)| {
            (
                *pr,
                range.iter().map(|commit| (*commit).to_owned()).collect(),
            )
        })
        .collect();
    probe
        .conflicts
        .sort_by_key(|conflict| (conflict.a, conflict.b));
    probe.ancestry.sort();
    stats.cache_hits = stats.base_hits + stats.pair_hits;
    stats.cache_misses = stats.base_misses + stats.pair_misses;
    stats.peak_concurrency = base_peak.max(merge_peak);
    probe.stats = stats;
    // Peripheral classification: cached per (hook, base, head), so an unchanged pair is
    // never re-asked and a changed head or base always is. No hook, no marks: every
    // change is binary-touching.
    if let Some(hook) = &options.peripheral {
        let cmd_hash = probe_config_hash(
            git,
            &format!("peripheral:{}:{}", hook.command, hook.timeout_secs),
        );
        for (pr, head) in &live {
            let key = ProbeCache::peripheral_key(&cmd_hash, base_sha, head);
            let mark = match cache.peripheral(&key) {
                Some(entry) => PeripheralMark {
                    peripheral: entry.peripheral,
                    cached: true,
                },
                None => {
                    let peripheral = classify_peripheral(git, hook, base_sha, head, *pr);
                    cache.put_peripheral(key, crate::probe_cache::PeripheralEntry { peripheral });
                    PeripheralMark {
                        peripheral,
                        cached: false,
                    }
                }
            };
            probe.peripheral.insert(*pr, mark);
        }
    }
    // Keep the cache proportional to the queue: entries for other heads or bases can no longer
    // be asked for (entries under another merge configuration are kept, so switching back hits). The cache is an optimisation: a failed write
    // must not fail planning.
    let current: BTreeSet<&str> = live.iter().map(|(_, head)| head.as_str()).collect();
    cache.retain(base_sha, &current);
    let _ = cache.save();
    Ok(probe)
}

/// Planning knobs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlanOptions {
    /// Maximum changes per batch (at least 1).
    pub max_batch: usize,
    /// Priority label and CI paths (for the label suggestion).
    pub priority: PriorityConfig,
    /// Maximum changes in a batch of only peripheral changes (0 = `max_batch`: no
    /// wider peripheral batches, and no separation either when 0 with no hook).
    pub peripheral_max_batch: usize,
}

/// Not at the front (priority changes and their dependencies sort first), then priority,
/// conflict degree, submission time and number.
type Rank<'a> = (bool, i64, usize, &'a str, i64);

/// Order and chunk the queued items. `all_items` may include landed and ejected records, which
/// satisfy or block explicit dependencies; only `Queued` items are planned.
pub fn make_plan(
    all_items: &[Item],
    base: &str,
    base_sha: &str,
    probe: &Probe,
    options: PlanOptions,
) -> Plan {
    let max_batch = options.max_batch.max(1);
    let by_pr: BTreeMap<i64, &Item> = all_items.iter().map(|item| (item.pr, item)).collect();
    let queued: Vec<&Item> = all_items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued)
        .collect();
    let mut held: BTreeMap<i64, Held> = BTreeMap::new();
    let hold = |held: &mut BTreeMap<i64, Held>, entry: Held| {
        held.entry(entry.pr).or_insert(entry);
    };

    // 1. Identity and base checks.
    for item in &queued {
        match probe.heads.get(&item.pr).cloned().flatten() {
            None => hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "head-missing".into(),
                    reason: format!(
                        "head {:?} no longer resolves; resubmit or withdraw",
                        item.head_ref
                    ),
                    with: vec![],
                    paths: vec![],
                },
            ),
            Some(current) if current != item.head => hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "head-moved".into(),
                    reason: format!(
                        "head moved since submission ({} -> {}); resubmit the new head",
                        short(&item.head),
                        short(&current)
                    ),
                    with: vec![],
                    paths: vec![],
                },
            ),
            Some(_) => {
                if let Some(paths) = probe.base_conflicts.get(&item.pr).filter(|p| !p.is_empty()) {
                    hold(
                        &mut held,
                        Held {
                            pr: item.pr,
                            kind: "conflicts-with-base".into(),
                            reason: format!(
                                "conflicts with {base} on {}; rebase onto {base} and resubmit",
                                paths.join(", ")
                            ),
                            with: vec![],
                            paths: paths.clone(),
                        },
                    );
                }
            }
        }
    }

    // Deferred after a validation refusal: not landable until the backoff expires. This is
    // a non-terminal queued state, distinct from every failure hold: it is not red, the
    // change is not ejected, and no test is blamed.
    let at = now();
    for item in &queued {
        // Escalated to its owner after the bounded number of deferrals: held, not retried,
        // until `merge-queue retry` clears it.
        if !item.escalated.is_empty() {
            hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "escalated".into(),
                    reason: format!(
                        "escalated to its owner: {}; `merge-queue retry {}` returns it to the queue",
                        item.escalated, item.pr
                    ),
                    with: vec![],
                    paths: vec![],
                },
            );
            continue;
        }
        if item.is_flaky_held_at(&at) && item.hold_verdict == crate::validate::SUSPECT_FLAKE_MARKER
        {
            hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: item.hold_kind().into(),
                    reason: format!(
                        "{}: {} timed out or reached no verdict once; they run again beside the \
                         run, and it goes back to the queue if they pass (held until {})",
                        item.hold_label(),
                        item.flaky_nodes.join(", "),
                        item.flaky_hold_until,
                    ),
                    with: vec![],
                    paths: vec![],
                },
            );
            continue;
        }
        if item.is_flaky_held_at(&at) {
            let why = if item.hold_verdict == crate::validate::INFRA_BLOCKED_MARKER {
                "the host refused every measurement of them on a run and on its rerun"
            } else {
                "passed only after a retry"
            };
            let ruling = match (&item.held_candidate, &item.accepted) {
                (Some(candidate), Some(ruling)) => format!(
                    "accepted by {}: a run lands {} if the base is still {}",
                    ruling.by,
                    short(&candidate.tip),
                    short(&candidate.parent)
                ),
                (Some(candidate), None) => format!(
                    "`merge-queue accept-flaky {} --by WHO --reason WHY` lands the validated commit \
                     {}, and `merge-queue retry {}` validates it again",
                    candidate
                        .prs()
                        .iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                    short(&candidate.tip),
                    item.pr
                ),
                _ => format!("`merge-queue retry {}` returns it to the queue", item.pr),
            };
            hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: item.hold_kind().into(),
                    reason: format!(
                        "validation was {} ({}: {why}); held for a ruling until {}; {ruling}",
                        item.hold_label(),
                        item.flaky_nodes.join(", "),
                        item.flaky_hold_until,
                    ),
                    with: vec![],
                    paths: vec![],
                },
            );
            continue;
        }
        if item.is_deferred_at(&at) {
            hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "validation-refused".into(),
                    reason: format!(
                        "validation admission refused to run ({}); stays queued, retry after {} (refusal {})",
                        if item.last_refusal.is_empty() {
                            "retry later".to_owned()
                        } else {
                            item.last_refusal.clone()
                        },
                        item.next_retry_at,
                        item.validation_refusals
                    ),
                    with: vec![],
                    paths: vec![],
                },
            );
        }
    }

    for item in &queued {
        if let Some(external) = probe.external.get(&item.pr) {
            hold(&mut held, external.clone());
        }
    }
    // A change stacked on one that was ejected or withdrawn still carries that change's
    // commits: landing it would land them too.
    for item in &queued {
        let Some(range) = probe.ranges.get(&item.pr) else {
            continue;
        };
        let carried: Vec<i64> = all_items
            .iter()
            .filter(|other| {
                matches!(other.status, ItemStatus::Ejected | ItemStatus::Withdrawn)
                    && other.pr != item.pr
                    && range.contains(&other.head)
            })
            .map(|other| other.pr)
            .collect();
        if let Some(first) = carried.first() {
            let status = by_pr[first].status.as_str();
            hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "waiting-on-dependency".into(),
                    reason: format!(
                        "contains the commits of #{first}, which is {status}; rebase without them or resubmit #{first}"
                    ),
                    with: carried,
                    paths: vec![],
                },
            );
        }
    }

    // 2. Dependencies: explicit `after` plus head ancestry.
    let queued_set: BTreeSet<i64> = queued.iter().map(|item| item.pr).collect();
    let mut preds: BTreeMap<i64, BTreeSet<i64>> =
        queued_set.iter().map(|pr| (*pr, BTreeSet::new())).collect();
    let mut dependencies = BTreeSet::new();
    for item in &queued {
        for dep in &item.after {
            match by_pr.get(dep).map(|dep| dep.status) {
                Some(ItemStatus::Landed) => {}
                Some(ItemStatus::Queued) => {
                    preds.entry(item.pr).or_default().insert(*dep);
                    dependencies.insert((*dep, item.pr));
                }
                other => hold(
                    &mut held,
                    Held {
                        pr: item.pr,
                        kind: "waiting-on-dependency".into(),
                        reason: format!(
                            "depends on #{dep}, which is {}; submit it or drop the dependency",
                            other.map_or("not in the queue", ItemStatus::as_str)
                        ),
                        with: vec![*dep],
                        paths: vec![],
                    },
                ),
            }
        }
    }
    for (before, after) in &probe.ancestry {
        if queued_set.contains(before) && queued_set.contains(after) {
            preds.entry(*after).or_default().insert(*before);
            dependencies.insert((*before, *after));
        }
    }

    // 3. Conflict-aware selection in dependency order.
    let mut conflicts: BTreeMap<i64, BTreeMap<i64, Vec<String>>> = BTreeMap::new();
    for edge in &probe.conflicts {
        conflicts
            .entry(edge.a)
            .or_default()
            .insert(edge.b, edge.paths.clone());
        conflicts
            .entry(edge.b)
            .or_default()
            .insert(edge.a, edge.paths.clone());
    }
    let eligible = |held: &BTreeMap<i64, Held>, pr: &i64| !held.contains_key(pr);
    let degree = |held: &BTreeMap<i64, Held>, pr: i64| {
        conflicts.get(&pr).map_or(0, |peers| {
            peers.keys().filter(|peer| eligible(held, peer)).count()
        })
    };
    let degrees: BTreeMap<i64, usize> = queued_set
        .iter()
        .map(|pr| (*pr, degree(&held, *pr)))
        .collect();
    // The front of the queue: priority changes and, transitively, what they must land after.
    let mut front: BTreeSet<i64> = queued
        .iter()
        .filter(|item| item.prioritized)
        .map(|item| item.pr)
        .collect();
    let mut stack: Vec<i64> = front.iter().copied().collect();
    while let Some(pr) = stack.pop() {
        for dep in preds.get(&pr).into_iter().flatten() {
            if front.insert(*dep) {
                stack.push(*dep);
            }
        }
    }
    let rank = |pr: i64| -> Rank<'_> {
        let item = by_pr[&pr];
        (
            !front.contains(&pr),
            item.priority,
            degrees[&pr],
            item.submitted_at.as_str(),
            pr,
        )
    };
    let mut remaining: BTreeSet<i64> = queued_set.clone();
    let mut decided: BTreeSet<i64> = BTreeSet::new();
    let mut selected: Vec<i64> = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .copied()
            .filter(|pr| preds[pr].is_subset(&decided))
            .min_by_key(|pr| rank(*pr));
        let Some(pr) = ready.or_else(|| remaining.iter().copied().min_by_key(|pr| rank(*pr)))
        else {
            break;
        };
        remaining.remove(&pr);
        decided.insert(pr);
        if held.contains_key(&pr) {
            continue;
        }
        let blocked_by: Vec<i64> = preds[&pr]
            .iter()
            .copied()
            .filter(|dep| !selected.contains(dep))
            .collect();
        if !blocked_by.is_empty() {
            hold(
                &mut held,
                Held {
                    pr,
                    kind: "waiting-on-dependency".into(),
                    reason: format!(
                        "must land after {}, which cannot land this round",
                        list(&blocked_by)
                    ),
                    with: blocked_by,
                    paths: vec![],
                },
            );
            continue;
        }
        let clashes: Vec<(i64, Vec<String>)> = conflicts
            .get(&pr)
            .into_iter()
            .flatten()
            .filter(|(peer, _)| selected.contains(peer))
            .map(|(peer, paths)| (*peer, paths.clone()))
            .collect();
        if clashes.is_empty() {
            selected.push(pr);
        } else {
            let with: Vec<i64> = clashes.iter().map(|(peer, _)| *peer).collect();
            let paths: BTreeSet<String> =
                clashes.into_iter().flat_map(|(_, paths)| paths).collect();
            hold(
                &mut held,
                Held {
                    pr,
                    kind: "conflicts-with-queued".into(),
                    reason: format!(
                        "conflicts with {} on {}; rebase after {} lands and resubmit",
                        list(&with),
                        paths.iter().cloned().collect::<Vec<_>>().join(", "),
                        if with.len() == 1 { "it" } else { "they" }
                    ),
                    with,
                    paths: paths.into_iter().collect(),
                },
            );
        }
    }

    // 4. Chunk the selected order into batches.
    let mut batches: Vec<PlannedBatch> = Vec::new();
    let mut current: Vec<i64> = Vec::new();
    let close = |current: &mut Vec<i64>, batches: &mut Vec<PlannedBatch>, note: &str| {
        if !current.is_empty() {
            batches.push(PlannedBatch {
                index: batches.len(),
                prs: std::mem::take(current),
                note: note.to_owned(),
            });
        }
    };
    // Peripheral changes never share a batch with binary-touching ones, and a run of
    // peripheral changes chunks at the wider peripheral cap: they cannot conflict
    // through the product binary, so width is cheap for them.
    let peripheral_limit = options.peripheral_max_batch.max(max_batch);
    let mut current_peripheral = false;
    for pr in &selected {
        let item = by_pr[pr];
        if item.solo || item.exact || item.prioritized {
            close(&mut current, &mut batches, "");
            current.push(*pr);
            let note = if item.exact {
                "exact"
            } else if item.prioritized {
                "priority"
            } else {
                "solo"
            };
            close(&mut current, &mut batches, note);
            continue;
        }
        let peripheral = probe.peripheral.get(pr).is_some_and(|mark| mark.peripheral);
        if !current.is_empty() && peripheral != current_peripheral {
            let note = if current_peripheral { "peripheral" } else { "" };
            close(&mut current, &mut batches, note);
        }
        if current.is_empty() {
            current_peripheral = peripheral;
        }
        current.push(*pr);
        let limit = if peripheral {
            peripheral_limit
        } else {
            max_batch
        };
        if current.len() >= limit {
            close(
                &mut current,
                &mut batches,
                if peripheral {
                    "peripheral"
                } else {
                    "max-batch"
                },
            );
        }
    }
    close(&mut current, &mut batches, "");

    let items = queued
        .iter()
        .map(|item| PlannedItem {
            pr: item.pr,
            head: probe
                .heads
                .get(&item.pr)
                .cloned()
                .flatten()
                .unwrap_or_default(),
            priority: item.priority,
            submitted_at: item.submitted_at.clone(),
            files: probe.files.get(&item.pr).map_or(0, BTreeSet::len),
            conflict_degree: degrees[&item.pr],
            prioritized: item.prioritized,
            peripheral: probe
                .peripheral
                .get(&item.pr)
                .is_some_and(|mark| mark.peripheral),
            peripheral_source: probe
                .peripheral
                .get(&item.pr)
                .map_or_else(String::new, |mark| {
                    if mark.cached {
                        "cache".to_owned()
                    } else {
                        "hook".to_owned()
                    }
                }),
        })
        .collect();
    let suggest_priority = queued
        .iter()
        .filter(|item| {
            !item.prioritized
                && probe
                    .files
                    .get(&item.pr)
                    .is_some_and(|files| options.priority.only_ci_paths(files))
        })
        .map(|item| item.pr)
        .collect();
    Plan {
        schema: PLAN_SCHEMA.to_owned(),
        created_at: now(),
        base: base.to_owned(),
        base_sha: base_sha.to_owned(),
        items,
        batches,
        held: held.into_values().collect(),
        conflicts: probe.conflicts.clone(),
        dependencies: dependencies.into_iter().collect(),
        suggest_priority,
        prior_conflicts: probe.prior.clone(),
    }
}

/// Conflict clusters (connected components of the pairwise-conflict graph) with two or more
/// members, via the landing planner's component builder.
pub fn clusters(plan: &Plan) -> Vec<Vec<i64>> {
    let numbers: Vec<i64> = plan.items.iter().map(|item| item.pr).collect();
    let edges: Vec<ConflictEdge> = plan
        .conflicts
        .iter()
        .map(|edge| ConflictEdge {
            a: edge.a,
            b: edge.b,
            paths: edge.paths.clone(),
        })
        .collect();
    connected_components(&numbers, &edges)
        .into_iter()
        .filter(|component| component.len() > 1)
        .collect()
}

/// Render a plan for people.
pub fn render(plan: &Plan) -> String {
    let mut out = String::new();
    let queued = plan.items.len();
    let landable: usize = plan.batches.iter().map(|batch| batch.prs.len()).sum();
    out.push_str(&format!(
        "Plan for {} @ {}: {queued} queued, {landable} landable in {} batch(es), {} held\n",
        plan.base,
        short(&plan.base_sha),
        plan.batches.len(),
        plan.held.len()
    ));
    let by_pr: BTreeMap<i64, &PlannedItem> =
        plan.items.iter().map(|item| (item.pr, item)).collect();
    for batch in &plan.batches {
        let members: Vec<String> = batch
            .prs
            .iter()
            .map(|pr| {
                let item = by_pr[pr];
                let front = if item.prioritized { "priority, " } else { "" };
                format!("#{pr} ({front}p{}, {} files)", item.priority, item.files)
            })
            .collect();
        let note = if batch.note.is_empty() {
            String::new()
        } else {
            format!("  [{}]", batch.note)
        };
        out.push_str(&format!(
            "  batch {}: {}{note}\n",
            batch.index + 1,
            members.join(", ")
        ));
    }
    if !plan.held.is_empty() {
        out.push_str("Held:\n");
        for held in &plan.held {
            out.push_str(&format!("  #{} {}: {}\n", held.pr, held.kind, held.reason));
        }
    }
    let clusters = clusters(plan);
    if !clusters.is_empty() {
        out.push_str("Conflict clusters:\n");
        for cluster in clusters {
            out.push_str(&format!("  {}\n", list(&cluster)));
        }
    }
    if !plan.suggest_priority.is_empty() {
        out.push_str(&format!(
            "Suggested priority label (changes only CI paths; never applied by the queue): {}\n",
            list(&plan.suggest_priority)
        ));
    }
    out
}

/// First 10 characters of a commit ID.
pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn list(prs: &[i64]) -> String {
    prs.iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod unit {
    use super::*;

    fn item(pr: i64, priority: i64, at: &str) -> Item {
        Item {
            pr,
            head_ref: format!("b{pr}"),
            head: format!("h{pr}"),
            priority,
            submitted_at: at.to_owned(),
            submitted_by: String::new(),
            shepherd: String::new(),
            notify: String::new(),
            notify_mode: None,
            notify_delivery: crate::model::NotifyDelivery::default(),
            title: String::new(),
            after: vec![],
            solo: false,
            prioritized: false,
            claim: None,
            exact: false,
            exact_peer_head: String::new(),
            requires: vec![],
            source: String::new(),
            close_pending: false,
            validation_refusals: 0,
            suspect_reruns: 0,
            next_retry_at: String::new(),
            last_refusal: String::new(),
            flaky_nodes: Vec::new(),
            flaky_hold_until: String::new(),
            hold_verdict: String::new(),
            held_candidate: None,
            accepted: None,
            escalated: String::new(),
            deciding_report: String::new(),
            deciding_failed: vec![],
            status: ItemStatus::Queued,
            landed_as: String::new(),
            landed_map: Vec::new(),
            landed_batch: String::new(),
            history: vec![],
        }
    }

    fn probe_for(items: &[Item]) -> Probe {
        Probe {
            heads: items
                .iter()
                .map(|item| (item.pr, Some(item.head.clone())))
                .collect(),
            files: items
                .iter()
                .map(|item| (item.pr, [format!("f{}", item.pr)].into()))
                .collect(),
            ..Probe::default()
        }
    }

    fn conflict(a: i64, b: i64) -> Conflict {
        Conflict {
            a,
            b,
            paths: vec!["shared".into()],
        }
    }

    fn opts(max_batch: usize) -> PlanOptions {
        PlanOptions {
            max_batch,
            peripheral_max_batch: 0,
            ..PlanOptions::default()
        }
    }

    #[test]
    fn orders_by_priority_then_age_and_chunks() {
        let items = vec![item(1, 100, "t3"), item(2, 50, "t9"), item(3, 100, "t1")];
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(2));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![2, 3], vec![1]]);
        assert!(plan.held.is_empty());
    }

    #[test]
    fn peripheral_changes_batch_wide_and_never_mix_with_binary_touching() {
        let items = vec![
            item(1, 100, "t1"),
            item(2, 100, "t2"),
            item(3, 100, "t3"),
            item(4, 100, "t4"),
            item(5, 100, "t5"),
        ];
        let mut probe = probe_for(&items);
        for pr in [1, 2, 4, 5] {
            probe.peripheral.insert(
                pr,
                PeripheralMark {
                    peripheral: true,
                    cached: pr == 5,
                },
            );
        }
        let mut options = opts(1);
        options.peripheral_max_batch = 3;
        let plan = make_plan(&items, "main", "B", &probe, options);
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![1, 2], vec![3], vec![4, 5]], "{plan:?}");
        assert_eq!(plan.batches[0].note, "peripheral");
        let by_pr: BTreeMap<i64, _> = plan.items.iter().map(|i| (i.pr, i)).collect();
        assert!(by_pr[&1].peripheral && by_pr[&1].peripheral_source == "hook");
        assert!(by_pr[&5].peripheral && by_pr[&5].peripheral_source == "cache");
        assert!(!by_pr[&3].peripheral && by_pr[&3].peripheral_source.is_empty());
    }

    #[test]
    fn hub_conflict_loses_to_its_independent_peers() {
        // #1 conflicts with #2, #3 and #4, which are mutually clean. FIFO would land only #1;
        // fewest-conflicts-first lands three.
        let items = vec![
            item(1, 100, "t1"),
            item(2, 100, "t2"),
            item(3, 100, "t3"),
            item(4, 100, "t4"),
        ];
        let mut probe = probe_for(&items);
        probe.conflicts = vec![conflict(1, 2), conflict(1, 3), conflict(1, 4)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![2, 3, 4]);
        assert_eq!(plan.held.len(), 1);
        assert_eq!(plan.held[0].kind, "conflicts-with-queued");
        assert_eq!(plan.held[0].with, vec![2, 3, 4]);
        assert_eq!(clusters(&plan), vec![vec![1, 2, 3, 4]]);
    }

    #[test]
    fn priority_beats_conflict_degree() {
        let items = vec![item(1, 10, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.conflicts = vec![conflict(1, 2), conflict(1, 3)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![1]);
        assert_eq!(plan.held.len(), 2);
    }

    #[test]
    fn a_priority_change_goes_first_in_a_batch_of_its_own() {
        // Queued last and with the worst numeric priority, behind three ordinary changes.
        let mut items = vec![
            item(1, 100, "t1"),
            item(2, 100, "t2"),
            item(3, 100, "t3"),
            item(4, 500, "t4"),
        ];
        items[3].prioritized = true;
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![4], vec![1, 2, 3]]);
        assert_eq!(plan.batches[0].note, "priority");
        assert!(render(&plan).contains("#4 (priority, p500"));
    }

    #[test]
    fn a_priority_change_wins_a_conflict_and_pulls_its_dependency_forward() {
        // #4 is prioritized and must land after #3; #1 conflicts with #4.
        let mut items = vec![
            item(1, 10, "t1"),
            item(2, 100, "t2"),
            item(3, 100, "t3"),
            item(4, 100, "t4"),
        ];
        items[3].prioritized = true;
        items[3].after = vec![3];
        let mut probe = probe_for(&items);
        probe.conflicts = vec![conflict(1, 4)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![3], vec![4], vec![2]]);
        assert_eq!(plan.held.len(), 1);
        assert_eq!(plan.held[0].pr, 1);
        assert_eq!(plan.held[0].kind, "conflicts-with-queued");
    }

    #[test]
    fn changes_touching_only_ci_paths_get_a_label_suggestion() {
        let mut items = vec![item(1, 100, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        items[2].prioritized = true;
        let mut probe = probe_for(&items);
        probe.files.insert(1, ["ci/run.sh".to_owned()].into());
        probe
            .files
            .insert(2, ["ci/run.sh".to_owned(), "src/lib.rs".to_owned()].into());
        probe
            .files
            .insert(3, ["validation.dag.yaml".to_owned()].into());
        let options = PlanOptions {
            max_batch: 4,
            peripheral_max_batch: 0,
            priority: PriorityConfig {
                ci_paths: vec!["ci".into(), "*.dag.yaml".into()],
                ..PriorityConfig::default()
            },
        };
        let plan = make_plan(&items, "main", "B", &probe, options);
        // #2 also touches source; #3 already carries the label.
        assert_eq!(plan.suggest_priority, vec![1]);
        assert!(render(&plan).contains("Suggested priority label"));
        // No CI paths configured: no suggestion.
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert!(plan.suggest_priority.is_empty());
    }

    #[test]
    fn the_priority_label_is_configurable_and_case_blind() {
        let labels = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        let default = PriorityConfig::default();
        assert!(default.labelled(&labels(&["bug", "MQ-Priority"])));
        assert!(!default.labelled(&labels(&["p1"])));
        let custom = PriorityConfig {
            label: "ci-fix".into(),
            ..PriorityConfig::default()
        };
        assert!(custom.labelled(&labels(&["ci-fix"])));
        assert!(!custom.labelled(&labels(&["mq-priority"])));
        let off = PriorityConfig {
            label: "-".into(),
            ..PriorityConfig::default()
        };
        assert!(!off.labelled(&labels(&["mq-priority", "-"])));
    }

    #[test]
    fn identity_and_base_holds() {
        let items = vec![item(1, 100, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.heads.insert(1, Some("other".into()));
        probe.heads.insert(2, None);
        probe.base_conflicts.insert(3, vec!["x".into()]);
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert!(plan.batches.is_empty());
        let kinds: Vec<_> = plan.held.iter().map(|h| h.kind.as_str()).collect();
        assert_eq!(kinds, ["head-moved", "head-missing", "conflicts-with-base"]);
    }

    #[test]
    fn deferred_after_refusal_is_held_not_batched() {
        let mut items = vec![item(1, 100, "t1"), item(2, 100, "t2")];
        items[0].validation_refusals = 2;
        items[0].next_retry_at = "9999-01-01T00:00:00Z".into();
        items[0].last_refusal = "exit 75 COULD_NOT_RUN".into();
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        assert_eq!(plan.batches.len(), 1);
        assert_eq!(plan.batches[0].prs, vec![2]);
        assert_eq!(plan.held.len(), 1);
        assert_eq!(plan.held[0].pr, 1);
        assert_eq!(plan.held[0].kind, "validation-refused");
        // Once the retry time has passed the change is planned again.
        items[0].next_retry_at = "2000-01-01T00:00:00Z".into();
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        assert!(plan.held.is_empty());
        assert_eq!(plan.batches[0].prs, vec![1, 2]);
    }

    #[test]
    fn escalated_change_is_held_until_retry() {
        let mut items = vec![item(1, 100, "t1"), item(2, 100, "t2")];
        items[0].escalated = "validation did not run 3 time(s)".into();
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        assert_eq!(plan.batches[0].prs, vec![2]);
        assert_eq!(plan.held[0].pr, 1);
        assert_eq!(plan.held[0].kind, "escalated");
        assert!(plan.held[0].reason.contains("merge-queue retry 1"));
    }

    #[test]
    fn stacked_change_follows_its_parent_and_inherits_its_hold() {
        let items = vec![item(1, 100, "t2"), item(2, 50, "t1"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.ancestry = vec![(1, 2)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![1, 2, 3]);

        probe.base_conflicts.insert(1, vec!["x".into()]);
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![3]);
        let waiting = plan.held.iter().find(|h| h.pr == 2).unwrap();
        assert_eq!(waiting.kind, "waiting-on-dependency");
    }

    #[test]
    fn explicit_dependencies_and_solo() {
        let mut items = vec![item(1, 100, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        items[0].after = vec![3];
        items[1].solo = true;
        let mut landed = item(9, 100, "t0");
        landed.status = ItemStatus::Landed;
        items[2].after = vec![9];
        items.push(landed);
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![2], vec![3, 1]]);
        assert_eq!(plan.batches[0].note, "solo");

        items[2].after = vec![42];
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        let held: Vec<_> = plan.held.iter().map(|h| (h.pr, h.kind.as_str())).collect();
        assert_eq!(
            held,
            [(1, "waiting-on-dependency"), (3, "waiting-on-dependency")]
        );
    }

    #[test]
    fn probe_finds_real_conflicts_and_stacks() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe");
        let git = init(&tmp.0);
        write(&tmp.0, "shared.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("shared.txt", "A\n2\n3\n")]);
        let b = branch(&tmp.0, "b", "main", &[("shared.txt", "B\n2\n3\n")]);
        let c = branch(&tmp.0, "c", "main", &[("shared.txt", "1\n2\nC\n")]);
        let d = branch(&tmp.0, "d", "a", &[("d.txt", "d\n")]);
        let heads = [(1, Some(a)), (2, Some(b)), (3, Some(c)), (4, Some(d))].into();
        let probe = probe(&git, &base, &heads, &crate::git::nothing_regenerable).unwrap();
        let regen = |path: &str| path == "shared.txt";
        let lenient = super::probe(&git, &base, &heads, &regen).unwrap();
        assert!(lenient.conflicts.is_empty());
        // a/b conflict on the same line; c touches another hunk of the same file (clean);
        // d is stacked on a, and so conflicts with b too.
        let pairs: Vec<_> = probe.conflicts.iter().map(|c| (c.a, c.b)).collect();
        assert_eq!(pairs, vec![(1, 2), (2, 4)]);
        assert_eq!(probe.ancestry, vec![(1, 4)]);
        assert!(probe.base_conflicts.is_empty());
    }

    #[test]
    fn peripheral_hook_classifies_fail_closed_and_caches_per_base_and_head() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe-peripheral");
        let cache_tmp = TempDir::new("probe-peripheral-cache");
        let git = init(&tmp.0);
        write(&tmp.0, "shared.txt", "1\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("peripheral.txt", "x\n")]);
        let b = branch(&tmp.0, "b", "main", &[("src.txt", "x\n")]);
        let heads: BTreeMap<i64, Option<String>> = [(1, Some(a.clone())), (2, Some(b))].into();
        let calls = tmp.0.join("hook-calls");
        let hook = PeripheralHook {
            command: format!(
                "echo call >> {}; if [ -f peripheral.txt ]; then echo peripheral; else echo binary-touching; fi",
                calls.display()
            ),
            timeout_secs: 30,
        };
        let options = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            parallelism: 1,
            config_hash: None,
            peripheral: Some(hook.clone()),
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        };
        let first = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            options.clone(),
        )
        .unwrap();
        assert!(first.peripheral[&1].peripheral && !first.peripheral[&1].cached);
        assert!(!first.peripheral[&2].peripheral && !first.peripheral[&2].cached);
        assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 2);
        // Same (base, head): the cache answers, the hook does not run again.
        let second = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            options.clone(),
        )
        .unwrap();
        assert!(second.peripheral[&1].peripheral && second.peripheral[&1].cached);
        assert_eq!(std::fs::read_to_string(&calls).unwrap().lines().count(), 2);
        // A hook that errors is binary-touching too.
        let broken = ProbeOptions {
            peripheral: Some(PeripheralHook {
                command: "exit 3".to_owned(),
                timeout_secs: 30,
            }),
            ..options
        };
        let failed = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            broken,
        )
        .unwrap();
        assert!(!failed.peripheral[&1].peripheral);
    }

    #[test]
    fn footprint_prior_marks_disjoint_files_sharing_a_test_and_probes_them() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe-prior");
        let git = init(&tmp.0);
        write(
            &tmp.0,
            "ci/test-footprints.json",
            "{\"suite\": [\"src/a.rs\", \"src/b.rs\"]}\n",
        );
        write(&tmp.0, "src/a.rs", "a\n");
        write(&tmp.0, "src/b.rs", "b\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("src/a.rs", "a2\n")]);
        let b = branch(&tmp.0, "b", "main", &[("src/b.rs", "b2\n")]);
        let heads: BTreeMap<i64, Option<String>> = [(1, Some(a)), (2, Some(b))].into();
        // No footprints: the file-disjoint pair is never probed and never marked.
        let plain = probe(&git, &base, &heads, &crate::git::nothing_regenerable).unwrap();
        assert!(plain.prior.is_empty() && plain.stats.pairs_skipped >= 1);
        let options = ProbeOptions {
            footprints: Some("ci/test-footprints.json".to_owned()),
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
            ..ProbeOptions::default()
        };
        let probed = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            options,
        )
        .unwrap();
        assert_eq!(probed.prior, vec![(1, 2)]);
        // The prior is not a verdict: disjoint files still do not conflict.
        assert!(probed.conflicts.is_empty(), "{probed:?}");
    }

    fn git_commit(dir: &std::path::Path) {
        use crate::git::testrepo::git;
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "more"]);
    }

    fn regen_config(paths: &[&str]) -> crate::state::RegenerateConfig {
        crate::state::RegenerateConfig {
            paths: paths.iter().map(|path| (*path).to_owned()).collect(),
            command: "regenerate".to_owned(),
            build_command: String::new(),
            when: String::new(),
        }
    }

    #[test]
    fn probe_conflicts_confined_to_regenerated_paths_are_not_conflicts() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe-regen-only");
        let git = init(&tmp.0);
        write(&tmp.0, "generated/output.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        // Both changes touch ONLY the regenerated file, rewriting its first
        // line differently: a textual conflict confined to that file.
        let a = branch(
            &tmp.0,
            "a",
            "main",
            &[("generated/output.txt", "A\n2\n3\n")],
        );
        let b = branch(
            &tmp.0,
            "b",
            "main",
            &[("generated/output.txt", "B\n2\n3\n")],
        );
        let heads: BTreeMap<i64, Option<String>> = [(1, Some(a)), (2, Some(b))].into();

        let strict = probe(&git, &base, &heads, &crate::git::nothing_regenerable).unwrap();
        assert_eq!(strict.conflicts.len(), 1, "{strict:?}");

        // An exact-path pattern and a directory pattern without its trailing
        // slash both cover the file, so neither records a conflict.
        for paths in [
            vec!["generated/output.txt"],
            vec!["generated"],
            vec!["generated/"],
        ] {
            let config = regen_config(&paths);
            let lenient = probe(&git, &base, &heads, &|path| config.covers(path)).unwrap();
            assert!(lenient.conflicts.is_empty(), "{paths:?}: {lenient:?}");
            assert!(lenient.base_conflicts.is_empty(), "{paths:?}: {lenient:?}");
        }
    }

    #[test]
    fn probe_base_conflicts_on_regenerated_paths_are_filtered() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe-regen-base");
        let git = init(&tmp.0);
        write(&tmp.0, "generated/output.txt", "1\n2\n3\n");
        write(&tmp.0, "real.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        let old_base = git.rev_parse("main").unwrap();
        let only = branch(
            &tmp.0,
            "only",
            "main",
            &[("generated/output.txt", "P\n2\n3\n")],
        );
        let mixed = branch(
            &tmp.0,
            "mixed",
            "main",
            &[
                ("generated/output.txt", "P\n2\n3\n"),
                ("real.txt", "P\n2\n3\n"),
            ],
        );
        let _ = old_base;
        // The base advances, rewriting the same lines of both files.
        write(&tmp.0, "generated/output.txt", "Z\n2\n3\n");
        write(&tmp.0, "real.txt", "Z\n2\n3\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let config = regen_config(&["generated/"]);

        // Confined to the regenerated file: not a base conflict.
        let heads: BTreeMap<i64, Option<String>> = [(1, Some(only))].into();
        let probed = probe(&git, &base, &heads, &|path| config.covers(path)).unwrap();
        assert!(probed.base_conflicts.is_empty(), "{probed:?}");
        let raw = probe(&git, &base, &heads, &crate::git::nothing_regenerable).unwrap();
        assert_eq!(
            raw.base_conflicts[&1],
            vec!["generated/output.txt".to_owned()]
        );

        // Mixed with a real path: still a base conflict, recorded with only
        // its real paths.
        let heads: BTreeMap<i64, Option<String>> = [(2, Some(mixed))].into();
        let probed = probe(&git, &base, &heads, &|path| config.covers(path)).unwrap();
        assert_eq!(
            probed.base_conflicts[&2],
            vec!["real.txt".to_owned()],
            "{probed:?}"
        );
    }

    #[test]
    fn probe_base_verdict_follows_combine_mode_and_skips_exact() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe-rebase-base");
        let git = init(&tmp.0);
        write(&tmp.0, "f.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        // The bottom change appends A; the stacked change appends B after it.
        branch(&tmp.0, "a", "main", &[("f.txt", "1\n2\n3\nA\n")]);
        let stacked = branch(&tmp.0, "b", "a", &[("f.txt", "1\n2\n3\nA\nB\n")]);
        // A real conflict: appends X where the landed change appends A.
        let real = branch(&tmp.0, "c", "main", &[("f.txt", "1\n2\n3\nX\n")]);
        // The bottom change lands as a rebased commit: same patch, new parent.
        write(&tmp.0, "other.txt", "o\n");
        git_commit(&tmp.0);
        write(&tmp.0, "f.txt", "1\n2\n3\nA\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let heads: BTreeMap<i64, Option<String>> = [(2, Some(stacked)), (3, Some(real))].into();

        // Merge combine: plain merge-tree holds both (the reported bug).
        let merged = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            ProbeOptions::sequential_uncached(),
        )
        .unwrap();
        assert_eq!(
            merged.base_conflicts[&2],
            vec!["f.txt".to_owned()],
            "{merged:?}"
        );
        assert_eq!(
            merged.base_conflicts[&3],
            vec!["f.txt".to_owned()],
            "{merged:?}"
        );

        // Rebase combine: the replay drops the already-landed patch, so the
        // stacked change is clean; the real conflict still conflicts.
        let rebased = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                combine: CombineMode::Rebase,
                ..ProbeOptions::sequential_uncached()
            },
        )
        .unwrap();
        assert!(!rebased.base_conflicts.contains_key(&2), "{rebased:?}");
        assert_eq!(
            rebased.base_conflicts[&3],
            vec!["f.txt".to_owned()],
            "{rebased:?}"
        );

        // Exact: never merge-probed, whatever the mode (build's lands_exactly
        // decides), and the cached non-exact verdict must not answer for it.
        let cache_tmp = TempDir::new("probe-exact-cache");
        let cached_merge = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            config_hash: Some("cfg".to_owned()),
            ..ProbeOptions::sequential_uncached()
        };
        let first = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            cached_merge.clone(),
        )
        .unwrap();
        assert_eq!(
            first.base_conflicts[&3],
            vec!["f.txt".to_owned()],
            "{first:?}"
        );
        let exact = probe_with_options(
            &git,
            &base,
            &heads,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                exact: BTreeSet::from([3]),
                ..cached_merge
            },
        )
        .unwrap();
        assert!(!exact.base_conflicts.contains_key(&3), "{exact:?}");
    }

    /// A fixture queue: conflicting, clean and stacked branches over one base.
    /// Returns the repository, its git handle, the base and `(pr, head)` pairs.
    fn fixture_queue(
        tmp: &crate::git::testrepo::TempDir,
        count: i64,
    ) -> (crate::git::Git, String, Vec<(i64, String)>) {
        use crate::git::testrepo::*;
        let git = init(&tmp.0);
        write(&tmp.0, "shared.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let mut heads = Vec::new();
        for pr in 1..=count {
            // Every branch rewrites line 1 of shared.txt differently (mutual conflicts)
            // and adds its own file.
            let sha = branch(
                &tmp.0,
                &format!("b{pr}"),
                "main",
                &[
                    ("shared.txt", &format!("v{pr}\n2\n3\n")),
                    (&format!("own{pr}.txt"), "x\n"),
                ],
            );
            heads.push((pr, sha));
        }
        // One stacked branch on the first head.
        let stacked = branch(&tmp.0, "stacked", "b1", &[("stacked.txt", "s\n")]);
        heads.push((count + 1, stacked));
        (git, base, heads)
    }

    fn heads_map(heads: &[(i64, String)]) -> BTreeMap<i64, Option<String>> {
        heads
            .iter()
            .map(|(pr, sha)| (*pr, Some(sha.clone())))
            .collect()
    }

    fn assert_same_probe(a: &Probe, b: &Probe) {
        assert_eq!(a.heads, b.heads);
        assert_eq!(a.files, b.files);
        assert_eq!(a.base_conflicts, b.base_conflicts);
        assert_eq!(a.conflicts, b.conflicts);
        assert_eq!(a.ancestry, b.ancestry);
    }

    #[test]
    fn cached_parallel_plan_equals_sequential_uncached() {
        use crate::git::testrepo::TempDir;
        let tmp = TempDir::new("probe-equiv-repo");
        let cache_tmp = TempDir::new("probe-equiv-cache");
        let (git, base, heads) = fixture_queue(&tmp, 8);
        let map = heads_map(&heads);
        let sequential = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions::sequential_uncached(),
        )
        .unwrap();
        let parallel_cached = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                cache_dir: Some(cache_tmp.0.clone()),
                parallelism: 8,
                config_hash: None,
                peripheral: None,
                footprints: None,
                combine: CombineMode::Merge,
                exact: BTreeSet::new(),
            },
        )
        .unwrap();
        assert_same_probe(&sequential, &parallel_cached);
        assert!(parallel_cached.stats.cache_misses > 0);
        assert!(parallel_cached.stats.peak_concurrency > 1);
        assert!(parallel_cached.stats.peak_concurrency <= 8);

        // Same probes, same plans (created_at is the only run-dependent field).
        let items: Vec<Item> = heads
            .iter()
            .map(|(pr, sha)| {
                let mut entry = item(*pr, 100, "t1");
                entry.head = sha.clone();
                entry
            })
            .collect();
        let mut plan_a = make_plan(&items, "main", &base, &sequential, opts(4));
        let plan_b = make_plan(&items, "main", &base, &parallel_cached, opts(4));
        plan_a.created_at = plan_b.created_at.clone();
        assert_eq!(plan_a, plan_b);
    }

    #[test]
    fn cache_hit_skips_git_and_persists_across_loads() {
        use crate::git::testrepo::TempDir;
        let tmp = TempDir::new("probe-hit-repo");
        let cache_tmp = TempDir::new("probe-hit-cache");
        let (git, base, heads) = fixture_queue(&tmp, 6);
        let map = heads_map(&heads);
        let options = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            parallelism: 8,
            config_hash: None,
            peripheral: None,
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        };
        let first = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            options.clone(),
        )
        .unwrap();
        assert!(first.stats.cache_misses > 0, "{:?}", first.stats);

        // A fresh probe loads the cache from disk (a new process would do the same) and
        // needs no repository at all: a git handle pointing nowhere proves hits ran no
        // subprocess.
        let nowhere = crate::git::Git::new("/nonexistent/probe-cache-test");
        let second = probe_with_options(
            &nowhere,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            options,
        )
        .unwrap();
        assert_same_probe(&first, &second);
        assert_eq!(second.stats.cache_misses, 0, "{:?}", second.stats);
        assert!(second.stats.cache_hits > 0);
    }

    #[test]
    fn cache_misses_on_head_and_config_change() {
        use crate::git::testrepo::{branch, TempDir};
        let tmp = TempDir::new("probe-inval-repo");
        let cache_tmp = TempDir::new("probe-inval-cache");
        let (git, base, heads) = fixture_queue(&tmp, 5);
        let map = heads_map(&heads);
        let cached = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            parallelism: 4,
            config_hash: Some("cfg-a".to_owned()),
            peripheral: None,
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        };
        let first = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            cached.clone(),
        )
        .unwrap();
        assert!(first.stats.cache_misses > 0);

        // Same heads, different merge configuration: everything misses again.
        let changed_config = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                config_hash: Some("cfg-b".to_owned()),
                ..cached.clone()
            },
        )
        .unwrap();
        assert_same_probe(&first, &changed_config);
        assert!(changed_config.stats.cache_misses > 0);
        assert_eq!(
            changed_config.stats.cache_hits, 0,
            "{:?}",
            changed_config.stats
        );

        // A moved head is a new key: its base and pair probes miss, the rest still hit.
        let moved = branch(&tmp.0, "moved", "main", &[("moved.txt", "m\n")]);
        let mut moved_map = map.clone();
        moved_map.insert(1, Some(moved));
        let after_move = probe_with_options(
            &git,
            &base,
            &moved_map,
            &crate::git::nothing_regenerable,
            cached,
        )
        .unwrap();
        assert!(after_move.stats.cache_misses > 0, "{:?}", after_move.stats);
        assert!(after_move.stats.cache_hits > 0, "{:?}", after_move.stats);
    }

    #[test]
    fn parallel_probe_is_deterministic() {
        use crate::git::testrepo::TempDir;
        let tmp = TempDir::new("probe-det-repo");
        let (git, base, heads) = fixture_queue(&tmp, 10);
        let map = heads_map(&heads);
        let reference = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions::sequential_uncached(),
        )
        .unwrap();
        assert!(!reference.conflicts.is_empty());
        for _ in 0..5 {
            let parallel = probe_with_options(
                &git,
                &base,
                &map,
                &crate::git::nothing_regenerable,
                ProbeOptions {
                    cache_dir: None,
                    parallelism: 8,
                    config_hash: None,
                    peripheral: None,
                    footprints: None,
                    combine: CombineMode::Merge,
                    exact: BTreeSet::new(),
                },
            )
            .unwrap();
            assert_same_probe(&reference, &parallel);
            assert!(parallel.stats.peak_concurrency <= 8);
        }
    }

    #[test]
    fn staged_first_stage_decides_every_pair_and_finish_is_all_hits() {
        use crate::git::testrepo::TempDir;
        let tmp = TempDir::new("probe-staged-repo");
        let cache_tmp = TempDir::new("probe-staged-cache");
        let (git, base, heads) = fixture_queue(&tmp, 8);
        let map = heads_map(&heads);
        let options = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            parallelism: 8,
            config_hash: None,
            peripheral: None,
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        };
        // The first stage leaves no pair unknown: an optimistic first batch could pair two
        // conflicting changes (ejecting one the plan would only hold) or a change stacked on a
        // held one.
        let first = probe_first_stage(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            options.clone(),
        )
        .unwrap();
        assert_eq!(first.stats.pairs_unknown, 0, "{:?}", first.stats);
        let items: Vec<Item> = heads
            .iter()
            .map(|(pr, sha)| {
                let mut entry = item(*pr, 100, "t1");
                entry.head = sha.clone();
                entry
            })
            .collect();
        let sequential = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions::sequential_uncached(),
        )
        .unwrap();
        assert_same_probe(&first, &sequential);
        let mut partial_plan = make_plan(&items, "main", &base, &first, opts(4));
        let one_shot_plan = make_plan(&items, "main", &base, &sequential, opts(4));
        partial_plan.created_at = one_shot_plan.created_at.clone();
        assert_eq!(partial_plan, one_shot_plan);
        // Finishing is all cache hits.
        let full = probe_with_options(&git, &base, &map, &crate::git::nothing_regenerable, options)
            .unwrap();
        assert_eq!(full.stats.base_misses, 0, "{:?}", full.stats);
        assert_eq!(full.stats.pair_misses, 0, "{:?}", full.stats);
        assert_same_probe(&full, &sequential);
    }

    #[test]
    fn base_move_reprobes_only_base_entries() {
        use crate::git::testrepo::{git as tgit, write, TempDir};
        let tmp = TempDir::new("probe-basemove-repo");
        let cache_tmp = TempDir::new("probe-basemove-cache");
        let (git, base, heads) = fixture_queue(&tmp, 6);
        let map = heads_map(&heads);
        let options = ProbeOptions {
            cache_dir: Some(cache_tmp.0.clone()),
            parallelism: 4,
            config_hash: None,
            peripheral: None,
            footprints: None,
            combine: CombineMode::Merge,
            exact: BTreeSet::new(),
        };
        let warm = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            options.clone(),
        )
        .unwrap();
        assert!(warm.stats.cache_misses > 0, "{:?}", warm.stats);

        // Move the base by one unrelated commit; every head stays put.
        tgit(&tmp.0, &["checkout", "-q", "main"]);
        write(&tmp.0, "base2.txt", "new base\n");
        tgit(&tmp.0, &["add", "-A"]);
        tgit(&tmp.0, &["commit", "-q", "-m", "move base"]);
        let new_base = git.rev_parse("main").unwrap();
        assert_ne!(new_base, base);

        // Incremental re-plan: pairwise keys do not name the base, so all
        // pairs hit; only the per-head base entries re-probe.
        let moved = probe_with_options(
            &git,
            &new_base,
            &map,
            &crate::git::nothing_regenerable,
            options,
        )
        .unwrap();
        assert_eq!(moved.stats.base_misses, heads.len(), "{:?}", moved.stats);
        assert_eq!(moved.stats.base_hits, 0, "{:?}", moved.stats);
        assert_eq!(moved.stats.pair_misses, 0, "{:?}", moved.stats);
        assert!(moved.stats.pair_hits > 0, "{:?}", moved.stats);
        assert_eq!(moved.stats.pairs_unknown, 0, "{:?}", moved.stats);
    }

    /// Wall-clock comparison on a wide fixture; run explicitly with `--ignored --nocapture`.
    #[test]
    #[ignore = "timing evidence, not a gate"]
    fn probe_speedup_evidence() {
        use crate::git::testrepo::TempDir;
        let tmp = TempDir::new("probe-speed-repo");
        let cache_tmp = TempDir::new("probe-speed-cache");
        let (git, base, heads) = fixture_queue(&tmp, 24);
        let map = heads_map(&heads);
        let pairs = heads.len() * (heads.len() - 1) / 2;
        let start = std::time::Instant::now();
        let sequential = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions::sequential_uncached(),
        )
        .unwrap();
        let sequential_wall = start.elapsed();
        let start = std::time::Instant::now();
        let parallel = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                cache_dir: Some(cache_tmp.0.clone()),
                parallelism: 32,
                config_hash: None,
                peripheral: None,
                footprints: None,
                combine: CombineMode::Merge,
                exact: BTreeSet::new(),
            },
        )
        .unwrap();
        let parallel_wall = start.elapsed();
        let start = std::time::Instant::now();
        let cached = probe_with_options(
            &git,
            &base,
            &map,
            &crate::git::nothing_regenerable,
            ProbeOptions {
                cache_dir: Some(cache_tmp.0.clone()),
                parallelism: 32,
                config_hash: None,
                peripheral: None,
                footprints: None,
                combine: CombineMode::Merge,
                exact: BTreeSet::new(),
            },
        )
        .unwrap();
        let cached_wall = start.elapsed();
        assert_same_probe(&sequential, &parallel);
        assert_same_probe(&sequential, &cached);
        println!(
            "probe speedup: heads={} pairs={} sequential={sequential_wall:?} \
             parallel32={parallel_wall:?} (peak {}) cached={cached_wall:?} (hits {})",
            heads.len(),
            pairs,
            parallel.stats.peak_concurrency,
            cached.stats.cache_hits
        );
    }

    /// Before/after for pipelined planning on one fixture: the one-shot
    /// probe (today's `run`, which validates nothing until it returns)
    /// against first-stage plus finish. Run explicitly with
    /// `--ignored --nocapture`; the harness reports medians, the test only
    /// checks the two paths agree and the first stage is sooner.
    #[test]
    #[ignore = "timing evidence, not a gate"]
    fn staged_pipeline_timing_evidence() {
        use crate::git::testrepo::TempDir;
        fn cpu_ticks() -> u64 {
            // utime + stime from /proc/self/stat (fields after the comm).
            let text = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
            let after = text.rsplit(')').next().unwrap_or_default();
            let fields: Vec<&str> = after.split_whitespace().collect();
            // fields[0] is state (overall field 3), so utime/stime are at 11/12.
            let at = |index: usize| {
                fields
                    .get(index)
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_default()
            };
            at(11) + at(12)
        }
        fn median(mut values: Vec<std::time::Duration>) -> std::time::Duration {
            values.sort();
            values[values.len() / 2]
        }
        let tmp = TempDir::new("probe-stage-time-repo");
        let (git, base, heads) = fixture_queue(&tmp, 32);
        let map = heads_map(&heads);
        let pairs = heads.len() * (heads.len() - 1) / 2;
        let mut before_full = Vec::new();
        let mut before_cpu = Vec::new();
        let mut after_first = Vec::new();
        let mut after_total = Vec::new();
        let mut after_cpu = Vec::new();
        for rep in 0..3 {
            let before_cache = TempDir::new(&format!("stage-before-{rep}"));
            let cpu0 = cpu_ticks();
            let start = std::time::Instant::now();
            let before = probe_with_options(
                &git,
                &base,
                &map,
                &crate::git::nothing_regenerable,
                ProbeOptions {
                    cache_dir: Some(before_cache.0.clone()),
                    parallelism: 32,
                    config_hash: None,
                    peripheral: None,
                    footprints: None,
                    combine: CombineMode::Merge,
                    exact: BTreeSet::new(),
                },
            )
            .unwrap();
            before_full.push(start.elapsed());
            before_cpu.push(cpu_ticks().saturating_sub(cpu0));

            let after_cache = TempDir::new(&format!("stage-after-{rep}"));
            let options = ProbeOptions {
                cache_dir: Some(after_cache.0.clone()),
                parallelism: 32,
                config_hash: None,
                peripheral: None,
                footprints: None,
                combine: CombineMode::Merge,
                exact: BTreeSet::new(),
            };
            let cpu0 = cpu_ticks();
            let start = std::time::Instant::now();
            let first = probe_first_stage(
                &git,
                &base,
                &map,
                &crate::git::nothing_regenerable,
                options.clone(),
            )
            .unwrap();
            after_first.push(start.elapsed());
            assert_eq!(first.stats.pair_misses, 0, "{:?}", first.stats);
            let full =
                probe_with_options(&git, &base, &map, &crate::git::nothing_regenerable, options)
                    .unwrap();
            after_total.push(start.elapsed());
            after_cpu.push(cpu_ticks().saturating_sub(cpu0));
            assert_same_probe(&before, &full);
        }
        println!(
            "staged timing: heads={} pairs={} before_full_wall={:?} before_cpu_ticks={:?} \
             after_first_wall={:?} after_full_wall={:?} after_cpu_ticks={:?} (median of 3, cold cache each rep)",
            heads.len(),
            pairs,
            median(before_full.clone()),
            {
                let mut values = before_cpu.clone();
                values.sort_unstable();
                values[values.len() / 2]
            },
            median(after_first.clone()),
            median(after_total),
            {
                let mut values = after_cpu.clone();
                values.sort_unstable();
                values[values.len() / 2]
            },
        );
        assert!(
            median(after_first) < median(before_full.clone()),
            "first stage must beat the one-shot probe to first batch"
        );
    }
}
