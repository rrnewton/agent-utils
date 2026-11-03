//! Queue operations shared by the command line and tests: refresh, submit, plan.

use std::collections::BTreeMap;

use crate::git::Git;
use crate::model::{now, Held, Item, ItemStatus, Plan, Requirement};
use crate::plan::{make_plan, probe_first_stage, probe_with_options, PlanOptions, ProbeOptions};
use crate::probe_cache::probe_config_hash;
use crate::state::RegenerateConfig;
use crate::state::{Config, State};
use crate::tests::ValidationClaim;

/// Private ref holding the fetched base.
pub const BASE_REF: &str = "refs/merge-queue/base";

/// Private ref holding one fetched change head.
pub fn head_ref(pr: i64) -> String {
    format!("refs/merge-queue/heads/{pr}")
}

/// The repository named by a configuration, with its network wrapper and, when `union_paths`
/// is set, an attributes file (kept in the state directory) selecting Git's union merge driver
/// for those paths.
pub fn repo(state: &State, config: &Config) -> Result<Git, String> {
    let mut git = Git::new(&config.repo);
    git.net_wrapper = config.net_wrapper.clone();
    git.anonymous_fetch = config.github.anonymous_fetch;
    if !config.identity.name.is_empty() && !config.identity.email.is_empty() {
        git.identity = Some((config.identity.name.clone(), config.identity.email.clone()));
    }
    if !config.union_paths.is_empty() {
        let path = state.root.join("union.attributes");
        let text: String = config
            .union_paths
            .iter()
            .map(|pattern| format!("{pattern} merge=union\n"))
            .collect();
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::create_dir_all(&state.root)
                .map_err(|error| format!("cannot create {}: {error}", state.root.display()))?;
            std::fs::write(&path, text)
                .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        }
        git.settings
            .push(format!("core.attributesFile={}", path.display()));
    }
    Ok(git)
}

fn source_ref(head: &str) -> String {
    if head.starts_with("refs/") {
        head.to_owned()
    } else {
        format!("refs/heads/{head}")
    }
}

/// Fetch (or, for a local queue, resolve) the base and the given heads in one pass.
/// Returns the base commit and each change's current head (`None` when it no longer resolves).
pub fn refresh(
    git: &Git,
    config: &Config,
    heads: &[(i64, String)],
) -> Result<(String, BTreeMap<i64, Option<String>>), String> {
    let mut resolved = BTreeMap::new();
    if config.remote.is_empty() {
        let base = git.rev_parse(&source_ref(&config.base))?;
        for (pr, head) in heads {
            resolved.insert(*pr, git.try_rev_parse(&source_ref(head)));
        }
        return Ok((base, resolved));
    }
    let base_spec = format!("+{}:{BASE_REF}", source_ref(&config.base));
    let head_specs: Vec<String> = heads
        .iter()
        .map(|(pr, head)| format!("+{}:{}", source_ref(head), head_ref(*pr)))
        .collect();
    let mut args = vec!["fetch", "--quiet", "--no-tags", "--no-write-fetch-head"];
    args.push(&config.remote);
    args.push(&base_spec);
    args.extend(head_specs.iter().map(String::as_str));
    if git.run_net(&args).is_err() {
        // One missing head fails the whole fetch; retry one ref at a time so the rest resolve.
        git.run_net(&[
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            &config.remote,
            &base_spec,
        ])?;
        for ((pr, _), spec) in heads.iter().zip(&head_specs) {
            let fetched = git.run_net(&[
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-write-fetch-head",
                &config.remote,
                spec,
            ]);
            if fetched.is_err() {
                let _ = git.delete_ref(&head_ref(*pr));
            }
        }
    }
    let base = git.rev_parse(BASE_REF)?;
    for (pr, _) in heads {
        resolved.insert(*pr, git.try_rev_parse(&head_ref(*pr)));
    }
    Ok((base, resolved))
}

