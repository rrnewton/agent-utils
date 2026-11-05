//! Queue operations shared by the command line and tests: refresh, submit, plan.

use std::collections::{BTreeMap, BTreeSet};

use crate::git::Git;
use crate::model::{now, Held, HeldCandidate, Item, ItemStatus, Plan, Requirement, Ruling};
use crate::plan::{
    make_plan, probe_first_stage, probe_with_options, short, PlanOptions, ProbeOptions,
};
use crate::probe_cache::probe_config_hash;
use crate::state::{Config, State};
use crate::state::{PriorityConfig, RegenerateConfig};
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
    git.net_timeout = config.net_timeout();
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
    if let Err(error) = git.run_net(&args) {
        // A hung transport is not a missing head: fetching one ref at a time would only wait
        // again, once per ref.
        if crate::bounded::is_timeout(&error) {
            return Err(error);
        }
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

/// The commit a change's head names now, read without writing to the repository (a run may be
/// using it): `ls-remote` on the remote, or the local ref of a local queue.
pub fn peek_head(state: &State, config: &Config, head_ref: &str) -> Result<String, String> {
    let git = repo(state, config)?;
    let source = source_ref(head_ref);
    if config.remote.is_empty() {
        return git
            .try_rev_parse(&source)
            .ok_or_else(|| format!("head {head_ref:?} does not resolve"));
    }
    git.run_net(&["ls-remote", &config.remote, &source])?
        .lines()
        .find_map(|line| {
            line.split_once('\t')
                .filter(|(_, name)| *name == source)
                .map(|(sha, _)| sha.to_owned())
        })
        .ok_or_else(|| format!("head {head_ref:?} does not resolve on {}", config.remote))
}

/// What to record for a submission.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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
    /// A priority change: planned ahead of all other work, validated alone.
    pub prioritized: bool,
    /// Submitter's validation claim.
    pub claim: Option<ValidationClaim>,
    /// `sync` source that produced this submission; empty for a direct `submit`.
    pub source: String,
    /// Peer commits that must land first.
    pub requires: Vec<Requirement>,
    /// Queue the head without the precheck (`submit --skip-precheck`), for a refusal that is
    /// wrong.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub skip_precheck: bool,
    /// When the submission was recorded because another writer held the queue (see
    /// [`crate::intake`]); empty for one applied at once.
    #[serde(skip)]
    pub recorded_at: String,
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
    let (base, heads) = refresh(&git, config, &wanted)?;
    Ok(submissions
        .into_iter()
        .map(|submission| {
            let current = heads.get(&submission.pr).cloned().flatten();
            submit_resolved(state, config, &git, &base, submission, current)
        })
        .collect())
}

