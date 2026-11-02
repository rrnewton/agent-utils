//! Ready sources: reconcile the queue with a label, every open pull request, or a ready-list
//! file.
//!
//! A source lists the changes that are ready now. `sync` submits new ones and moved heads (one
//! batched fetch for all of them), withdraws the changes this same source added that it no longer
//! lists, and retries closing landed pull requests whose close the request budget postponed.
//! Changes submitted directly with `submit` are never withdrawn by a source.

use std::collections::BTreeMap;
use std::path::PathBuf;

use pr_landing_planner::priority::{LabelPriority, PriorityProvider, DEFAULT_LABEL_PATTERN};
use serde::Serialize;

use crate::github::Hosting;
use crate::model::ItemStatus;
use crate::ops::{self, Submission};
use crate::state::{Config, State, DEFAULT_PRIORITY};

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
    /// Changes submitted or resubmitted (new, or head or priority moved).
    pub submitted: Vec<i64>,
    /// Changes already queued at the listed head.
    pub unchanged: Vec<i64>,
    /// Changes withdrawn because this source no longer lists them.
    pub withdrawn: Vec<i64>,
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
    title: String,
    by: String,
    notify: String,
}

/// One parsed ready-file line: the change number and its `key=value` fields.
pub type ReadyLine = (i64, Vec<(String, String)>);

/// Parse a ready-list file. Each line: `PR [SHA] [head=REF] [priority=N] [after=N,N] [solo]
/// [by=NAME] [notify=NAME]`; `#` starts a comment. Without `head=`, the head is
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
                None if word == "solo" => fields.push(("solo".to_owned(), String::new())),
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
                title: String::new(),
                by: String::new(),
                notify: String::new(),
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
                    "by" => entry.by = value,
                    "notify" => entry.notify = value,
                    "title" => entry.title = value.replace('_', " "),
                    other => return Err(format!("#{pr}: unknown field {other:?}")),
                }
            }
            Ok(entry)
        })
        .collect()
}

/// Reconcile the queue with `source`.
pub fn sync(
    state: &State,
    config: &Config,
    source: &ReadySource,
    limit: u32,
    dry_run: bool,
) -> Result<SyncReport, String> {
    let tag = source.tag();
    let mut report = SyncReport {
        source: tag.clone(),
        dry_run,
        ..SyncReport::default()
    };
    let hosting = match source {
        ReadySource::File(_) if !config.land.close_prs => None,
        _ => Some(Hosting::new(&config.github, &state.root)?),
    };
    let entries = match source {
        ReadySource::File(path) => entries_from_file(path)?,
        ReadySource::Label(_) | ReadySource::AllOpen => {
            let hosting = hosting.as_ref().ok_or("no hosting configured")?;
            let label = match source {
                ReadySource::Label(label) => Some(label.as_str()),
                _ => None,
            };
            let mut priorities = LabelPriority::new(DEFAULT_LABEL_PATTERN)?;
            hosting
                .list_open(&config.base, label, limit)?
                .into_iter()
                .filter(|pr| !pr.draft)
                .map(|pr| {
                    let labels: Vec<String> = pr.labels.iter().map(|l| l.name.clone()).collect();
                    Entry {
                        pr: pr.number,
                        head_ref: format!("refs/pull/{}/head", pr.number),
                        priority: priorities.priority(pr.number, &labels),
                        head: pr.head,
                        after: Vec::new(),
                        solo: false,
                        title: pr.title,
                        by: pr.author.map(|a| a.login).unwrap_or_default(),
                        notify: String::new(),
                    }
                })
                .collect()
        }
    };
    report.listed = entries.len();
    let items: BTreeMap<i64, _> = state.items()?.into_iter().map(|i| (i.pr, i)).collect();
    let listed: std::collections::BTreeSet<i64> = entries.iter().map(|e| e.pr).collect();
    let mut submissions = Vec::new();
    for entry in entries {
        let current = items.get(&entry.pr);
        let same = current.is_some_and(|item| {
            item.status == ItemStatus::Queued
                && !entry.head.is_empty()
                && item.head.starts_with(&entry.head)
                && item.priority == entry.priority
        });
        let settled = current.is_some_and(|item| {
            matches!(item.status, ItemStatus::Landed)
                || (item.status == ItemStatus::Ejected && item.head.starts_with(&entry.head))
        });
        if same || settled {
            report.unchanged.push(entry.pr);
            continue;
        }
        submissions.push(Submission {
            pr: entry.pr,
            head_ref: entry.head_ref,
            head: entry.head,
            priority: entry.priority,
            by: entry.by,
            notify: entry.notify,
            title: entry.title,
            after: entry.after,
            solo: entry.solo,
            claim: None,
            source: tag.clone(),
            requires: Vec::new(),
        });
    }
    let withdraw: Vec<i64> = items
        .values()
        .filter(|item| item.status == ItemStatus::Queued && item.source == tag)
        .filter(|item| !listed.contains(&item.pr))
        .map(|item| item.pr)
        .collect();
    if dry_run {
        report.submitted = submissions.iter().map(|s| s.pr).collect();
        report.withdrawn = withdraw;
    } else {
        let numbers: Vec<i64> = submissions.iter().map(|s| s.pr).collect();
        if !submissions.is_empty() {
            for (pr, result) in numbers
                .iter()
                .zip(ops::submit_many(state, config, submissions)?)
            {
                match result {
                    Ok(_) => report.submitted.push(*pr),
                    Err(error) => report.refused.push((*pr, error)),
                }
            }
        }
        for pr in withdraw {
            ops::withdraw(state, pr, &format!("{tag} no longer lists it"))?;
            report.withdrawn.push(pr);
        }
        if config.land.close_prs {
            if let Some(hosting) = &hosting {
                for mut item in state.items()? {
                    if item.status != ItemStatus::Landed || !item.close_pending {
                        continue;
                    }
                    match hosting.close(item.pr, &close_comment(&item.landed_as)) {
                        Ok(()) => {
                            item.close_pending = false;
                            item.log("closed", "pull request closed after landing");
                            state.save_item(&item)?;
                            report.closed.push(item.pr);
                        }
                        Err(_) => break,
                    }
                }
            }
        }
    }
    report.api_remaining = hosting.map(|h| h.budget.remaining());
    Ok(report)
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
