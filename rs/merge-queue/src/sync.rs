//! Ready sources: reconcile the queue with a label, every open pull request, or a ready-list
//! file.
//!
//! A source lists the changes that are ready now. `sync` submits new ones and moved heads (one
//! batched fetch for all of them), withdraws the changes this same source added that it no longer
//! lists, and retries closing landed pull requests whose close the request budget postponed.
//! Changes submitted directly with `submit` are never withdrawn by a source.
//!
//! With `require_approval_at_head`, a GitHub source's one batched list call also carries each
//! pull request's reviews: a change counts as ready only with an approving review at its current
//! head. An unapproved change is not submitted, and a queued change whose head no longer carries
//! an approval is withdrawn (notifying its submitter through the withdrawn path). The author
//! becomes the notification target only when the change has no `--notify` subscription yet; an
//! existing subscription is kept.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pr_landing_planner::priority::{LabelPriority, PriorityProvider, DEFAULT_LABEL_PATTERN};
use serde::Serialize;

use crate::github::Hosting;
use crate::model::ItemStatus;
use crate::ops::{self, Submission};
use crate::state::{Config, Lock, State, DEFAULT_PRIORITY};

/// Where the ready set comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReadySource {
    /// Open pull requests carrying this label (one API list call).
    Label(String),
    /// Every open, non-draft pull request targeting the base (one API list call).
    AllOpen,
    /// A local file, one change per line (no API call).
    File(PathBuf),
}

impl ReadySource {
    /// Stable tag recorded on the items this source submits.
    pub fn tag(&self) -> String {
        match self {
            Self::Label(label) => format!("label:{label}"),
            Self::AllOpen => "all-open".to_owned(),
            Self::File(path) => format!("file:{}", path.display()),
        }
    }
}

/// What a sync did.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SyncReport {
    /// Source tag.
    pub source: String,
    /// Whether nothing was changed.
    pub dry_run: bool,
    /// Ready changes the source listed.
    pub listed: usize,
    /// Draft pull requests the source returned but that are never ready (named, not hidden).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drafts: Vec<i64>,
    /// Changes submitted or resubmitted (new, or head or priority moved).
    pub submitted: Vec<i64>,
    /// Changes already queued at the listed head.
    pub unchanged: Vec<i64>,
    /// Changes withdrawn because this source no longer lists them.
    pub withdrawn: Vec<i64>,
    /// The listing held as many pull requests as the limit allowed, so it may have been cut
    /// short: a queued change missing from it was not withdrawn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// Listed changes that are not ready under `require_approval_at_head`
    /// (no approving review at the current head). They are not submitted;
    /// a queued one is also withdrawn (and so appears in `withdrawn`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unapproved: Vec<i64>,
    /// Submissions recorded for the next holder of the writer lock, because another writer
    /// (a run) held it (see `crate::intake`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recorded: Vec<i64>,
    /// Submissions refused, with the reason.
    pub refused: Vec<(i64, String)>,
    /// Landed pull requests closed now (postponed closes retried).
    pub closed: Vec<i64>,
    /// API requests left in the rolling hour, when the source used the API.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_remaining: Option<u32>,
}

/// One ready entry from any source.
#[derive(Clone, Debug)]
struct Entry {
    pr: i64,
    head_ref: String,
    head: String,
    priority: i64,
    after: Vec<i64>,
    solo: bool,
    /// Carries the priority label (or `prioritized` in a ready file).
    prioritized: bool,
    title: String,
    by: String,
    notify: String,
    /// Per-change `notify_mode` from a ready file (`notify-mode=MODE`).
    notify_mode: Option<crate::model::NotifyMode>,
    /// Whether an approving review at `head` was seen. Always true for
    /// sources that carry no review data (ready files) and when the
    /// approval rule is off, so those paths are unchanged.
    approved_at_head: bool,
    /// Whether `notify` is only a default (the author, under the approval
    /// rule) that must not replace an existing subscription.
    notify_is_default: bool,
}

/// One parsed ready-file line: the change number and its `key=value` fields.
pub type ReadyLine = (i64, Vec<(String, String)>);

