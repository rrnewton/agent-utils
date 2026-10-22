//! Least-recently-used replacement of idle tabs once a workspace reaches `max_panes`.
//!
//! A new agent that needs a tab in a full workspace no longer has to wait for somebody to close
//! one by hand. Instead the tab whose last recorded `herdr-run` run is OLDEST is closed, provided
//! its shell is provably idle, and the new tab takes its place.
//!
//! The same idle verdict also retires tabs on a lifetime ([`retire_idle_tabs`]): a tab whose last
//! recorded run finished more than `idle_tab_ttl_seconds` ago is closed even when the workspace
//! is nowhere near the cap. Without that, the cap is the only closer, so every command workspace
//! fills to `max_panes` idle shells and stays there for as long as the server runs.
//!
//! "Provably idle" is deliberately narrower than "looks quiet":
//!
//! * no other `herdr-run` holds the pane's lock (the runner holds it for a whole command);
//! * the shell alone owns the terminal's foreground process group ([`assess_process`]);
//! * the shell leads its own session and NO other process is in that session, so a background
//!   job (`cmd &`) or a stopped job keeps the tab open;
//! * the tab holds exactly one pane, because closing a tab closes every pane in it.
//!
//! The verdict is taken under the pane lock, immediately before `tab close`, never from an earlier
//! survey. When `/proc` cannot be read the tab counts as busy: failing closed only costs a refusal,
//! failing open would kill somebody's command. One window remains, and it is harmless: a caller
//! that resolved its target before the close, and takes the pane lock after it, finds the pane gone
//! and fails its readiness check before anything is typed.
//!
//! Order comes from the run spool. Panes with no record at all — tabs nobody has used through
//! `herdr-run`, including leaked ones `reap` cannot judge — come first, in listing order.

use std::path::Path;

use fs2::FileExt;
use serde_json::{Map, Value};

use crate::audit;
use crate::client::{HerdrApi, Pane};
use crate::config::Config;
use crate::error::{HerdrRunError, Result};
use crate::output;
use crate::readiness::{assess_process, ProcessSignal};
use crate::state::{open_lock_file, pane_lock_path};
use crate::sweep::{load_run_records, scope_of};
use crate::textfmt::{number_text, quote, quote_optional};

/// How many skipped tabs a refusal names before summarising the rest as a count.
const REFUSAL_DETAIL_LIMIT: usize = 3;

/// One tab considered for replacement, in least-recently-used order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Herdr tab identifier, the unit that is closed.
    pub tab_id: String,
    /// Run ID (spool directory name) of the pane's most recent recorded run, if any.
    pub last_run: Option<String>,
    /// Agent label of that run, if any.
    pub agent: Option<String>,
    /// Number of panes the listing shows in this tab.
    pub tab_panes: usize,
}

/// What `/proc` says about the shell's session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionScan {
    /// PID of the pane's shell.
    pub shell_pid: i64,
    /// Session ID of the shell, or `None` when its `stat` could not be read.
    pub shell_sid: Option<i64>,
    /// Other PIDs in the shell's session, ascending, or `None` when `/proc` could not be listed.
    pub others: Option<Vec<i64>>,
}

/// One closed tab.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Eviction {
    /// The candidate that was closed.
    pub candidate: Candidate,
    /// Why it was judged idle.
    pub reason: String,
}