fn submit_resolved(
    state: &State,
    config: &Config,
    git: &Git,
    base: &str,
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
        prioritized: submission.prioritized,
        claim: submission.claim,
        exact: existing.as_ref().is_some_and(|old| old.exact),
        requires: submission.requires,
        source: submission.source,
        close_pending: false,
        suspect_reruns: if same_head {
            existing.as_ref().map_or(0, |old| old.suspect_reruns)
        } else {
            0
        },
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
        flaky_nodes: if same_head {
            existing
                .as_ref()
                .map_or_else(Vec::new, |old| old.flaky_nodes.clone())
        } else {
            Vec::new()
        },
        flaky_hold_until: if same_head {
            existing
                .as_ref()
                .map_or_else(String::new, |old| old.flaky_hold_until.clone())
        } else {
            String::new()
        },
        hold_verdict: if same_head {
            existing
                .as_ref()
                .map_or_else(String::new, |old| old.hold_verdict.clone())
        } else {
            String::new()
        },
        held_candidate: if same_head {
            existing.as_ref().and_then(|old| old.held_candidate.clone())
        } else {
            None
        },
        accepted: if same_head {
            existing.as_ref().and_then(|old| old.accepted.clone())
        } else {
            None
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
    let mut detail = match &item.claim {
        Some(claim) => format!(
            "head {current}; submitter validated {} ({}, policy {})",
            claim.selection.summary(),
            claim.scope.as_str(),
            claim.policy
        ),
        None => format!("head {current}; no submitter validation claim"),
    };
    if !submission.recorded_at.is_empty() {
        detail.push_str(&format!(
            "; recorded at {} while another writer held the queue",
            submission.recorded_at
        ));
    }
    // The precheck runs on the head alone, before any batch is built on it.
    let mut refusal = None;
    if submission.skip_precheck {
        if !config.precheck.command.trim().is_empty() {
            crate::precheck::skip(&state.root, config, &current, &item.submitted_by)?;
            detail.push_str("; precheck skipped (--skip-precheck)");
        }
    } else if let Some(verdict) = crate::precheck::check(
        &state.root,
        config,
        git,
        item.pr,
        &item.head_ref,
        &current,
        base,
    ) {
        match verdict.outcome {
            crate::precheck::Outcome::Passed => detail.push_str(&format!(
                "; precheck passed ({:.1}s)",
                verdict.elapsed_ms as f64 / 1000.0
            )),
            crate::precheck::Outcome::CouldNotRun => detail.push_str(&format!(
                "; precheck could not run, so it was queued without one: {}",
                verdict.reason
            )),
            crate::precheck::Outcome::Skipped => {
                detail.push_str(&format!("; precheck {}", verdict.reason));
            }
            crate::precheck::Outcome::Failed => {
                let kept = crate::precheck::verdict_path(&state.root, &current, &verdict.command);
                refusal = Some((verdict.refusal(item.pr, &kept), kept));
            }
        }
    }
    item.log("submitted", detail);
    if let Some((text, kept)) = refusal {
        // Ejected before any batch is built, and the submitter hears of it now rather than
        // when the run holding the queue ends.
        item.status = ItemStatus::Ejected;
        item.deciding_report = kept.display().to_string();
        item.deciding_failed.clear();
        item.log("ejected", format!("refused at submission: {text}"));
        state.save_item(&item)?;
        crate::notify::deliver_now(state, config, item.pr);
        return Err(text);
    }
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
    retry_recorded(state, pr, "")
}

/// [`retry`] of a request recorded at `recorded_at` while another writer held the queue (see
/// [`crate::intake`]); the time goes into the change's history.
pub fn retry_recorded(state: &State, pr: i64, recorded_at: &str) -> Result<Item, String> {
    let mut item = state
        .item(pr)?
        .ok_or_else(|| format!("#{pr} is not in the queue"))?;
    if item.status != ItemStatus::Queued {
        return Err(format!("#{pr} is {}, not queued", item.status.as_str()));
    }
    let was = if !item.flaky_hold_until.is_empty() {
        format!(
            "{} hold ({}; until {})",
            item.hold_label(),
            item.flaky_nodes.join(", "),
            item.flaky_hold_until
        )
    } else if item.escalated.is_empty() {
        format!("{} refusal(s)", item.validation_refusals)
    } else {
        format!("escalated: {}", item.escalated)
    };
    item.escalated.clear();
    item.clear_hold();
    item.validation_refusals = 0;
    item.next_retry_at.clear();
    item.last_refusal.clear();
    let recorded = if recorded_at.is_empty() {
        String::new()
    } else {
        format!("; recorded at {recorded_at} while another writer held the queue")
    };
    item.log(
        "retry",
        format!("returned to the queue by its owner (was {was}){recorded}"),
    );
    state.save_item(&item)?;
    let mut ledger = state.shadow_refusals()?;
    if ledger.remove(&pr).is_some() {
        state.save_shadow_refusals(&ledger)?;
    }
    Ok(item)
}

/// The held batch `prs` names, checked for `merge-queue accept-flaky`: every change is queued
/// and held for a GREEN_FLAKY ruling with the candidate it was validated in recorded, they were
/// all validated together, and `prs` is that whole batch, each still at the head validated.
/// Returns the items and the candidate.
pub fn held_batch(state: &State, prs: &[i64]) -> Result<(Vec<Item>, HeldCandidate), String> {
    let mut items = Vec::new();
    let mut held: Option<HeldCandidate> = None;
    for pr in prs {
        let item = state
            .item(*pr)?
            .ok_or_else(|| format!("#{pr} is not in the queue"))?;
        if item.status != ItemStatus::Queued {
            return Err(format!("#{pr} is {}, not queued", item.status.as_str()));
        }
        if item.flaky_hold_until.is_empty() {
            return Err(format!("#{pr} is not held for a ruling"));
        }
        if item.hold_verdict == crate::validate::SUSPECT_FLAKE_MARKER {
            return Err(format!(
                "#{pr} is held {} while {} run again beside the queue, not {}: there is no pass \
                 to accept; it goes back to the queue if they pass",
                item.hold_verdict,
                item.flaky_nodes.join(", "),
                crate::validate::FLAKY_MARKER
            ));
        }
        if !item.hold_verdict.is_empty() {
            return Err(format!(
                "#{pr} is held {} (the host refused to measure {}), not {}: there is no pass to \
                 accept; `merge-queue retry {pr}` validates it again once the host is fixed",
                item.hold_verdict,
                item.flaky_nodes.join(", "),
                crate::validate::FLAKY_MARKER
            ));
        }
        let Some(candidate) = item.held_candidate.clone() else {
            return Err(format!(
                "#{pr}'s hold records no validated candidate (it was held after a red batch's \
                 rerun, or by an older version of the queue): `merge-queue retry {pr}` validates \
                 it again"
            ));
        };
        match &held {
            Some(first) if first.tip != candidate.tip => {
                return Err(format!(
                    "#{} and #{pr} were held in different batches ({} and {}): accept one batch at \
                     a time",
                    prs[0],
                    short(&first.tip),
                    short(&candidate.tip)
                ));
            }
            Some(_) => {}
            None => held = Some(candidate),
        }
        items.push(item);
    }
    let held = held.ok_or("name the changes of the held batch")?;
    let named: BTreeSet<i64> = prs.iter().copied().collect();
    let batch: BTreeSet<i64> = held.prs().into_iter().collect();
    if named != batch {
        return Err(format!(
            "the held batch is {} (validated together at {}): name every change of it, and only \
             those",
            held.prs()
                .iter()
                .map(|pr| format!("#{pr}"))
                .collect::<Vec<_>>()
                .join(", "),
            short(&held.tip)
        ));
    }
    for item in &items {
        let validated = held
            .changes
            .iter()
            .find(|change| change.pr == item.pr)
            .map(|change| change.head.as_str())
            .unwrap_or_default();
        if item.head != validated {
            return Err(format!(
                "#{}'s head is {}, not the head {} the held batch validated: `merge-queue retry \
                 {}` validates it again",
                item.pr,
                short(&item.head),
                short(validated),
                item.pr
            ));
        }
    }
    Ok((items, held))
}

/// Accept the GREEN_FLAKY passes of the held batch `prs` (see [`held_batch`]): the ruling is
/// recorded on every change of it, and the next run acts on it, landing exactly the commit that
/// was validated if the base has not moved, and otherwise returning the batch to the queue for
/// a new validation. `recorded_at` is the time the request was recorded while another writer
/// held the queue (see [`crate::intake`]), or empty.
pub fn accept_flaky(
    state: &State,
    prs: &[i64],
    by: &str,
    reason: &str,
    recorded_at: &str,
) -> Result<HeldCandidate, String> {
    if by.trim().is_empty() || reason.trim().is_empty() {
        return Err("a ruling names who gives it (--by) and why (--reason)".to_owned());
    }
    let (items, held) = held_batch(state, prs)?;
    let recorded = if recorded_at.is_empty() {
        String::new()
    } else {
        format!("; recorded at {recorded_at} while another writer held the queue")
    };
    let ruling = Ruling {
        by: by.to_owned(),
        reason: reason.to_owned(),
        at: if recorded_at.is_empty() {
            now()
        } else {
            recorded_at.to_owned()
        },
    };
    for mut item in items {
        item.accepted = Some(ruling.clone());
        item.log(
            "accepted-flaky",
            format!(
                "{by} accepted the {} passes ({}) of {} at {}: {reason}; the next run lands that \
                 commit if the base is still {}{recorded}",
                crate::validate::FLAKY_MARKER,
                item.flaky_nodes.join(", "),
                held.prs()
                    .iter()
                    .map(|pr| format!("#{pr}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                short(&held.tip),
                short(&held.parent)
            ),
        );
        state.save_item(&item)?;
    }
    Ok(held)
}

/// Probe options for one queue: the persistent cache in the state
/// directory, the configured concurrency bound, and a hash of everything
/// besides commit IDs that can change a probe result.
fn probe_options_for(state: &State, config: &Config, git: &Git) -> ProbeOptions {
    // Probe results depend only on commit IDs and the merge configuration, so they are
    // cached in the state directory across runs; misses run under the configured bound.
    let config_material = format!(
        "union={:?};regen-paths={:?};regen-when={:?};regen-set={};combine={};recount={}",
        config.union_paths,
        config.regenerate.paths,
        config.regenerate.when,
        !config.regenerate.command.trim().is_empty(),
        config.combine.as_str(),
        config.recount_pins
    );
    // Pin sites settle like regenerated files; a queue without pins keeps its cache keys.
    let config_material = if config.pins.is_empty() {
        config_material
    } else {
        let sites: Vec<_> = config
            .pins
            .iter()
            .map(|pin| (&pin.gitlink, &pin.files))
            .collect();
        format!("{config_material};pins={sites:?}")
    };
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
        peripheral: config
            .peripheral
            .enabled()
            .then(|| crate::plan::PeripheralHook {
                command: config.peripheral.command.clone(),
                timeout_secs: config.peripheral.timeout(),
            }),
    }
}

/// The peripheral batch cap a plan uses, resolved against its `max_batch`.
fn peripheral_limit(config: &Config, max_batch: usize) -> usize {
    if config.peripheral.enabled() {
        config.peripheral.batch_limit(max_batch)
    } else {
        0
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
    let recount = config.recount_pins;
    let pins = config.pins.clone();
    let mut probe = probe_with_options(
        &git,
        &base_sha,
        &current,
        &|path| {
            regenerate.covers(path)
                || (recount && path == crate::recount::PIN_PATH)
                || pins.iter().any(|pin| pin.covers(path))
        },
        probe_options_for(state, config, &git),
    )?;
    probe.external = peer_holds(config, &items)?;
    Ok(make_plan(
        &items,
        &config.base,
        &base_sha,
        &probe,
        PlanOptions {
            max_batch,
            priority: config.priority.clone(),
            peripheral_max_batch: peripheral_limit(config, max_batch),
        },
    ))
}

/// Plan the queued changes a run has not started on top of `tip`, the candidate of the last
/// batch it has in flight, for a run taking in requests recorded while it held the queue. The
/// changes in `carried` are already in `tip`, so the plan treats them as landed; the changes in
/// `skip` (started but not carried by `tip`, or set aside by the run) are not planned. The base
/// probes run against `tip`, so a change that conflicts with the work in flight is held rather
/// than planned.
pub fn plan_on(
    state: &State,
    config: &Config,
    max_batch: usize,
    tip: &str,
    carried: &BTreeSet<i64>,
    skip: &BTreeSet<i64>,
) -> Result<Plan, String> {
    let mut items = state.items()?;
    for item in items
        .iter_mut()
        .filter(|item| item.status == ItemStatus::Queued)
    {
        if carried.contains(&item.pr) {
            item.status = ItemStatus::Landed;
        } else if skip.contains(&item.pr) {
            item.status = ItemStatus::Withdrawn;
        }
    }
    let heads: Vec<(i64, String)> = items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued)
        .map(|item| (item.pr, item.head_ref.clone()))
        .collect();
    let git = repo(state, config)?;
    let (_, current) = refresh(&git, config, &heads)?;
    let regenerate = config.regenerate.clone();
    let recount = config.recount_pins;
    let pins = config.pins.clone();
    let mut probe = probe_with_options(
        &git,
        tip,
        &current,
        &|path| {
            regenerate.covers(path)
                || (recount && path == crate::recount::PIN_PATH)
                || pins.iter().any(|pin| pin.covers(path))
        },
        probe_options_for(state, config, &git),
    )?;
    probe.external = peer_holds(config, &items)?;
    Ok(make_plan(
        &items,
        &config.base,
        tip,
        &probe,
        PlanOptions {
            max_batch,
            priority: config.priority.clone(),
            peripheral_max_batch: peripheral_limit(config, max_batch),
        },
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
    recount: bool,
    pins: Vec<crate::state::PinConfig>,
    probe_options: ProbeOptions,
    max_batch: usize,
    peripheral_max_batch: usize,
    priority: PriorityConfig,
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
        let recount = self.recount;
        let pins = self.pins.clone();
        let mut probe = probe_with_options(
            &self.git,
            &self.base_sha,
            &self.heads,
            &|path| {
                regenerate.covers(path)
                    || (recount && path == crate::recount::PIN_PATH)
                    || pins.iter().any(|pin| pin.covers(path))
            },
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
                priority: self.priority.clone(),
                peripheral_max_batch: self.peripheral_max_batch,
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
    let recount = config.recount_pins;
    let pins = config.pins.clone();
    let probe_options = probe_options_for(state, config, &git);
    let mut probe = probe_first_stage(
        &git,
        &base_sha,
        &current,
        &|path| {
            regenerate.covers(path)
                || (recount && path == crate::recount::PIN_PATH)
                || pins.iter().any(|pin| pin.covers(path))
        },
        probe_options.clone(),
    )?;
    let external = peer_holds(config, &items)?;
    probe.external = external.clone();
    let partial = make_plan(
        &items,
        &config.base,
        &base_sha,
        &probe,
        PlanOptions {
            max_batch,
            priority: config.priority.clone(),
            peripheral_max_batch: peripheral_limit(config, max_batch),
        },
    );
    let continuation = PlanContinuation {
        git,
        items,
        base: config.base.clone(),
        base_sha,
        heads: current,
        regenerate,
        recount,
        pins,
        probe_options,
        max_batch,
        peripheral_max_batch: peripheral_limit(config, max_batch),
        priority: config.priority.clone(),
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