/// Parse a ready-list file. Each line: `PR [SHA] [head=REF] [priority=N] [after=N,N] [solo]
/// [prioritized] [by=NAME] [notify=NAME]`; `#` starts a comment. Without `head=`, the head is
/// `refs/pull/PR/head`.
pub fn parse_ready_file(text: &str) -> Result<Vec<ReadyLine>, String> {
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let pr: i64 = words
            .next()
            .and_then(|word| word.trim_start_matches('#').parse().ok())
            .filter(|pr| *pr > 0)
            .ok_or_else(|| {
                format!(
                    "line {}: expected a change number first: {line}",
                    number + 1
                )
            })?;
        let mut fields = Vec::new();
        for word in words {
            match word.split_once('=') {
                Some((key, value)) => fields.push((key.to_owned(), value.to_owned())),
                None if word == "solo" || word == "prioritized" => {
                    fields.push((word.to_owned(), String::new()));
                }
                None if word.len() >= 7 && word.chars().all(|c| c.is_ascii_hexdigit()) => {
                    fields.push(("sha".to_owned(), word.to_owned()));
                }
                None => return Err(format!("line {}: unknown field {word:?}", number + 1)),
            }
        }
        entries.push((pr, fields));
    }
    Ok(entries)
}

fn entries_from_file(path: &PathBuf) -> Result<Vec<Entry>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    parse_ready_file(&text)?
        .into_iter()
        .map(|(pr, fields)| {
            let mut entry = Entry {
                pr,
                head_ref: format!("refs/pull/{pr}/head"),
                head: String::new(),
                priority: DEFAULT_PRIORITY,
                after: Vec::new(),
                solo: false,
                prioritized: false,
                title: String::new(),
                by: String::new(),
                notify: String::new(),
                notify_mode: None,
                approved_at_head: true,
                notify_is_default: false,
            };
            for (key, value) in fields {
                match key.as_str() {
                    "sha" => entry.head = value,
                    "head" => entry.head_ref = value,
                    "priority" => {
                        entry.priority = value
                            .parse()
                            .map_err(|_| format!("#{pr}: bad priority {value:?}"))?;
                    }
                    "after" => {
                        for dep in value.split(',').filter(|v| !v.is_empty()) {
                            entry.after.push(
                                dep.trim_start_matches('#')
                                    .parse()
                                    .map_err(|_| format!("#{pr}: bad after {dep:?}"))?,
                            );
                        }
                    }
                    "solo" => entry.solo = true,
                    "prioritized" => entry.prioritized = true,
                    "by" => entry.by = value,
                    "notify" => entry.notify = value,
                    "notify-mode" => {
                        entry.notify_mode = Some(crate::model::NotifyMode::parse(&value)?)
                    }
                    "title" => entry.title = value.replace('_', " "),
                    other => return Err(format!("#{pr}: unknown field {other:?}")),
                }
            }
            Ok(entry)
        })
        .collect()
}

/// What a source listed, before anything changed: [`list`] reads it (one API call for a
/// GitHub source), [`reconcile`] acts on it.
#[derive(Clone, Debug)]
pub struct Listing {
    entries: Vec<Entry>,
    drafts: Vec<i64>,
    truncated: bool,
}