/// What to record for a submission.
#[derive(Clone, Debug, Default)]
pub struct Submission {
    /// Change number.
    pub pr: i64,
    /// Head branch or ref.
    pub head_ref: String,
    /// Exact head the submitter means; resolved from `head_ref` when empty.
    pub head: String,
    /// Priority (lower lands first).
    pub priority: i64,
    /// Submitter.
    pub by: String,
    /// agentctl session name to notify on state changes; empty keeps any
    /// name a previous submission recorded.
    pub notify: String,
    /// One-line description.
    pub title: String,
    /// Changes that must land first.
    pub after: Vec<i64>,
    /// Land alone.
    pub solo: bool,
    /// Submitter's validation claim.
    pub claim: Option<ValidationClaim>,
    /// `sync` source that produced this submission; empty for a direct `submit`.
    pub source: String,
    /// Peer commits that must land first.
    pub requires: Vec<Requirement>,
}

/// Submit (or resubmit) a change. Returns the stored item.
pub fn submit(state: &State, config: &Config, submission: Submission) -> Result<Item, String> {
    submit_many(state, config, vec![submission])?
        .pop()
        .ok_or_else(|| "nothing submitted".to_owned())?
}

/// Submit several changes with one fetch for all their heads. Each result is the stored item or
/// why that one submission was refused.
pub fn submit_many(
    state: &State,
    config: &Config,
    submissions: Vec<Submission>,
) -> Result<Vec<Result<Item, String>>, String> {
    let git = repo(state, config)?;
    let wanted: Vec<(i64, String)> = submissions
        .iter()
        .filter(|s| s.pr > 0 && !s.head_ref.is_empty())
        .map(|s| (s.pr, s.head_ref.clone()))
        .collect();
    let (_, heads) = refresh(&git, config, &wanted)?;
    Ok(submissions
        .into_iter()
        .map(|submission| {
            let current = heads.get(&submission.pr).cloned().flatten();
            submit_resolved(state, submission, current)
        })
        .collect())
}

fn submit_resolved(
    state: &State,
    submission: Submission,
    current: Option<String>,
) -> Result<Item, String> {
    if submission.pr <= 0 {
        return Err(format!(
            "change number must be positive, got {}",
            submission.pr
        ));
    }
    if submission.head_ref.is_empty() {
        return Err("a head branch or ref is required".to_owned());
    }
    let current = current.ok_or_else(|| {
        format!(
            "head {:?} of #{} does not resolve",
            submission.head_ref, submission.pr
        )
    })?;
    if !submission.head.is_empty() && !current.starts_with(&submission.head) {
        return Err(format!(
            "#{}: {:?} is at {current}, not the submitted {}",
            submission.pr, submission.head_ref, submission.head
        ));
    }
    let existing = state.item(submission.pr)?;
    // A notification name is a subscription: a resubmission that does not
    // name one keeps the recorded name and its delivery record.
    let notify = if submission.notify.is_empty() {
        existing
            .as_ref()
            .map_or_else(String::new, |old| old.notify.clone())
    } else {
        submission.notify.clone()
    };
    let notify_delivery = existing
        .as_ref()
        .map_or_else(Default::default, |old| old.notify_delivery.clone());
    // A resubmission at a new head is a new validation attempt: clear any refusal deferral.
    // A resubmission at the same head keeps the count so backoff is not reset by resubmitting.
    let same_head = existing.as_ref().is_some_and(|old| old.head == current);
    let mut item = Item {
        pr: submission.pr,
        head_ref: submission.head_ref,
        head: current.clone(),
        priority: submission.priority,
        submitted_at: now(),
        submitted_by: submission.by,
        notify,
        notify_delivery,
        title: submission.title,
        after: submission.after,
        solo: submission.solo,
        claim: submission.claim,
        exact: existing.as_ref().is_some_and(|old| old.exact),
        requires: submission.requires,
        source: submission.source,
        close_pending: false,
        validation_refusals: if same_head {
            existing.as_ref().map_or(0, |old| old.validation_refusals)
        } else {
            0
        },
        next_retry_at: if same_head {
            existing
                .as_ref()
                .map_or_else(String::new, |old| old.next_retry_at.clone())
        } else {
            String::new()
        },
        escalated: if same_head {
            existing
                .as_ref()
                .map_or_else(String::new, |old| old.escalated.clone())
        } else {
            String::new()
        },
        last_refusal: if same_head {
            existing
                .as_ref()
                .map_or_else(String::new, |old| old.last_refusal.clone())
        } else {
            String::new()
        },
        deciding_report: existing
            .as_ref()
            .map_or_else(String::new, |old| old.deciding_report.clone()),
        deciding_failed: existing
            .as_ref()
            .map_or_else(Vec::new, |old| old.deciding_failed.clone()),
        status: ItemStatus::Queued,
        landed_as: String::new(),
        history: existing.map(|old| old.history).unwrap_or_default(),
    };
    let detail = match &item.claim {
        Some(claim) => format!(
            "head {current}; submitter validated {} ({}, policy {})",
            claim.selection.summary(),
            claim.scope.as_str(),
            claim.policy
        ),
        None => format!("head {current}; no submitter validation claim"),
    };
    item.log("submitted", detail);
    state.save_item(&item)?;
    Ok(item)
}

