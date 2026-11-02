//! Order and chunk queued changes into batches.
//!
//! The plan is bound to one exact base commit and one exact head per change. It is built from
//! three kinds of evidence, all probed locally with `git merge-tree` (the landing planner's
//! exact-conflict method): conflicts with the base, pairwise conflicts between queued heads, and
//! ancestry (a change stacked on another must land after it).
//!
//! Without repair, two changes that textually conflict cannot both land this round: once one
//! lands, the other conflicts with the base and needs a rebase. The plan therefore picks a
//! conflict-free subset. Priority decides first; among equal priorities the change with fewer
//! conflicts wins (a greedy maximum-independent-set heuristic, which lands more changes per round
//! than first-come-first-served when one change conflicts with many), then submission time.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use pr_landing_planner::graph::connected_components;
use pr_landing_planner::model::ConflictEdge;

use crate::git::{Git, Regenerable};
use crate::model::{
    now, Conflict, Held, Item, ItemStatus, Plan, PlannedBatch, PlannedItem, PLAN_SCHEMA,
};
use crate::probe_cache::{probe_config_hash, BaseEntry, PairAncestry, PairEntry, ProbeCache};
use crate::state::DEFAULT_PROBE_PARALLELISM;

/// How one probing run used its cache and its concurrency bound.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProbeStats {
    /// Lookups answered wholly from the probe cache (no git subprocess ran for them).
    pub cache_hits: usize,
    /// Lookups that ran at least one git subprocess.
    pub cache_misses: usize,
    /// Highest number of probe jobs observed running at once; never above the bound.
    pub peak_concurrency: usize,
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
    /// Cache and concurrency accounting for the run that produced this probe. Planning
    /// ignores it; it exists so callers can report what probing actually cost.
    pub stats: ProbeStats,
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
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            parallelism: DEFAULT_PROBE_PARALLELISM,
            config_hash: None,
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

/// One pairwise job's raw outcome, before regenerable filtering and ordering.
enum PairOutcome {
    /// `(before, after)` change numbers.
    Ancestor(i64, i64),
    /// Raw conflicting paths between the two heads.
    Conflicts(Vec<String>),
    /// No ancestry and no conflict.
    Clean,
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
    let real = |paths: Vec<String>| -> Vec<String> {
        if paths.iter().all(|path| regenerable(path)) {
            Vec::new()
        } else {
            paths
        }
    };
    let config_hash = options
        .config_hash
        .clone()
        .unwrap_or_else(|| probe_config_hash(git, ""));
    let cache = Mutex::new(match &options.cache_dir {
        Some(dir) => ProbeCache::load(dir),
        None => ProbeCache::ephemeral(),
    });
    let hits = AtomicUsize::new(0);
    let misses = AtomicUsize::new(0);
    let mut probe = Probe {
        heads: heads.clone(),
        ..Probe::default()
    };
    let live: Vec<(i64, &str)> = heads
        .iter()
        .filter_map(|(pr, head)| head.as_deref().map(|head| (*pr, head)))
        .collect();

    // Phase 1: one base probe per live head. Jobs are independent.
    let (base_results, base_peak) = run_bounded(live.len(), options.parallelism, |index| {
        let head = live[index].1;
        let key = ProbeCache::base_key(&config_hash, base_sha, head);
        if let Some(entry) = cache.lock().unwrap().base(&key).cloned() {
            hits.fetch_add(1, Ordering::SeqCst);
            let files: BTreeSet<String> = entry.files.into_iter().collect();
            return Ok((files, entry.conflicts));
        }
        misses.fetch_add(1, Ordering::SeqCst);
        let files = git.changed_files(base_sha, head)?;
        let conflicts = git.merge_conflicts(base_sha, head)?;
        cache.lock().unwrap().put_base(
            key,
            BaseEntry {
                files: files.iter().cloned().collect(),
                conflicts: conflicts.clone(),
            },
        );
        Ok((files, conflicts))
    });
    for ((pr, _), result) in live.iter().zip(base_results) {
        let (files, raw) = result?;
        probe.files.insert(*pr, files);
        let paths = real(raw);
        if !paths.is_empty() {
            probe.base_conflicts.insert(*pr, paths);
        }
    }