/// How [`reconcile`] changes the queue.
#[derive(Clone, Copy)]
pub enum Apply<'a> {
    /// Change nothing: report what would change.
    DryRun,
    /// Submit and withdraw under the writer lock the caller holds.
    Locked(&'a Lock),
    /// Another writer holds the lock (the text says which): record the submissions for its
    /// next holder (`crate::intake`), so a run in progress can take them. Withdrawals wait for
    /// a sync that gets the lock.
    Record(&'a str),
}

/// The hosting client a sync from `source` uses: none for a ready file unless landed pull
/// requests are closed.
fn hosting_for(
    state: &State,
    config: &Config,
    source: &ReadySource,
) -> Result<Option<Hosting>, String> {
    Ok(match source {
        ReadySource::File(_) if !config.land.close_prs => None,
        _ => Some(Hosting::new(
            &config.github,
            &state.root,
            config.net_timeout(),
        )?),
    })
}

/// Read what `source` lists now. For a GitHub source this is the one list call, which can wait
/// on the request budget; it changes nothing, so a caller need not hold the writer lock.
pub fn list(
    state: &State,
    config: &Config,
    source: &ReadySource,
    limit: u32,
) -> Result<Listing, String> {
    let mut drafts = Vec::new();
    let mut truncated = false;
    let entries = match source {
        ReadySource::File(path) => entries_from_file(path)?,
        ReadySource::Label(_) | ReadySource::AllOpen => {
            let hosting = Hosting::new(&config.github, &state.root, config.net_timeout())?;
            let label = match source {
                ReadySource::Label(label) => Some(label.as_str()),
                _ => None,
            };
            let mut priorities = LabelPriority::new(DEFAULT_LABEL_PATTERN)?;
            let require_approval = config.require_approval_at_head;
            let listed = hosting.list_open(&config.base, label, limit, require_approval)?;
            truncated = listed.len() as u64 >= u64::from(limit);
            drafts = listed
                .iter()
                .filter(|pr| pr.draft)
                .map(|pr| pr.number)
                .collect();
            listed
                .into_iter()
                .filter(|pr| !pr.draft)
                .map(|pr| {
                    let labels: Vec<String> = pr.labels.iter().map(|l| l.name.clone()).collect();
                    let author = pr
                        .author
                        .as_ref()
                        .map(|a| a.login.clone())
                        .unwrap_or_default();
                    let approved_at_head = !require_approval || pr.approved_at_head();
                    Entry {
                        pr: pr.number,
                        head_ref: format!("refs/pull/{}/head", pr.number),
                        priority: priorities.priority(pr.number, &labels),
                        // Reviews arrived in the same batched list call
                        // above; no per-pull-request call is made here.
                        approved_at_head,
                        head: pr.head,
                        after: Vec::new(),
                        solo: false,
                        prioritized: config.priority.labelled(&labels),
                        title: pr.title,
                        by: author.clone(),
                        // Under the approval rule the author is the
                        // submitter a withdrawal must notify, through the
                        // existing withdrawn-notification path.
                        notify: if require_approval {
                            author
                        } else {
                            String::new()
                        },
                        notify_mode: None,
                        notify_is_default: require_approval,
                    }
                })
                .collect()
        }
    };
    Ok(Listing {
        entries,
        drafts,
        truncated,
    })
}

/// Reconcile the queue with `source`.
///
/// `lock` is the queue writer lock the caller holds; `None` is a dry run,
/// which reports what would change and writes nothing. The notification
/// sweep a withdrawal triggers runs under that same lock, with `notifier`
/// (the daemon passes its own, so its `--agentctl` and send timeout apply)
/// or, when `None`, the configured one.
pub fn sync(
    state: &State,
    config: &Config,
    source: &ReadySource,
    limit: u32,
    lock: Option<&Lock>,
    notifier: Option<&crate::notify::Notifier>,
) -> Result<SyncReport, String> {
    let hosting = hosting_for(state, config, source)?;
    let listing = list(state, config, source, limit)?;
    let apply = match lock {
        Some(lock) => Apply::Locked(lock),
        None => Apply::DryRun,
    };
    let mut report = reconcile(state, config, source, listing, apply, notifier)?;
    if let Some(lock) = lock {
        if config.land.close_prs && hosting.is_some() {
            close_postponed(state, config)?;
            report.closed = record_closed(state, lock)?;
        }
    }
    report.api_remaining = hosting.map(|h| h.budget.remaining());
    Ok(report)
}

/// Act on what `source` listed: submit new changes and moved heads, and withdraw the changes
/// this source added that it no longer lists (see [`Apply`] for how). Closing landed pull
/// requests is separate ([`close_postponed`], [`record_closed`]).
pub fn reconcile(
    state: &State,
    config: &Config,
    source: &ReadySource,
    listing: Listing,
    apply: Apply<'_>,
    notifier: Option<&crate::notify::Notifier>,
) -> Result<SyncReport, String> {
    let tag = source.tag();
    let Listing {
        entries,
        drafts,
        truncated,
    } = listing;
    let mut report = SyncReport {
        source: tag.clone(),
        dry_run: matches!(apply, Apply::DryRun),
        listed: entries.len(),
        drafts,
        truncated,
        ..SyncReport::default()
    };
    let items: BTreeMap<i64, _> = state.items()?.into_iter().map(|i| (i.pr, i)).collect();
    let listed: std::collections::BTreeSet<i64> = entries.iter().map(|e| e.pr).collect();
    let mut submissions = Vec::new();
    let mut approval_withdraw: Vec<i64> = Vec::new();
    for entry in entries {
        let current = items.get(&entry.pr);
        if config.require_approval_at_head && !entry.approved_at_head {
            // Not ready: never submitted. A queued change whose head no
            // longer carries an approval at its current head is withdrawn
            // (through the existing withdrawn path, so its submitter is
            // notified); resubmission with an approval at the new head
            // re-queues it through the ordinary path below.
            report.unapproved.push(entry.pr);
            if current.is_some_and(|item| item.status == ItemStatus::Queued) {
                approval_withdraw.push(entry.pr);
            }
            continue;
        }
        let same = current.is_some_and(|item| {
            item.status == ItemStatus::Queued
                && !entry.head.is_empty()
                && item.head.starts_with(&entry.head)
                && item.priority == entry.priority
                && item.prioritized == entry.prioritized
        });
        let settled = current.is_some_and(|item| {
            matches!(item.status, ItemStatus::Landed)
                || (item.status == ItemStatus::Ejected && item.head.starts_with(&entry.head))
        });
        if same || settled {
            report.unchanged.push(entry.pr);
            continue;
        }
        // A default target never replaces an existing `--notify`
        // subscription: an empty name makes the submission keep it.
        let notify =
            if entry.notify_is_default && current.is_some_and(|item| !item.notify.is_empty()) {
                String::new()
            } else {
                entry.notify
            };
        submissions.push(Submission {
            pr: entry.pr,
            head_ref: entry.head_ref,
            head: entry.head,
            priority: entry.priority,
            by: entry.by,
            notify,
            shepherd: String::new(),
            notify_mode: entry.notify_mode,
            title: entry.title,
            after: entry.after,
            solo: entry.solo,
            prioritized: entry.prioritized,
            exact: false,
            claim: None,
            source: tag.clone(),
            requires: Vec::new(),
            skip_precheck: false,
            recorded_at: String::new(),
        });
    }
    // A listing cut short by its limit says nothing about the changes it did not reach: only
    // a complete one withdraws what it no longer lists.
    let mut withdraw: Vec<i64> = items
        .values()
        .filter(|item| item.status == ItemStatus::Queued && item.source == tag)
        .filter(|item| !truncated && !listed.contains(&item.pr))
        .map(|item| item.pr)
        .collect();
    for pr in &approval_withdraw {
        if !withdraw.contains(pr) {
            withdraw.push(*pr);
        }
    }
    withdraw.sort_unstable();
    match apply {
        Apply::Locked(lock) => {
            let numbers: Vec<i64> = submissions.iter().map(|s| s.pr).collect();
            if !submissions.is_empty() {
                for (pr, result) in
                    numbers
                        .iter()
                        .zip(ops::submit_many(state, config, submissions)?)
                {
                    match result {
                        Ok(_) => report.submitted.push(*pr),
                        Err(error) => report.refused.push((*pr, error)),
                    }
                }
            }
            let approval_withdrawn = !approval_withdraw.is_empty();
            for pr in withdraw {
                let why = if approval_withdraw.contains(&pr) {
                    format!("{tag}: no approving review at current head (require_approval_at_head)")
                } else {
                    format!("{tag} no longer lists it")
                };
                ops::withdraw(state, pr, &why)?;
                report.withdrawn.push(pr);
            }
            if approval_withdrawn {
                // Fire the existing withdrawn notification now rather than
                // waiting for the daemon: `ops::withdraw` above set the
                // withdrawn status the notify sweep derives its transition
                // from, and the sweep dedups, so a later daemon cycle will
                // not resend. Best-effort, as in `serve`: a failed send is
                // recorded on the item and never fails the sync.
                let configured;
                let notifier = match notifier {
                    Some(notifier) => notifier,
                    None => {
                        configured = crate::notify::Notifier::resolve(None, None, config);
                        &configured
                    }
                };
                if let Err(error) = crate::notify::sweep_locked(state, lock, config, notifier) {
                    eprintln!("merge-queue sync: notification sweep failed: {error}");
                }
            }
        }
        Apply::Record(busy) => {
            for submission in submissions {
                let pr = submission.pr;
                let seen = submission.head.clone();
                match crate::intake::record(
                    state,
                    crate::intake::Request::Submit(Box::new(submission)),
                    busy,
                    &seen,
                ) {
                    Ok(_) => report.recorded.push(pr),
                    Err(error) => report.refused.push((pr, error)),
                }
            }
        }
        Apply::DryRun => {
            report.submitted = submissions.iter().map(|s| s.pr).collect();
            report.withdrawn = withdraw;
        }
    }
    Ok(report)
}

/// Where a pull request closed after landing waits, until a holder of the writer lock records
/// the close on its item ([`record_closed`]).
fn closed_dir(state: &State) -> PathBuf {
    state.root.join("closed")
}

/// Close the landed pull requests whose close the request budget postponed, oldest first,
/// stopping at the first that fails (the budget is likely spent). The API calls can wait on the
/// budget, so this takes no writer lock: each close is noted under `<state>/closed/` for
/// [`record_closed`], and a change noted there is not closed again.
pub fn close_postponed(state: &State, config: &Config) -> Result<(), String> {
    let hosting = Hosting::new(&config.github, &state.root, config.net_timeout())?;
    for item in state.items()? {
        if item.status != ItemStatus::Landed || !item.close_pending {
            continue;
        }
        let note = closed_dir(state).join(format!("{}.json", item.pr));
        if note.exists() {
            continue;
        }
        match hosting.close(item.pr, &close_comment(&item.landed_as)) {
            Ok(()) => crate::state::write_json(&note, &item.pr)?,
            Err(_) => break,
        }
    }
    Ok(())
}

/// Record on their items the closes [`close_postponed`] noted, under the writer lock the caller
/// holds. Returns the changes recorded.
pub fn record_closed(state: &State, _lock: &Lock) -> Result<Vec<i64>, String> {
    let dir = closed_dir(state);
    let notes = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot list {}: {error}", dir.display())),
    };
    let mut prs: Vec<i64> = notes
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()?
                .strip_suffix(".json")?
                .parse()
                .ok()
        })
        .collect();
    prs.sort_unstable();
    let mut recorded = Vec::new();
    for pr in prs {
        if let Some(mut item) = state.item(pr)? {
            if item.status == ItemStatus::Landed && item.close_pending {
                item.close_pending = false;
                item.log("closed", "pull request closed after landing");
                state.save_item(&item)?;
                recorded.push(pr);
            }
        }
        let note = dir.join(format!("{pr}.json"));
        std::fs::remove_file(&note)
            .map_err(|error| format!("cannot remove {}: {error}", note.display()))?;
    }
    Ok(recorded)
}

/// The comment left on a pull request closed after landing.
pub fn close_comment(landed: &str) -> String {
    format!("Landed by merge-queue in {landed} (the exact validated commit; see the queue's run report).")
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn ready_file_lines() {
        let entries = parse_ready_file(
            "# ready list\n12 abcdef1 priority=50 after=10,#11\n13 head=feature/x solo by=lane-a\n\n",
        )
        .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, 12);
        assert!(entries[0].1.contains(&("sha".into(), "abcdef1".into())));
        assert!(entries[1].1.contains(&("solo".into(), String::new())));
        assert!(parse_ready_file("x y").is_err());
        assert!(parse_ready_file("5 bogus").is_err());
    }
}