/// Mark a queued change withdrawn.
pub fn withdraw(state: &State, pr: i64, why: &str) -> Result<Item, String> {
    let mut item = state
        .item(pr)?
        .ok_or_else(|| format!("#{pr} is not in the queue"))?;
    item.status = ItemStatus::Withdrawn;
    // Withdrawal is not decided by a run; drop any stale deciding record.
    item.deciding_report.clear();
    item.deciding_failed.clear();
    item.log("withdrawn", why);
    state.save_item(&item)?;
    Ok(item)
}

/// Return an escalated (or deferred) change to the queue: clear its escalation, refusal
/// count and backoff, here and in the shadow ledger. The owner runs this once whatever kept
/// validation from running is fixed.
pub fn retry(state: &State, pr: i64) -> Result<Item, String> {
    let mut item = state
        .item(pr)?
        .ok_or_else(|| format!("#{pr} is not in the queue"))?;
    if item.status != ItemStatus::Queued {
        return Err(format!("#{pr} is {}, not queued", item.status.as_str()));
    }
    let was = if item.escalated.is_empty() {
        format!("{} refusal(s)", item.validation_refusals)
    } else {
        format!("escalated: {}", item.escalated)
    };
    item.escalated.clear();
    item.validation_refusals = 0;
    item.next_retry_at.clear();
    item.last_refusal.clear();
    item.log(
        "retry",
        format!("returned to the queue by its owner (was {was})"),
    );
    state.save_item(&item)?;
    let mut ledger = state.shadow_refusals()?;
    if ledger.remove(&pr).is_some() {
        state.save_shadow_refusals(&ledger)?;
    }
    Ok(item)
}

/// Probe options for one queue: the persistent cache in the state
/// directory, the configured concurrency bound, and a hash of everything
/// besides commit IDs that can change a probe result.
fn probe_options_for(state: &State, config: &Config, git: &Git) -> ProbeOptions {
    // Probe results depend only on commit IDs and the merge configuration, so they are
    // cached in the state directory across runs; misses run under the configured bound.
    let config_material = format!(
        "union={:?};regen-paths={:?};regen-when={:?};regen-set={};combine={}",
        config.union_paths,
        config.regenerate.paths,
        config.regenerate.when,
        !config.regenerate.command.trim().is_empty(),
        config.combine.as_str()
    );
    // Merge answers also depend on attributes and merge configuration outside the queue's own
    // settings, and on the Git version; a change to any of them must not reuse old answers.
    let config_material = format!(
        "{config_material};{}",
        crate::probe_cache::merge_environment(git)
    );
    ProbeOptions {
        cache_dir: Some(state.root.clone()),
        parallelism: config.probe_parallelism(),
        config_hash: Some(probe_config_hash(git, &config_material)),
    }
}

/// Refresh the base and every queued head, probe them, and build a plan.
pub fn plan(state: &State, config: &Config, max_batch: usize) -> Result<Plan, String> {
    let items = state.items()?;
    let heads: Vec<(i64, String)> = items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued)
        .map(|item| (item.pr, item.head_ref.clone()))
        .collect();
    let git = repo(state, config)?;
    let (base_sha, current) = refresh(&git, config, &heads)?;
    let regenerate = config.regenerate.clone();
    let mut probe = probe_with_options(
        &git,
        &base_sha,
        &current,
        &|path| regenerate.covers(path),
        probe_options_for(state, config, &git),
    )?;
    probe.external = peer_holds(config, &items)?;
    Ok(make_plan(
        &items,
        &config.base,
        &base_sha,
        &probe,
        PlanOptions { max_batch },
    ))
}