/// Order the workspace's panes least-recently-used first.
///
/// `records` must be oldest run first, as [`load_run_records`] returns them, so the LAST record
/// naming a pane is its most recent run. Panes without a record sort first; ties keep listing order.
#[must_use]
pub fn lru_candidates(panes: &[Pane], records: &[Value]) -> Vec<Candidate> {
    let mut latest: std::collections::HashMap<&str, (&str, Option<&str>)> =
        std::collections::HashMap::new();
    for record in records {
        let (Some(pane_id), Some(run_id)) = (
            record.get("pane_id").and_then(Value::as_str),
            record.get("run_id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let agent = record.get("agent").and_then(Value::as_str);
        latest.insert(pane_id, (run_id, agent));
    }
    let mut candidates = panes
        .iter()
        .map(|pane| {
            let last = latest.get(pane.pane_id.as_str());
            Candidate {
                pane_id: pane.pane_id.clone(),
                tab_id: pane.tab_id.clone(),
                last_run: last.map(|(run_id, _)| (*run_id).to_owned()),
                agent: last.and_then(|(_, agent)| agent.map(str::to_owned)),
                tab_panes: panes
                    .iter()
                    .filter(|other| other.tab_id == pane.tab_id)
                    .count(),
            }
        })
        .collect::<Vec<_>>();
    // `None` orders before `Some`, and the sort is stable.
    candidates.sort_by(|left, right| left.last_run.cmp(&right.last_run));
    candidates
}

/// Read the session ID (field 6) from the text of `/proc/<pid>/stat`.
#[must_use]
pub fn parse_session_id(stat_text: &str) -> Option<i64> {
    // The command name may itself contain spaces and parentheses; the fields resume after the
    // LAST ')'. They are then: state, ppid, pgrp, session.
    let (_, rest) = stat_text.rsplit_once(')')?;
    rest.split_whitespace().nth(3)?.parse().ok()
}

fn read_session_id(pid: i64, proc_root: &Path) -> Option<i64> {
    // Decoded lossily: a command name that is not UTF-8 must not drop its process out of the
    // session scan, because a process missing from the scan is one the tab could be closed over.
    let bytes = std::fs::read(proc_root.join(pid.to_string()).join("stat")).ok()?;
    parse_session_id(&String::from_utf8_lossy(&bytes))
}

/// Find every other process in the session `shell_pid` leads.
#[must_use]
pub fn scan_session(shell_pid: i64, proc_root: &Path) -> SessionScan {
    let shell_sid = read_session_id(shell_pid, proc_root);
    let others = if shell_sid == Some(shell_pid) {
        std::fs::read_dir(proc_root).ok().map(|entries| {
            let mut members = entries
                .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<i64>().ok())
                .filter(|pid| *pid != shell_pid)
                .filter(|pid| read_session_id(*pid, proc_root) == Some(shell_pid))
                .collect::<Vec<_>>();
            members.sort_unstable();
            members
        })
    } else {
        None
    };
    SessionScan {
        shell_pid,
        shell_sid,
        others,
    }
}

/// Combine the foreground-group verdict with the session scan into one idle verdict.
#[must_use]
pub fn judge_idle(signal: &ProcessSignal, session: &SessionScan) -> (bool, String) {
    if !signal.idle {
        return (false, signal.reason.clone());
    }
    let shell = session.shell_pid;
    match session.shell_sid {
        None => {
            return (
                false,
                format!("cannot read the session of shell {shell}; treating the tab as busy"),
            )
        }
        Some(sid) if sid != shell => {
            return (
                false,
                format!(
                "shell {shell} does not lead its session (session {sid}); treating the tab as busy"
            ),
            )
        }
        Some(_) => {}
    }
    match &session.others {
        None => (
            false,
            format!("cannot list processes in session {shell}; treating the tab as busy"),
        ),
        Some(others) if !others.is_empty() => (
            false,
            format!(
                "session {shell} still holds {} other process(es): {}",
                others.len(),
                others
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Some(_) => (
            true,
            format!("{}; no other process in session {shell}", signal.reason),
        ),
    }
}

/// Render a run ID's `YYYYMMDDTHHMMSS` prefix as an RFC 3339 UTC time.
#[must_use]
pub fn run_time(run_id: &str) -> Option<String> {
    let stamp = run_id.get(..15)?;
    let bytes = stamp.as_bytes();
    if bytes[8] != b'T'
        || !bytes[..8].iter().all(u8::is_ascii_digit)
        || !bytes[9..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}Z",
        &stamp[..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    ))
}

/// Close the least-recently-used idle tab in `workspace_id`.
///
/// Returns `Ok(Ok(eviction))` after one close, or `Ok(Err(skipped))` naming every tab that was
/// considered and why it stayed open. Only listing the panes can fail outright.
pub fn evict_one<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    workspace_id: &str,
    agent: &str,
    proc_root: &Path,
) -> Result<std::result::Result<Eviction, Vec<(String, String)>>> {
    let panes = client.panes(Some(workspace_id))?;
    let records = load_run_records(config);
    let mut skipped = Vec::new();
    for candidate in lru_candidates(&panes, &records) {
        match try_evict(client, config, &candidate, proc_root) {
            Ok(reason) => {
                let eviction = Eviction { candidate, reason };
                log_eviction(config, workspace_id, agent, &eviction);
                return Ok(Ok(eviction));
            }
            Err(reason) => skipped.push((candidate.pane_id, reason)),
        }
    }
    Ok(Err(skipped))
}

fn try_evict<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    candidate: &Candidate,
    proc_root: &Path,
) -> std::result::Result<String, String> {
    require_unsplit(candidate)?;
    let lock = pane_lock_path(&candidate.pane_id)
        .and_then(|path| open_lock_file(&path))
        .map_err(|error| format!("cannot open the pane lock: {error}"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            return Err("another herdr-run holds the pane lock".to_owned());
        }
        Err(error) => return Err(format!("cannot lock the pane: {error}")),
    }
    // Judged now, under the lock, and closed at once: never from an earlier survey.
    let reason = judge_shell(client, config, candidate, proc_root)?;
    client
        .close_tab(&candidate.tab_id)
        .map_err(|error| format!("tab close failed: {error}"))?;
    drop(lock);
    Ok(reason)
}

fn require_unsplit(candidate: &Candidate) -> std::result::Result<(), String> {
    if candidate.tab_panes != 1 {
        return Err(format!(
            "tab {} holds {} panes; only an unsplit tab is replaced",
            candidate.tab_id, candidate.tab_panes
        ));
    }
    Ok(())
}

fn judge_shell<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    candidate: &Candidate,
    proc_root: &Path,
) -> std::result::Result<String, String> {
    let info = client
        .process_info(&candidate.pane_id)
        .map_err(|error| format!("process-info failed: {error}"))?;
    let (idle, reason) = judge_idle(
        &assess_process(&info, config),
        &scan_session(info.shell_pid, proc_root),
    );
    if idle {
        Ok(reason)
    } else {
        Err(reason)
    }
}

/// What the cap path would find if a new agent arrived now.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Survey {
    /// Panes whose tab the cap path could replace, least-recently-used first.
    pub replaceable: Vec<String>,
    /// Every other pane, with the reason its tab would stay open.
    pub kept: Vec<(String, String)>,
}

/// Judge every tab as [`evict_one`] would, and close nothing.
///
/// The pane lock is not taken: a report must not make a runner wait, and the lock is held only
/// while a command runs, when the shell does not own the foreground anyway. So this is a reading
/// of the moment, not a promise; the cap path decides again, under the lock, before any close.
pub fn survey<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    panes: &[Pane],
    records: &[Value],
    proc_root: &Path,
) -> Survey {
    let mut survey = Survey::default();
    for candidate in lru_candidates(panes, records) {
        match require_unsplit(&candidate)
            .and_then(|()| judge_shell(client, config, &candidate, proc_root))
        {
            Ok(_) => survey.replaceable.push(candidate.pane_id),
            Err(reason) => survey.kept.push((candidate.pane_id, reason)),
        }
    }
    survey
}

/// [`survey`] the workspace `config` names, listed as the cap path lists it.
pub fn survey_workspace<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    records: &[Value],
    proc_root: &Path,
) -> Result<Survey> {
    let Some(workspace_id) = client.workspace_id_for_label(&config.workspace)? else {
        return Err(HerdrRunError::unavailable(format!(
            "herdr has no workspace labelled {}",
            quote(&config.workspace)
        )));
    };
    let panes = client.panes(Some(&workspace_id))?;
    Ok(survey(client, config, &panes, records, proc_root))
}

