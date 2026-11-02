//! Queue operations shared by the command line and tests: refresh, submit, plan.

use std::collections::BTreeMap;

use crate::git::Git;
use crate::model::{now, Item, ItemStatus, Plan};
use crate::plan::{make_plan, probe, PlanOptions};
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
    /// One-line description.
    pub title: String,
    /// Changes that must land first.
    pub after: Vec<i64>,
    /// Land alone.
    pub solo: bool,
    /// Submitter's validation claim.
    pub claim: Option<ValidationClaim>,
}

/// Submit (or resubmit) a change. Returns the stored item.
pub fn submit(state: &State, config: &Config, submission: Submission) -> Result<Item, String> {
    if submission.pr <= 0 {
        return Err(format!(
            "change number must be positive, got {}",
            submission.pr
        ));
    }
    if submission.head_ref.is_empty() {
        return Err("a head branch or ref is required".to_owned());
    }
    let git = repo(state, config)?;
    let (_, heads) = refresh(
        &git,
        config,
        &[(submission.pr, submission.head_ref.clone())],
    )?;
    let current = heads
        .get(&submission.pr)
        .cloned()
        .flatten()
        .ok_or_else(|| {
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
    let mut item = Item {
        pr: submission.pr,
        head_ref: submission.head_ref,
        head: current.clone(),
        priority: submission.priority,
        submitted_at: now(),
        submitted_by: submission.by,
        title: submission.title,
        after: submission.after,
        solo: submission.solo,
        claim: submission.claim,
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
    item.log("withdrawn", why);
    state.save_item(&item)?;
    Ok(item)
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
    let probe = probe(&git, &base_sha, &current, &|path| regenerate.covers(path))?;
    Ok(make_plan(
        &items,
        &config.base,
        &base_sha,
        &probe,
        PlanOptions { max_batch },
    ))
}