/// The unfinished half of a staged plan (see [`plan_staged`]): everything
/// needed to probe the pairs the first stage left unknown and build the
/// completed plan, bound to the same base and heads. It owns its inputs so
/// a caller can finish it on a background thread while the first batch
/// already validates. Finishing is deterministic and needs no further
/// refresh: a base move in the meantime is handled by the caller's
/// existing base-moved flow, not by quietly rebasing this plan.
pub struct PlanContinuation {
    git: Git,
    items: Vec<Item>,
    base: String,
    base_sha: String,
    heads: BTreeMap<i64, Option<String>>,
    regenerate: RegenerateConfig,
    probe_options: ProbeOptions,
    max_batch: usize,
    external: BTreeMap<i64, Held>,
}

impl PlanContinuation {
    /// Exact base commit both the first-stage and the completed plan bind to.
    pub fn base_sha(&self) -> &str {
        &self.base_sha
    }

    /// Probe the remaining pairs and build the completed plan.
    ///
    /// The result equals [`plan`]'s plan for the same items, base and heads:
    /// it is the same full probe and the same [`make_plan`] over it. Pairs
    /// the first stage already cached are hits here, and base entries are
    /// hits too, so finishing re-runs only the pairs the first stage left
    /// unknown. Because pairwise cache keys do not name the base, a base
    /// move before finishing (or before the next plan) re-probes only the
    /// per-head base entries and no pair at all on a warm cache.
    pub fn finish(self) -> Result<Plan, String> {
        let regenerate = self.regenerate.clone();
        let mut probe = probe_with_options(
            &self.git,
            &self.base_sha,
            &self.heads,
            &|path| regenerate.covers(path),
            self.probe_options.clone(),
        )?;
        probe.external = self.external.clone();
        Ok(make_plan(
            &self.items,
            &self.base,
            &self.base_sha,
            &probe,
            PlanOptions {
                max_batch: self.max_batch,
            },
        ))
    }
}

/// Staged planning for `run`: a first-stage plan now, the rest on demand.
///
/// The returned plan is built from base probes plus whatever pairwise
/// results the cache already holds, so its first batch (if any) is ready
/// as soon as the per-head base probes return; no pairwise git subprocess
/// has run for it. Pairs the cache cannot decide are treated as clean
/// with no ancestry in this plan, which makes it optimistic: the batch is
/// still safe to validate, because building it re-checks combination and
/// validation tests its exact tree, but only the completed plan is
/// authoritative for the remaining batches. [`PlanContinuation::finish`]
/// produces that plan, identical to [`plan`]'s, and is meant to run
/// concurrently with validation of the first batch.
pub fn plan_staged(
    state: &State,
    config: &Config,
    max_batch: usize,
) -> Result<(Plan, PlanContinuation), String> {
    let items = state.items()?;
    let heads: Vec<(i64, String)> = items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued)
        .map(|item| (item.pr, item.head_ref.clone()))
        .collect();
    let git = repo(state, config)?;
    let (base_sha, current) = refresh(&git, config, &heads)?;
    let regenerate = config.regenerate.clone();
    let probe_options = probe_options_for(state, config, &git);
    let mut probe = probe_first_stage(
        &git,
        &base_sha,
        &current,
        &|path| regenerate.covers(path),
        probe_options.clone(),
    )?;
    let external = peer_holds(config, &items)?;
    probe.external = external.clone();
    let partial = make_plan(
        &items,
        &config.base,
        &base_sha,
        &probe,
        PlanOptions { max_batch },
    );
    let continuation = PlanContinuation {
        git,
        items,
        base: config.base.clone(),
        base_sha,
        heads: current,
        regenerate,
        probe_options,
        max_batch,
        external,
    };
    Ok((partial, continuation))
}