fn log_eviction(config: &Config, workspace_id: &str, agent: &str, eviction: &Eviction) {
    let candidate = &eviction.candidate;
    let last_run_at = candidate.last_run.as_deref().and_then(run_time);
    output::diagnostic(&format!(
        "herdr-run: replaced idle tab {} (pane {}, agent {}, last run {}) to make room for '{agent}'",
        candidate.tab_id,
        candidate.pane_id,
        candidate
            .agent
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown"),
        last_run_at.as_deref().unwrap_or("none recorded"),
    ));
    let mut fields = Map::new();
    fields.insert("pane_id".to_owned(), Value::from(candidate.pane_id.clone()));
    fields.insert("tab_id".to_owned(), Value::from(candidate.tab_id.clone()));
    fields.insert("workspace_id".to_owned(), Value::from(workspace_id));
    fields.insert("evicted_agent".to_owned(), candidate.agent.clone().into());
    fields.insert("last_run".to_owned(), candidate.last_run.clone().into());
    fields.insert("last_run_at".to_owned(), last_run_at.into());
    let log = audit::audit_path(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    if !audit::record(
        &log,
        agent,
        &format!("tab close {}", candidate.tab_id),
        "EVICTED",
        &eviction.reason,
        fields,
    ) {
        output::diagnostic(&format!(
            "herdr-run: WARNING: could not append audit record to {}",
            log.display()
        ));
    }
}

// --- idle lifetime: retire finished tabs after `idle_tab_ttl_seconds` ---------------------------
//
// The cap above only ever closes a tab to make room for a new one, so a workspace fills to
// `max_panes` idle shells and stays there. Retirement is the deterministic counterpart: a tab
// whose last recorded run FINISHED more than the TTL ago is closed as soon as anybody looks --
// at the end of a run, or from `sweep --idle` on a timer. Everything that makes the cap safe
// makes this safe, and it reuses those exact parts rather than growing a second notion of idle:
//
// * the candidate set is the panes a surviving run record names, in scope by the same
//   re-derivation `reap` applies ([`scope_of`]). A tab with no record is never a candidate here;
//   `reap` judges those, and the cap replaces them first;
// * a pane with ANY record lacking an exit code has work in flight and is kept -- reap's R1,
//   read the same way, over every record. `from_cache` records count in full here: the flag
//   says the TARGET came from the session cache, and the command still ran and finished in this
//   pane, which is all a finish time needs;
// * the finish is the completion marker's mtime, noted as `finished_at` by
//   [`load_run_records`]. A finish that cannot be read is unknown, and an unknown finish keeps
//   the tab;
// * the close itself is `try_evict`: unsplit tab, pane lock taken without waiting, idle verdict
//   under that lock immediately before `tab close`. Expiry is only ever the reason to LOOK; it
//   never authorises the close on its own.

/// One live pane judged against the idle TTL, in listing order.
#[derive(Clone, Debug, PartialEq)]
pub struct TtlCandidate {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Herdr tab identifier, the unit that is closed.
    pub tab_id: String,
    /// Run ID of the pane's most recent recorded run, if any record names the pane.
    pub last_run: Option<String>,
    /// Agent label of that run, if recorded.
    pub agent: Option<String>,
    /// Number of panes the listing shows in this tab.
    pub tab_panes: usize,
    /// Whether any run record names this pane at all.
    pub recorded: bool,
    /// When the most recent recorded run finished (epoch seconds), when knowable.
    pub finished_at: Option<f64>,
    /// Whether that finish is more than the TTL before `now`.
    pub expired: bool,
    /// Why the pane is or is not expired, recorded so a kept tab is never silent.
    pub reason: String,
}

/// Judge every listed pane against the idle TTL. Pure: no client, no clock of its own.
///
/// `records` must be oldest run first, as [`load_run_records`] returns them, so the LAST record
/// naming a pane is its most recent run. A pane is expired only when every record naming it
/// recorded an exit code and the most recent one's `finished_at` is a finite number more than
/// `ttl_seconds` before `now` — exactly at the boundary is within the TTL. Scope is not judged
/// here; [`retire_idle_tabs`] applies it before anything may close.
#[must_use]
pub fn ttl_candidates(
    panes: &[Pane],
    records: &[Value],
    now: f64,
    ttl_seconds: f64,
) -> Vec<TtlCandidate> {
    panes
        .iter()
        .map(|pane| {
            let mine = records
                .iter()
                .filter(|record| {
                    record.get("pane_id").and_then(Value::as_str) == Some(pane.pane_id.as_str())
                })
                .collect::<Vec<_>>();
            let tab_panes = panes
                .iter()
                .filter(|other| other.tab_id == pane.tab_id)
                .count();
            let Some(last) = mine.last() else {
                return TtlCandidate {
                    pane_id: pane.pane_id.clone(),
                    tab_id: pane.tab_id.clone(),
                    last_run: None,
                    agent: None,
                    tab_panes,
                    recorded: false,
                    finished_at: None,
                    expired: false,
                    reason: "no recorded run".to_owned(),
                };
            };
            let finished_at = last
                .get("finished_at")
                .and_then(Value::as_f64)
                .filter(|finished| finished.is_finite());
            let unfinished = mine
                .iter()
                .filter(|record| matches!(record.get("exit_code"), None | Some(Value::Null)))
                .count();
            let (expired, reason) = if unfinished > 0 {
                (
                    false,
                    format!("{unfinished} run(s) for this pane have no recorded exit_code"),
                )
            } else if finished_at.is_none() {
                (
                    false,
                    "the finish time of this pane's last recorded run is unknown".to_owned(),
                )
            } else if ttl_seconds <= 0.0 {
                (false, "the idle TTL is disabled".to_owned())
            } else if now - finished_at.expect("finish checked above") > ttl_seconds {
                (true, "last run finished more than the idle TTL ago".to_owned())
            } else {
                (false, "last run finished within the idle TTL".to_owned())
            };
            TtlCandidate {
                pane_id: pane.pane_id.clone(),
                tab_id: pane.tab_id.clone(),
                last_run: last
                    .get("run_id")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                agent: last
                    .get("agent")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                tab_panes,
                recorded: true,
                finished_at,
                expired,
                reason,
            }
        })
        .collect()
}

/// One tab an idle sweep closed (or, in a dry run, would close), and why it may close.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetiredTab {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Herdr tab identifier.
    pub tab_id: String,
    /// Agent label of the pane's most recent recorded run, if recorded.
    pub agent: Option<String>,
    /// Run ID of that run, if recorded.
    pub last_run: Option<String>,
    /// Why the tab was judged idle.
    pub reason: String,
}