    // Phase 2: one job per pair of distinct heads.
    let mut pairs: Vec<(i64, i64, String, String, bool)> = Vec::new();
    for (i, (a, head_a)) in live.iter().enumerate() {
        for (b, head_b) in &live[i + 1..] {
            if head_a == head_b {
                continue;
            }
            let disjoint = probe.files[a].is_disjoint(&probe.files[b]);
            pairs.push((*a, *b, (*head_a).to_owned(), (*head_b).to_owned(), disjoint));
        }
    }
    let (pair_results, pair_peak) = run_bounded(pairs.len(), options.parallelism, |index| {
        let (a, b, head_a, head_b, disjoint) = &pairs[index];
        let (a, b) = (*a, *b);
        let (key, lo, _) = ProbeCache::pair_key(&config_hash, head_a, head_b);
        let lo_is_a = lo == *head_a;
        let entry = cache.lock().unwrap().pair(&key).cloned();
        if let Some(entry) = &entry {
            match entry.ancestry {
                PairAncestry::LoFirst => {
                    hits.fetch_add(1, Ordering::SeqCst);
                    return Ok(if lo_is_a {
                        PairOutcome::Ancestor(a, b)
                    } else {
                        PairOutcome::Ancestor(b, a)
                    });
                }
                PairAncestry::HiFirst => {
                    hits.fetch_add(1, Ordering::SeqCst);
                    return Ok(if lo_is_a {
                        PairOutcome::Ancestor(b, a)
                    } else {
                        PairOutcome::Ancestor(a, b)
                    });
                }
                PairAncestry::None => {
                    if let Some(conflicts) = &entry.conflicts {
                        hits.fetch_add(1, Ordering::SeqCst);
                        return Ok(if conflicts.is_empty() {
                            PairOutcome::Clean
                        } else {
                            PairOutcome::Conflicts(conflicts.clone())
                        });
                    }
                    // Ancestry is known (neither); only conflicts may be missing, and
                    // only a non-disjoint pair needs them.
                    if *disjoint {
                        hits.fetch_add(1, Ordering::SeqCst);
                        return Ok(PairOutcome::Clean);
                    }
                }
            }
        }
        misses.fetch_add(1, Ordering::SeqCst);
        if entry.is_none() {
            if git.is_ancestor(head_a, head_b)? {
                cache.lock().unwrap().put_pair(
                    key,
                    PairEntry {
                        ancestry: if lo_is_a {
                            PairAncestry::LoFirst
                        } else {
                            PairAncestry::HiFirst
                        },
                        conflicts: None,
                    },
                );
                return Ok(PairOutcome::Ancestor(a, b));
            }
            if git.is_ancestor(head_b, head_a)? {
                cache.lock().unwrap().put_pair(
                    key,
                    PairEntry {
                        ancestry: if lo_is_a {
                            PairAncestry::HiFirst
                        } else {
                            PairAncestry::LoFirst
                        },
                        conflicts: None,
                    },
                );
                return Ok(PairOutcome::Ancestor(b, a));
            }
            if *disjoint {
                cache.lock().unwrap().put_pair(
                    key,
                    PairEntry {
                        ancestry: PairAncestry::None,
                        conflicts: None,
                    },
                );
                return Ok(PairOutcome::Clean);
            }
        }
        let conflicts = git.merge_conflicts(head_a, head_b)?;
        cache.lock().unwrap().put_pair(
            key,
            PairEntry {
                ancestry: PairAncestry::None,
                conflicts: Some(conflicts.clone()),
            },
        );
        Ok(if conflicts.is_empty() {
            PairOutcome::Clean
        } else {
            PairOutcome::Conflicts(conflicts)
        })
    });
    for ((a, b, _, _, _), result) in pairs.iter().zip(pair_results) {
        match result? {
            PairOutcome::Ancestor(before, after) => probe.ancestry.push((before, after)),
            PairOutcome::Conflicts(raw) => {
                let paths = real(raw);
                if !paths.is_empty() {
                    probe.conflicts.push(Conflict {
                        a: (*a).min(*b),
                        b: (*a).max(*b),
                        paths,
                    });
                }
            }
            PairOutcome::Clean => {}
        }
    }
    probe
        .conflicts
        .sort_by_key(|conflict| (conflict.a, conflict.b));
    probe.ancestry.sort();
    probe.stats = ProbeStats {
        cache_hits: hits.load(Ordering::SeqCst),
        cache_misses: misses.load(Ordering::SeqCst),
        peak_concurrency: base_peak.max(pair_peak),
    };
    // The cache is an optimisation: a failed write must not fail planning.
    if let Ok(mut cache) = cache.into_inner() {
        let _ = cache.save();
    }
    Ok(probe)
}

/// Planning knobs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanOptions {
    /// Maximum changes per batch (at least 1).
    pub max_batch: usize,
}

type Rank<'a> = (i64, usize, &'a str, i64);

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
    let rank = |pr: i64| -> Rank<'_> {
        let item = by_pr[&pr];
        (item.priority, degrees[&pr], item.submitted_at.as_str(), pr)
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
    for pr in &selected {
        if by_pr[pr].solo || by_pr[pr].exact {
            close(&mut current, &mut batches, "");
            current.push(*pr);
            close(
                &mut current,
                &mut batches,
                if by_pr[pr].exact { "exact" } else { "solo" },
            );
            continue;
        }
        current.push(*pr);
        if current.len() >= max_batch {
            close(&mut current, &mut batches, "max-batch");
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
        })
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
                format!("#{pr} (p{}, {} files)", item.priority, item.files)
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
            title: String::new(),
            after: vec![],
            solo: false,
            claim: None,
            exact: false,
            requires: vec![],
            source: String::new(),
            close_pending: false,
            validation_refusals: 0,
            next_retry_at: String::new(),
            last_refusal: String::new(),
            status: ItemStatus::Queued,
            landed_as: String::new(),
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
        PlanOptions { max_batch }
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

    fn git_commit(dir: &std::path::Path) {
        use crate::git::testrepo::git;
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "more"]);
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
        assert_eq!(changed_config.stats.cache_hits, 0);

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
                },
            )
            .unwrap();
            assert_same_probe(&reference, &parallel);
            assert!(parallel.stats.peak_concurrency <= 8);
        }
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
}