/// A peer queue: its state, configuration and repository.
pub struct Peer {
    /// Peer name.
    pub name: String,
    /// Peer state directory.
    pub state: State,
    /// Peer configuration.
    pub config: Config,
    /// Peer repository.
    pub git: Git,
}

/// Open the peer named `name`.
pub fn peer(config: &Config, name: &str) -> Result<Peer, String> {
    let dir = config.peers.get(name).ok_or_else(|| {
        format!("no peer named {name:?} (configure it with `init --peer {name}=DIR`)")
    })?;
    let state = State::at(dir.clone());
    let peer_config = state.config()?;
    let git = repo(&state, &peer_config)?;
    Ok(Peer {
        name: name.to_owned(),
        state,
        config: peer_config,
        git,
    })
}

/// Resolve `PEER#PR` (the head of a change queued in the peer, which is then marked exact so it
/// lands as its own commits) or `PEER@SHA` into a requirement.
pub fn resolve_requirement(config: &Config, spec: &str) -> Result<Requirement, String> {
    if let Some((name, number)) = spec.split_once('#') {
        let pr: i64 = number
            .parse()
            .map_err(|_| format!("bad requirement {spec:?}: expected PEER#PR or PEER@SHA"))?;
        let peer = peer(config, name)?;
        let _lock = peer.state.lock()?;
        let mut item = peer
            .state
            .item(pr)?
            .ok_or_else(|| format!("{name} has no change #{pr} in its queue"))?;
        if !item.exact {
            item.exact = true;
            item.log("exact", "another repository requires its exact head");
            peer.state.save_item(&item)?;
        }
        return Ok(Requirement {
            peer: name.to_owned(),
            commit: item.head,
            pr: Some(pr),
        });
    }
    if let Some((name, sha)) = spec.split_once('@') {
        let peer = peer(config, name)?;
        let commit = peer
            .git
            .try_rev_parse(sha)
            .unwrap_or_else(|| sha.to_owned());
        if commit.len() != 40 || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{sha:?} is not a commit {name}'s repository knows; give the full 40-hex commit"
            ));
        }
        return Ok(Requirement {
            peer: name.to_owned(),
            commit,
            pr: None,
        });
    }
    Err(format!(
        "bad requirement {spec:?}: expected PEER#PR or PEER@SHA"
    ))
}

/// Holds for queued items whose peer requirements are not yet on the peers' bases.
pub fn peer_holds(config: &Config, items: &[Item]) -> Result<BTreeMap<i64, Held>, String> {
    let mut holds = BTreeMap::new();
    let mut peers: BTreeMap<String, (Peer, String, Vec<Item>)> = BTreeMap::new();
    for item in items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued && !item.requires.is_empty())
    {
        for requirement in &item.requires {
            if !peers.contains_key(&requirement.peer) {
                let opened = peer(config, &requirement.peer)?;
                let (base, _) = refresh(&opened.git, &opened.config, &[])?;
                let peer_items = opened.state.items()?;
                peers.insert(requirement.peer.clone(), (opened, base, peer_items));
            }
            let (opened, base, peer_items) = &peers[&requirement.peer];
            if opened
                .git
                .is_ancestor(&requirement.commit, base)
                .unwrap_or(false)
            {
                continue;
            }
            let carrier = peer_items
                .iter()
                .find(|peer_item| peer_item.head == requirement.commit);
            let short = &requirement.commit[..requirement.commit.len().min(10)];
            let reason = match carrier {
                Some(peer_item) if peer_item.status == ItemStatus::Queued => format!(
                    "requires {}@{short} ({} #{} is queued; `run --with-peers` lands it first)",
                    opened.name, opened.name, peer_item.pr
                ),
                Some(peer_item) => format!(
                    "requires {}@{short}, but {} #{} is {}: it will not land at that commit",
                    opened.name,
                    opened.name,
                    peer_item.pr,
                    peer_item.status.as_str()
                ),
                None => format!(
                    "requires {}@{short}, which is neither on {}'s base nor in its queue",
                    opened.name, opened.name
                ),
            };
            holds.entry(item.pr).or_insert(Held {
                pr: item.pr,
                kind: "waiting-on-peer".into(),
                reason,
                with: vec![],
                paths: vec![],
            });
        }
    }
    Ok(holds)
}