/// What one idle sweep did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdleSweep {
    /// Tabs actually closed, oldest finish first.
    pub closed: Vec<RetiredTab>,
    /// Tabs a dry run would have closed, oldest finish first.
    pub would_close: Vec<RetiredTab>,
    /// Recorded panes left open, in listing order, each with its reason.
    pub kept: Vec<(String, String)>,
    /// Why herdr's workspace listing failed, when it did; nothing was closed.
    pub listing_error: Option<String>,
}

/// Close every tab in `config`'s workspace that is idle past the TTL.
///
/// Expired tabs are examined oldest finish first. `limit` bounds how many expired tabs one sweep
/// EXAMINES (each examination probes a pane and scans `/proc`); the opportunistic sweep at the
/// end of a run passes a small one so housekeeping can never stretch a caller's command, and
/// `exclude_pane_id` keeps that sweep from closing the very tab the run just used. A dry run
/// judges shells exactly as [`survey`] does — without the pane lock — and closes nothing.
pub fn retire_idle_tabs<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    actor: &str,
    now: f64,
    dry_run: bool,
    limit: Option<usize>,
    exclude_pane_id: Option<&str>,
    proc_root: &Path,
) -> IdleSweep {
    let listing = client
        .workspace_id_for_label(&config.workspace)
        .and_then(|found| match found {
            Some(workspace_id) => client
                .panes(Some(&workspace_id))
                .map(|panes| (workspace_id, panes)),
            None => Err(HerdrRunError::unavailable(format!(
                "herdr has no workspace labelled {}",
                quote(&config.workspace)
            ))),
        });
    let (workspace_id, panes) = match listing {
        Ok(listed) => listed,
        Err(error) => {
            return IdleSweep {
                listing_error: Some(error.to_string()),
                ..IdleSweep::default()
            }
        }
    };
    let records = load_run_records(config);
    let verdicts = ttl_candidates(&panes, &records, now, config.idle_tab_ttl_seconds);

    // First pass: scope and incarnation, which need no probing. Outcomes are keyed by pane so
    // the kept list can be emitted in listing order even though expired tabs are examined in
    // finish order.
    let mut kept_reasons: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut expired: Vec<&TtlCandidate> = Vec::new();
    for verdict in &verdicts {
        if !verdict.recorded || exclude_pane_id == Some(verdict.pane_id.as_str()) {
            continue;
        }
        let (in_scope, recorded_tab, tab_label, workspace_label) =
            scope_of(&verdict.pane_id, &records, config);
        if !in_scope {
            kept_reasons.insert(
                verdict.pane_id.clone(),
                format!(
                    "tab {} in workspace {} is not one of ours",
                    quote_optional(tab_label.as_deref()),
                    quote_optional(workspace_label.as_deref())
                ),
            );
            continue;
        }
        if let Some(recorded_tab) = recorded_tab {
            if recorded_tab != verdict.tab_id {
                kept_reasons.insert(
                    verdict.pane_id.clone(),
                    format!(
                        "pane was recorded in tab {} but is now in tab {}; refusing to guess",
                        quote(&recorded_tab),
                        quote(&verdict.tab_id)
                    ),
                );
                continue;
            }
        }
        if verdict.expired {
            expired.push(verdict);
        } else {
            kept_reasons.insert(verdict.pane_id.clone(), verdict.reason.clone());
        }
    }
    expired.sort_by(|left, right| {
        left.finished_at
            .unwrap_or(0.0)
            .partial_cmp(&right.finished_at.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut closed = Vec::new();
    let mut would_close = Vec::new();
    let mut examined = 0usize;
    for verdict in expired {
        if limit.is_some_and(|limit| examined >= limit) {
            kept_reasons.insert(
                verdict.pane_id.clone(),
                format!(
                    "not examined: this sweep's limit of {} tab(s) was reached",
                    limit.expect("limit checked above")
                ),
            );
            continue;
        }
        examined += 1;
        let candidate = Candidate {
            pane_id: verdict.pane_id.clone(),
            tab_id: verdict.tab_id.clone(),
            last_run: verdict.last_run.clone(),
            agent: verdict.agent.clone(),
            tab_panes: verdict.tab_panes,
        };
        let retired = |reason: String| RetiredTab {
            pane_id: verdict.pane_id.clone(),
            tab_id: verdict.tab_id.clone(),
            agent: verdict.agent.clone(),
            last_run: verdict.last_run.clone(),
            reason,
        };
        if dry_run {
            match require_unsplit(&candidate)
                .and_then(|()| judge_shell(client, config, &candidate, proc_root))
            {
                Ok(reason) => would_close.push(retired(reason)),
                Err(reason) => {
                    kept_reasons.insert(verdict.pane_id.clone(), reason);
                }
            }
            continue;
        }
        match try_evict(client, config, &candidate, proc_root) {
            Ok(reason) => {
                let tab = retired(reason);
                log_retirement(config, &workspace_id, actor, &tab);
                closed.push(tab);
            }
            Err(reason) => {
                kept_reasons.insert(verdict.pane_id.clone(), reason);
            }
        }
    }

    let kept = verdicts
        .iter()
        .filter_map(|verdict| {
            kept_reasons
                .get(&verdict.pane_id)
                .map(|reason| (verdict.pane_id.clone(), reason.clone()))
        })
        .collect();
    IdleSweep {
        closed,
        would_close,
        kept,
        listing_error: None,
    }
}

fn log_retirement(config: &Config, workspace_id: &str, actor: &str, retired: &RetiredTab) {
    let last_run_at = retired.last_run.as_deref().and_then(run_time);
    output::diagnostic(&format!(
        "herdr-run: retired idle tab {} (pane {}, agent {}, last run {}) idle past the {}s TTL",
        retired.tab_id,
        retired.pane_id,
        retired
            .agent
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown"),
        last_run_at.as_deref().unwrap_or("none recorded"),
        number_text(config.idle_tab_ttl_seconds),
    ));
    let mut fields = Map::new();
    fields.insert("pane_id".to_owned(), Value::from(retired.pane_id.clone()));
    fields.insert("tab_id".to_owned(), Value::from(retired.tab_id.clone()));
    fields.insert("workspace_id".to_owned(), Value::from(workspace_id));
    fields.insert("retired_agent".to_owned(), retired.agent.clone().into());
    fields.insert("last_run".to_owned(), retired.last_run.clone().into());
    fields.insert("last_run_at".to_owned(), last_run_at.into());
    let log = audit::audit_path(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    if !audit::record(
        &log,
        actor,
        &format!("tab close {}", retired.tab_id),
        "RETIRED",
        &retired.reason,
        fields,
    ) {
        output::diagnostic(&format!(
            "herdr-run: WARNING: could not append audit record to {}",
            log.display()
        ));
    }
}

/// Describe the tabs that stayed open, for the cap refusal.
#[must_use]
pub fn describe_skipped(skipped: &[(String, String)]) -> String {
    let mut parts = skipped
        .iter()
        .take(REFUSAL_DETAIL_LIMIT)
        .map(|(pane, reason)| format!("pane {pane}: {reason}"))
        .collect::<Vec<_>>();
    if skipped.len() > REFUSAL_DETAIL_LIMIT {
        parts.push(format!("and {} more", skipped.len() - REFUSAL_DETAIL_LIMIT));
    }
    format!(
        "None of its {} tab(s) could be replaced: {}.",
        skipped.len(),
        parts.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use serde_json::json;

    use crate::client::ProcessInfo;
    use crate::error::HerdrRunError;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    const CASES: &str = include_str!("../testdata/eviction_cases.json");

    fn cases() -> Value {
        serde_json::from_str(CASES).expect("golden eviction cases")
    }

    fn temporary_root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "herdr-run-evict-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary root");
        path
    }

    /// A pane ID no live Herdr pane can carry, so the account-global pane lock is ours alone.
    fn unique_pane(name: &str) -> String {
        format!(
            "evict-test-{}-{}-{name}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn write_stat(proc_root: &Path, pid: i64, sid: i64) {
        let directory = proc_root.join(pid.to_string());
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("stat"),
            format!("{pid} (proc {pid}) S 1 {pid} {sid} 0 0 0"),
        )
        .unwrap();
    }

    fn write_record(project: &Path, run_id: &str, pane_id: &str, agent: &str) {
        let directory = project.join(".herdr-run").join("runs").join(run_id);
        fs::create_dir_all(&directory).unwrap();
        let record = json!({"agent": agent, "pane_id": pane_id, "run_id": run_id, "exit_code": 0});
        fs::write(directory.join("meta.json"), record.to_string()).unwrap();
    }

    fn config(project: &Path) -> Config {
        Config {
            project_root: project.to_string_lossy().into_owned(),
            ..Config::default()
        }
    }

    #[derive(Default)]
    struct Fake {
        panes: Mutex<Vec<Pane>>,
        /// Pane -> shell PID for an idle shell; any other pane is running `git push`.
        idle: Mutex<BTreeMap<String, i64>>,
        closed: Mutex<Vec<String>>,
        probed: Mutex<Vec<String>>,
        failing_close: Mutex<BTreeSet<String>>,
    }

    impl Fake {
        fn add(&self, pane: &str, tab: &str, idle_shell: Option<i64>) {
            self.panes.lock().unwrap().push(Pane {
                pane_id: pane.to_owned(),
                tab_id: tab.to_owned(),
                workspace_id: "w1".to_owned(),
            });
            if let Some(pid) = idle_shell {
                self.idle.lock().unwrap().insert(pane.to_owned(), pid);
            }
        }

        fn closed(&self) -> Vec<String> {
            self.closed.lock().unwrap().clone()
        }

        fn probed(&self) -> Vec<String> {
            self.probed.lock().unwrap().clone()
        }
    }

    impl HerdrApi for Fake {
        fn ensure_server(&self) -> Result<bool> {
            Ok(false)
        }
        fn server_running(&self) -> bool {
            true
        }
        fn workspace_id_for_label(&self, _label: &str) -> Result<Option<String>> {
            Ok(Some("w1".to_owned()))
        }
        fn create_workspace(&self, _label: &str, _cwd: &str) -> Result<(String, String, String)> {
            unreachable!()
        }
        fn tab_id_for_label(&self, _workspace_id: &str, _label: &str) -> Result<Option<String>> {
            unreachable!()
        }
        fn create_tab(&self, _workspace_id: &str, _label: &str, _cwd: &str) -> Result<String> {
            unreachable!()
        }
        fn rename_tab(&self, _tab_id: &str, _label: &str) -> Result<()> {
            unreachable!()
        }
        fn close_tab(&self, tab_id: &str) -> Result<()> {
            if self.failing_close.lock().unwrap().contains(tab_id) {
                return Err(HerdrRunError::unavailable(
                    "tab close: herdr is not answering",
                ));
            }
            self.closed.lock().unwrap().push(tab_id.to_owned());
            self.panes
                .lock()
                .unwrap()
                .retain(|pane| pane.tab_id != tab_id);
            Ok(())
        }
        fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>> {
            assert_eq!(
                workspace_id,
                Some("w1"),
                "only the configured workspace is listed"
            );
            Ok(self.panes.lock().unwrap().clone())
        }
        fn pane_exists(&self, pane_id: &str) -> bool {
            self.panes
                .lock()
                .unwrap()
                .iter()
                .any(|pane| pane.pane_id == pane_id)
        }
        fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
            self.probed.lock().unwrap().push(pane_id.to_owned());
            Ok(match self.idle.lock().unwrap().get(pane_id) {
                Some(pid) => ProcessInfo {
                    pane_id: pane_id.to_owned(),
                    shell_pid: *pid,
                    foreground_pgid: *pid,
                    foreground: vec![(*pid, "bash".to_owned(), "/bin/bash".to_owned())],
                },
                None => ProcessInfo {
                    pane_id: pane_id.to_owned(),
                    shell_pid: 9000,
                    foreground_pgid: 9001,
                    foreground: vec![(9001, "git".to_owned(), "git push".to_owned())],
                },
            })
        }
        fn read(&self, _pane_id: &str, _source: &str, _lines: Option<usize>) -> Result<String> {
            unreachable!()
        }
        fn run(&self, _pane_id: &str, _command: &str) -> Result<()> {
            unreachable!()
        }
    }

    fn candidate_json(candidate: &Candidate) -> Value {
        json!({
            "pane_id": candidate.pane_id,
            "tab_id": candidate.tab_id,
            "last_run": candidate.last_run,
            "agent": candidate.agent,
            "tab_panes": candidate.tab_panes,
        })
    }

    #[test]
    fn golden_lru_order_matches_the_shared_cases() {
        for case in cases()["order"].as_array().unwrap() {
            let panes = case["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| Pane {
                    pane_id: pair[0].as_str().unwrap().to_owned(),
                    tab_id: pair[1].as_str().unwrap().to_owned(),
                    workspace_id: "w1".to_owned(),
                })
                .collect::<Vec<_>>();
            let records = case["records"].as_array().unwrap().clone();
            let actual = lru_candidates(&panes, &records)
                .iter()
                .map(candidate_json)
                .collect::<Vec<_>>();
            assert_eq!(Value::from(actual), case["expected"], "{}", case["name"]);
        }
    }

    #[test]
    fn golden_idle_verdicts_match_the_shared_cases() {
        let config = Config::default();
        for case in cases()["idle"].as_array().unwrap() {
            let process = &case["process"];
            let info = ProcessInfo {
                pane_id: "p1".to_owned(),
                shell_pid: process["shell_pid"].as_i64().unwrap(),
                foreground_pgid: process["foreground_pgid"].as_i64().unwrap(),
                foreground: process["foreground"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|entry| {
                        (
                            entry[0].as_i64().unwrap(),
                            entry[1].as_str().unwrap().to_owned(),
                            entry[2].as_str().unwrap().to_owned(),
                        )
                    })
                    .collect(),
            };
            let session = SessionScan {
                shell_pid: info.shell_pid,
                shell_sid: case["session"]["shell_sid"].as_i64(),
                others: case["session"]["others"]
                    .as_array()
                    .map(|values| values.iter().map(|value| value.as_i64().unwrap()).collect()),
            };
            let (idle, reason) = judge_idle(&assess_process(&info, &config), &session);
            assert_eq!(
                json!({"idle": idle, "reason": reason}),
                case["expected"],
                "{}",
                case["name"]
            );
        }
    }

    #[test]
    fn golden_helpers_match_the_shared_cases() {
        let cases = cases();
        for pair in cases["run_time"].as_array().unwrap() {
            let run_id = pair[0].as_str().unwrap();
            assert_eq!(Value::from(run_time(run_id)), pair[1], "{run_id}");
        }
        for pair in cases["session_id"].as_array().unwrap() {
            let stat = pair[0].as_str().unwrap();
            assert_eq!(Value::from(parse_session_id(stat)), pair[1], "{stat}");
        }
        for case in cases["refusal"].as_array().unwrap() {
            let skipped = case["skipped"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| {
                    (
                        pair[0].as_str().unwrap().to_owned(),
                        pair[1].as_str().unwrap().to_owned(),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                describe_skipped(&skipped),
                case["expected"].as_str().unwrap()
            );
        }
    }

    fn ttl_candidate_json(candidate: &TtlCandidate) -> Value {
        json!({
            "pane_id": candidate.pane_id,
            "tab_id": candidate.tab_id,
            "last_run": candidate.last_run,
            "agent": candidate.agent,
            "tab_panes": candidate.tab_panes,
            "recorded": candidate.recorded,
            "finished_at": candidate.finished_at,
            "expired": candidate.expired,
            "reason": candidate.reason,
        })
    }

    #[test]
    fn golden_idle_ttl_verdicts_match_the_shared_cases() {
        for case in cases()["ttl"].as_array().unwrap() {
            let panes = case["panes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pair| Pane {
                    pane_id: pair[0].as_str().unwrap().to_owned(),
                    tab_id: pair[1].as_str().unwrap().to_owned(),
                    workspace_id: "w1".to_owned(),
                })
                .collect::<Vec<_>>();
            let records = case["records"].as_array().unwrap().clone();
            let actual = ttl_candidates(
                &panes,
                &records,
                case["now"].as_f64().unwrap(),
                case["ttl_seconds"].as_f64().unwrap(),
            )
            .iter()
            .map(ttl_candidate_json)
            .collect::<Vec<_>>();
            assert_eq!(Value::from(actual), case["expected"], "{}", case["name"]);
        }
    }

    /// Plant one finished run: a record in scope for the default config, plus its completion
    /// marker aged to `finished_at` epoch seconds.
    fn write_finished(
        project: &Path,
        run_id: &str,
        pane_id: &str,
        tab_id: &str,
        agent: &str,
        finished_at: f64,
        exit_code: Option<i64>,
        marker: bool,
    ) {
        let directory = project.join(".herdr-run").join("runs").join(run_id);
        fs::create_dir_all(&directory).unwrap();
        let record = json!({
            "agent": agent,
            "pane_id": pane_id,
            "run_id": run_id,
            "exit_code": exit_code,
            "tab": {"id": tab_id, "label": agent},
            "workspace": {"id": "w1", "label": "agent-cmds"},
        });
        fs::write(directory.join("meta.json"), record.to_string()).unwrap();
        if marker {
            let path = directory.join("exit_code");
            fs::write(&path, format!("{}\n", exit_code.unwrap_or(0))).unwrap();
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(finished_at);
            fs::File::open(&path).unwrap().set_modified(when).unwrap();
        }
    }

    const NOW: f64 = 1_800_000_000.0;

    #[test]
    fn an_expired_idle_tab_is_retired_and_a_busy_one_is_not() {
        let world = World::new("ttl-close");
        let old = world.tab("old", Some(101));
        let busy = world.tab("busy", None);
        let fresh = world.tab("fresh", Some(103));
        write_finished(&world.project, "r1", &old, &format!("tab-{old}"), "alpha", NOW - 901.0, Some(0), true);
        write_finished(&world.project, "r2", &busy, &format!("tab-{busy}"), "beta", NOW - 5000.0, Some(0), true);
        write_finished(&world.project, "r3", &fresh, &format!("tab-{fresh}"), "gamma", NOW - 899.0, Some(0), true);

        let sweep = retire_idle_tabs(&world.fake, &world.config, "tester", NOW, false, None, None, &world.proc_root);
        assert_eq!(sweep.closed.len(), 1);
        assert_eq!(sweep.closed[0].pane_id, old);
        assert_eq!(world.fake.closed(), vec![format!("tab-{old}")]);
        let kept: std::collections::HashMap<_, _> = sweep.kept.into_iter().collect();
        assert!(kept[&busy].contains("running: git"), "{}", kept[&busy]);
        assert_eq!(kept[&fresh], "last run finished within the idle TTL");
        let audit = world.audit();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["verdict"], "RETIRED");
        assert_eq!(audit[0]["retired_agent"], "alpha");
    }

    #[test]
    fn retirement_honours_the_boundary_unfinished_runs_dry_runs_and_zero_ttl() {
        let world = World::new("ttl-guards");
        let edge = world.tab("edge", Some(101));
        let thinking = world.tab("thinking", Some(102));
        let old = world.tab("old", Some(103));
        write_finished(&world.project, "r1", &edge, &format!("tab-{edge}"), "alpha", NOW - 900.0, Some(0), true);
        write_finished(&world.project, "r2", &thinking, &format!("tab-{thinking}"), "beta", NOW - 5000.0, None, false);
        write_finished(&world.project, "r3", &old, &format!("tab-{old}"), "gamma", NOW - 5000.0, Some(0), true);

        let dry = retire_idle_tabs(&world.fake, &world.config, "tester", NOW, true, None, None, &world.proc_root);
        assert!(dry.closed.is_empty());
        assert_eq!(dry.would_close.len(), 1);
        assert_eq!(dry.would_close[0].pane_id, old);
        assert!(world.fake.closed().is_empty());
        let kept: std::collections::HashMap<_, _> = dry.kept.into_iter().collect();
        assert_eq!(kept[&edge], "last run finished within the idle TTL");
        assert_eq!(kept[&thinking], "1 run(s) for this pane have no recorded exit_code");

        let disabled = Config {
            idle_tab_ttl_seconds: 0.0,
            ..world.config.clone()
        };
        let sweep = retire_idle_tabs(&world.fake, &disabled, "tester", NOW, false, None, None, &world.proc_root);
        assert!(sweep.closed.is_empty());
        assert!(sweep.kept.iter().any(|(pane, reason)| pane == &old && reason == "the idle TTL is disabled"));
    }

    #[test]
    fn session_scan_lists_other_members_in_order_and_fails_closed() {
        let proc_root = temporary_root("scan");
        write_stat(&proc_root, 100, 100);
        write_stat(&proc_root, 205, 100);
        write_stat(&proc_root, 201, 100);
        write_stat(&proc_root, 300, 300);
        fs::create_dir_all(proc_root.join("self")).unwrap();
        assert_eq!(
            scan_session(100, &proc_root),
            SessionScan {
                shell_pid: 100,
                shell_sid: Some(100),
                others: Some(vec![201, 205]),
            }
        );
        assert_eq!(scan_session(300, &proc_root).others, Some(vec![]));
        // A shell whose stat is unreadable, or that is not its own session leader, is never idle.
        assert_eq!(scan_session(999, &proc_root).shell_sid, None);
        write_stat(&proc_root, 400, 100);
        assert_eq!(scan_session(400, &proc_root).others, None);
        fs::remove_dir_all(proc_root).unwrap();
    }

    struct World {
        project: PathBuf,
        proc_root: PathBuf,
        config: Config,
        fake: Fake,
    }

    impl World {
        fn new(name: &str) -> Self {
            let project = temporary_root(name);
            let proc_root = project.join("proc");
            fs::create_dir_all(&proc_root).unwrap();
            let config = config(&project);
            Self {
                project,
                proc_root,
                config,
                fake: Fake::default(),
            }
        }

        /// Add a one-pane tab; `idle_shell` plants a session-leading shell with that PID.
        fn tab(&self, name: &str, idle_shell: Option<i64>) -> String {
            let pane = unique_pane(name);
            if let Some(pid) = idle_shell {
                write_stat(&self.proc_root, pid, pid);
            }
            self.fake.add(&pane, &format!("tab-{pane}"), idle_shell);
            pane
        }

        fn evict(&self) -> std::result::Result<Eviction, Vec<(String, String)>> {
            evict_one(&self.fake, &self.config, "w1", "newcomer", &self.proc_root).unwrap()
        }

        fn audit(&self) -> Vec<Value> {
            let path = audit::audit_path(&self.project, Path::new(".herdr-run"));
            fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    impl Drop for World {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.project);
        }
    }

    #[test]
    fn the_least_recently_used_idle_tab_is_closed_and_logged() {
        let world = World::new("lru");
        let newest = world.tab("newest", Some(101));
        let oldest = world.tab("oldest", Some(102));
        let middle = world.tab("middle", Some(103));
        write_record(
            &world.project,
            "20261001T080000-old-1",
            &oldest,
            "old-agent",
        );
        write_record(
            &world.project,
            "20261001T090000-mid-2",
            &middle,
            "mid-agent",
        );
        write_record(
            &world.project,
            "20261001T100000-new-3",
            &newest,
            "new-agent",
        );

        let eviction = world.evict().expect("an idle tab");
        assert_eq!(eviction.candidate.pane_id, oldest);
        assert_eq!(world.fake.closed(), [format!("tab-{oldest}")]);
        let audit = world.audit();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0]["verdict"], "EVICTED");
        assert_eq!(audit[0]["agent"], "newcomer");
        assert_eq!(audit[0]["evicted_agent"], "old-agent");
        assert_eq!(audit[0]["pane_id"], Value::from(oldest.clone()));
        assert_eq!(audit[0]["last_run_at"], "2026-10-01T08:00:00Z");
        assert_eq!(
            audit[0]["command"],
            Value::from(format!("tab close tab-{oldest}"))
        );

        assert_eq!(world.evict().expect("next").candidate.pane_id, middle);
        assert_eq!(world.evict().expect("last").candidate.pane_id, newest);
    }

    #[test]
    fn panes_with_no_record_are_replaced_before_recorded_ones() {
        let world = World::new("unrecorded");
        let recorded = world.tab("recorded", Some(111));
        let unrecorded = world.tab("unrecorded", Some(112));
        write_record(
            &world.project,
            "20200101T000000-ancient-1",
            &recorded,
            "ancient",
        );
        assert_eq!(world.evict().expect("idle").candidate.pane_id, unrecorded);
        assert_eq!(world.audit()[0]["last_run_at"], Value::Null);
    }

    #[test]
    fn a_tab_running_a_command_is_never_closed() {
        let world = World::new("busy");
        let busy = world.tab("busy", None);
        let idle = world.tab("idle", Some(121));
        write_record(&world.project, "20261001T080000-a-1", &busy, "a");
        write_record(&world.project, "20261001T090000-b-2", &idle, "b");
        let eviction = world.evict().expect("the idle tab");
        assert_eq!(eviction.candidate.pane_id, idle);
        assert_eq!(world.fake.closed(), [format!("tab-{idle}")]);
    }

    #[test]
    fn every_tab_busy_refuses_and_closes_nothing() {
        let world = World::new("all-busy");
        let first = world.tab("first", None);
        let second = world.tab("second", None);
        let skipped = world.evict().expect_err("nothing is idle");
        let panes = skipped
            .iter()
            .map(|(pane, _)| pane.clone())
            .collect::<Vec<_>>();
        assert_eq!(panes, [first, second]);
        assert!(skipped[0].1.contains("running: git"), "{skipped:?}");
        assert!(world.fake.closed().is_empty());
        assert!(world.audit().is_empty());
    }

    #[test]
    fn a_background_job_in_the_shell_session_keeps_the_tab_open() {
        let world = World::new("background");
        let pane = world.tab("background", Some(131));
        write_stat(&world.proc_root, 132, 131);
        let skipped = world.evict().expect_err("background job");
        assert_eq!(
            skipped,
            [(
                pane,
                "session 131 still holds 1 other process(es): 132".to_owned()
            )]
        );
        assert!(world.fake.closed().is_empty());
    }

    #[test]
    fn a_pane_reserved_by_another_herdr_run_is_skipped_without_probing() {
        let world = World::new("locked");
        let locked = world.tab("locked", Some(141));
        let free = world.tab("free", Some(142));
        write_record(&world.project, "20261001T090000-b-2", &free, "b");
        let lock = open_lock_file(&pane_lock_path(&locked).unwrap()).unwrap();
        lock.try_lock_exclusive().unwrap();
        let eviction = world.evict().expect("the free tab");
        assert_eq!(eviction.candidate.pane_id, free);
        // The verdict is taken under the lock: a reserved pane is never even probed.
        assert_eq!(world.fake.probed(), [free]);
        drop(lock);
    }

    #[test]
    fn a_split_tab_is_never_closed() {
        let world = World::new("split");
        let pane = unique_pane("split-a");
        let sibling = unique_pane("split-b");
        write_stat(&world.proc_root, 151, 151);
        world.fake.add(&pane, "split-tab", Some(151));
        world.fake.add(&sibling, "split-tab", Some(151));
        let skipped = world.evict().expect_err("split");
        assert!(skipped[0].1.contains("holds 2 panes"), "{skipped:?}");
        assert!(world.fake.probed().is_empty());
    }

    #[test]
    fn a_failed_close_moves_on_to_the_next_idle_tab() {
        let world = World::new("close-fails");
        let stuck = world.tab("stuck", Some(161));
        let next = world.tab("next", Some(162));
        write_record(&world.project, "20261001T090000-b-2", &next, "b");
        world
            .fake
            .failing_close
            .lock()
            .unwrap()
            .insert(format!("tab-{stuck}"));
        assert_eq!(world.evict().expect("next").candidate.pane_id, next);
    }

    #[test]
    fn the_survey_judges_every_tab_as_the_cap_does_and_closes_nothing() {
        let world = World::new("survey");
        let busy = world.tab("busy", None);
        let newer_idle = world.tab("newer-idle", Some(171));
        let background = world.tab("background", Some(172));
        write_stat(&world.proc_root, 173, 172);
        let older_idle = world.tab("older-idle", Some(174));
        let split = unique_pane("survey-split");
        world.fake.add(&split, "survey-split-tab", Some(175));
        world.fake.add(
            &unique_pane("survey-split-b"),
            "survey-split-tab",
            Some(175),
        );
        write_record(&world.project, "20261001T080000-a-1", &busy, "a");
        write_record(&world.project, "20261001T090000-b-2", &older_idle, "b");
        write_record(&world.project, "20261001T100000-c-3", &newer_idle, "c");

        let panes = world.fake.panes(Some("w1")).unwrap();
        let records = load_run_records(&world.config);
        let survey = survey(
            &world.fake,
            &world.config,
            &panes,
            &records,
            &world.proc_root,
        );
        assert_eq!(survey.replaceable, [older_idle.clone(), newer_idle]);
        let kept = survey
            .kept
            .iter()
            .map(|(pane, _)| pane.clone())
            .collect::<Vec<_>>();
        assert!(kept.contains(&busy) && kept.contains(&background) && kept.contains(&split));
        assert_eq!(survey.kept.len(), 4, "{:?}", survey.kept);
        assert!(world.fake.closed().is_empty());
        assert!(world.audit().is_empty());
        // The first replaceable tab is the one the cap closes.
        assert_eq!(world.evict().expect("idle").candidate.pane_id, older_idle);
    }

    #[test]
    fn the_survey_neither_waits_for_nor_takes_a_pane_lock() {
        let world = World::new("survey-locked");
        let locked = world.tab("locked", Some(181));
        let lock = open_lock_file(&pane_lock_path(&locked).unwrap()).unwrap();
        lock.try_lock_exclusive().unwrap();
        let panes = world.fake.panes(Some("w1")).unwrap();
        let survey = survey(&world.fake, &world.config, &panes, &[], &world.proc_root);
        assert_eq!(survey.replaceable, std::slice::from_ref(&locked));
        assert_eq!(world.fake.probed(), [locked]);
        drop(lock);
    }

    #[test]
    fn surveying_an_all_busy_workspace_yields_the_cap_refusal() {
        let world = World::new("survey-busy");
        world.tab("first", None);
        world.tab("second", None);
        let survey = survey_workspace(&world.fake, &world.config, &[], &world.proc_root).unwrap();
        assert!(survey.replaceable.is_empty());
        let refusal = describe_skipped(&survey.kept);
        assert!(
            refusal.starts_with("None of its 2 tab(s) could be replaced: pane "),
            "{refusal}"
        );
        assert!(refusal.contains("running: git"), "{refusal}");
    }
}
