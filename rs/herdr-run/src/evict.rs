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
        match try_evict(client, config, &candidate, workspace_id, None, proc_root) {
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

/// An under-lock re-judgement: given the pane as re-listed, a refusal reason or none.
type Recheck<'a> = &'a dyn Fn(&Pane) -> Option<String>;

fn try_evict<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    candidate: &Candidate,
    workspace_id: &str,
    recheck: Option<Recheck<'_>>,
    proc_root: &Path,
) -> std::result::Result<String, String> {
    let (reason, lock) = claim_idle(client, config, candidate, workspace_id, recheck, proc_root)?;
    client
        .close_tab(&candidate.tab_id)
        .map_err(|error| format!("tab close failed: {error}"))?;
    drop(lock);
    Ok(reason)
}

/// Take the candidate's pane lock without waiting and judge the tab idle under it.
///
/// On success the lock is returned still HELD, with the idle verdict: the caller acts on the
/// tab (closes it, or renames it for another agent) before letting go, so nothing can start in
/// the pane between the verdict and the action.
fn claim_idle<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    candidate: &Candidate,
    workspace_id: &str,
    recheck: Option<Recheck<'_>>,
    proc_root: &Path,
) -> std::result::Result<(String, std::fs::File), String> {
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
    // That includes the TAB's identity, not just the shell's: the listing the candidate
    // came from may be seconds old, and process-info names a pane and its shell, not the
    // tab `tab close` is about to destroy. Re-read the listing first: the pane may have
    // been split (a sibling this verdict never judged would die with the tab) or moved
    // to another tab (the close would land on a tab nobody judged).
    let listed = client
        .panes(Some(workspace_id))
        .map_err(|error| format!("herdr could not list panes before closing: {error}"))?;
    let Some(current) = listed.iter().find(|pane| pane.pane_id == candidate.pane_id) else {
        return Err("herdr reports no such pane".to_owned());
    };
    if current.tab_id != candidate.tab_id {
        return Err(format!(
            "pane was in tab {} when listed but is now in tab {}; refusing to guess",
            quote(&candidate.tab_id),
            quote(&current.tab_id)
        ));
    }
    let fresh = Candidate {
        pane_id: candidate.pane_id.clone(),
        tab_id: candidate.tab_id.clone(),
        last_run: candidate.last_run.clone(),
        agent: candidate.agent.clone(),
        tab_panes: listed
            .iter()
            .filter(|pane| pane.tab_id == current.tab_id)
            .count(),
    };
    require_unsplit(&fresh)?;
    if let Some(recheck) = recheck {
        if let Some(refusal) = recheck(current) {
            return Err(refusal);
        }
    }
    let reason = judge_shell(client, config, &fresh, proc_root)?;
    Ok((reason, lock))
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
    let signal = assess_process(&info, config);
    if !signal.idle {
        // The session scan reads every process's `stat`; a shell that does not even own the
        // foreground is busy whatever it would find.
        return Err(signal.reason);
    }
    let (idle, reason) = judge_idle(&signal, &scan_session(info.shell_pid, proc_root));
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

// --- idle retention: retire finished tabs past `idle_tab_ttl_seconds` or `max_idle_tabs` -------
//
// The cap above only ever closes a tab to make room for a new one, so a workspace fills to
// `max_panes` idle shells and stays there. Retirement is the deterministic counterpart, on two
// bounds: a tab whose last recorded run FINISHED more than the TTL ago is closed, and past
// `max_idle_tabs` idle tabs the least recently used ones are closed however recent they are --
// at the end of every run, or from `sweep --idle` on a timer. Everything that makes the cap safe
// makes this safe, and it reuses those exact parts rather than growing a second notion of idle:
//
// * the candidate set is the panes a surviving run record names, in scope by the same
//   re-derivation `reap` applies ([`scope_of`]). A tab with no record is never a candidate here;
//   `reap` judges those, and the cap replaces them first;
// * a pane whose latest records lack an exit code has work in flight and is kept -- reap's R1,
//   read over the records after the pane's last finished run (every run is recorded in flight
//   the moment it is typed; a later finished run proves the earlier ones ended). `from_cache`
//   records count in full here: the flag
//   says the TARGET came from the session cache, and the command still ran and finished in this
//   pane, which is all a finish time needs. Such a pane is never COUNTED against
//   `max_idle_tabs` either, so the idle cap never limits how many commands run at once;
// * the finish is the completion marker's mtime, noted as `finished_at` by
//   [`load_run_records`]. A finish that cannot be read is unknown, and an unknown finish keeps
//   the tab and keeps it out of the count;
// * the close itself is `try_evict`: unsplit tab, pane lock taken without waiting, idle verdict
//   under that lock immediately before `tab close`. Expiry or rank is only ever the reason to
//   LOOK; it never authorises the close on its own.
//
// The idle count is read from the records, not probed: the most recently used `max_idle_tabs`
// settled tabs are kept without a control call, and only the ones past them are probed, so the
// sweep at the end of a run costs one listing plus a probe per tab it actually closes.

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
    /// Whether every recorded run finished and the last finish is known: the tab counts as idle
    /// for `max_idle_tabs` (subject to scope) and may be ranked by how recently it was used.
    pub settled: bool,
    /// Whether that finish is more than the TTL before `now`.
    pub expired: bool,
    /// Why the pane is or is not expired, recorded so a kept tab is never silent.
    pub reason: String,
}

/// Judge every listed pane against the idle TTL. Pure: no client, no clock of its own.
///
/// `records` must be oldest run first, as [`load_run_records`] returns them, so the LAST record
/// naming a pane is its most recent run. A pane is expired only when its most recent record
/// recorded an exit code and that record's `finished_at` is a finite number more than
/// `ttl_seconds` before `now` — exactly at the boundary is within the TTL. Scope is not judged
/// here; [`retire_idle_tabs`] applies it before anything may close.
#[must_use]
pub fn ttl_candidates(
    panes: &[Pane],
    records: &[Value],
    now: f64,
    ttl_seconds: f64,
) -> Vec<TtlCandidate> {
    let mut by_pane: std::collections::HashMap<&str, Vec<&Value>> =
        std::collections::HashMap::new();
    for record in records {
        if let Some(pane_id) = record.get("pane_id").and_then(Value::as_str) {
            by_pane.entry(pane_id).or_default().push(record);
        }
    }
    panes
        .iter()
        .map(|pane| {
            let mine: &[&Value] = by_pane
                .get(pane.pane_id.as_str())
                .map_or(&[], Vec::as_slice);
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
                    settled: false,
                    expired: false,
                    reason: "no recorded run".to_owned(),
                };
            };
            let finished_at = last
                .get("finished_at")
                .and_then(Value::as_f64)
                .filter(|finished| finished.is_finite());
            // Only runs AFTER the pane's last finished one can still be in flight: runs in one pane
            // are serialised by its lock and by the idle check before each command, so a later
            // run that finished proves every earlier one ended (or was killed with the shell's
            // prompt back), even one whose own exit code never arrived.
            let unfinished = mine
                .iter()
                .rev()
                .take_while(|record| matches!(record.get("exit_code"), None | Some(Value::Null)))
                .count();
            let settled = unfinished == 0 && finished_at.is_some();
            let (expired, reason) = if unfinished > 0 {
                (
                    false,
                    format!("{unfinished} run(s) for this pane have no recorded exit_code"),
                )
            } else {
                match finished_at {
                    None => (
                        false,
                        "the finish time of this pane's last recorded run is unknown".to_owned(),
                    ),
                    Some(_) if ttl_seconds <= 0.0 => (false, "the idle TTL is disabled".to_owned()),
                    Some(finished) if now - finished > ttl_seconds => (
                        true,
                        "last run finished more than the idle TTL ago".to_owned(),
                    ),
                    Some(_) => (false, "last run finished within the idle TTL".to_owned()),
                }
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
                settled,
                expired,
                reason,
            }
        })
        .collect()
}

/// Why an idle sweep closed a tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetireCause {
    /// Its last run finished more than `idle_tab_ttl_seconds` ago.
    IdleTtl,
    /// It was less recently used than the `max_idle_tabs` idle tabs kept.
    MaxIdleTabs,
}

impl RetireCause {
    /// The stable spelling used in JSON and the audit log.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdleTtl => "idle_tab_ttl_seconds",
            Self::MaxIdleTabs => "max_idle_tabs",
        }
    }
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
    /// Which bound selected the tab.
    pub cause: RetireCause,
    /// Why the tab was judged idle.
    pub reason: String,
}

/// What one idle sweep did.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdleSweep {
    /// Tabs actually closed: TTL expiries oldest finish first, then the idle cap's, least
    /// recently used first.
    pub closed: Vec<RetiredTab>,
    /// Tabs a dry run would have closed, in the same order.
    pub would_close: Vec<RetiredTab>,
    /// Recorded panes left open, in listing order, each with its reason.
    pub kept: Vec<(String, String)>,
    /// Why herdr's workspace listing failed, when it did; nothing was closed.
    pub listing_error: Option<String>,
}

/// List `config`'s workspace: its id and panes, or why that failed.
fn list_workspace<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
) -> Result<(String, Vec<Pane>)> {
    match client.workspace_id_for_label(&config.workspace)? {
        Some(workspace_id) => {
            let panes = client.panes(Some(&workspace_id))?;
            Ok((workspace_id, panes))
        }
        None => Err(HerdrRunError::unavailable(format!(
            "herdr has no workspace labelled {}",
            quote(&config.workspace)
        ))),
    }
}

/// Why a recorded pane is not ours to manage under the current configuration, if it is not.
fn scope_refusal(verdict: &TtlCandidate, records: &[Value], config: &Config) -> Option<String> {
    let (in_scope, recorded_tab, tab_label, workspace_label) =
        scope_of(&verdict.pane_id, records, config);
    if !in_scope {
        return Some(format!(
            "tab {} in workspace {} is not one of ours",
            quote_optional(tab_label.as_deref()),
            quote_optional(workspace_label.as_deref())
        ));
    }
    match recorded_tab {
        Some(recorded_tab) if recorded_tab != verdict.tab_id => Some(format!(
            "pane was recorded in tab {} but is now in tab {}; refusing to guess",
            quote(&recorded_tab),
            quote(&verdict.tab_id)
        )),
        _ => None,
    }
}

/// Order settled tabs most recently used first: latest finish first, ties in listing order.
fn most_recent_first(pool: &mut [&TtlCandidate]) {
    pool.sort_by(|left, right| {
        right
            .finished_at
            .unwrap_or(0.0)
            .partial_cmp(&left.finished_at.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Close every tab in `config`'s workspace that is idle past the TTL, then every idle tab past
/// the `max_idle_tabs` most recently used.
///
/// Expired tabs are examined oldest finish first. The idle cap then ranks the settled tabs still
/// open by finish, newest first, keeps the first `max_idle_tabs` of them unprobed, and examines
/// the rest least recently used first. `exclude_pane_id` keeps the sweep at the end of a run
/// from closing the very tab the run just used (it still counts as idle). A dry run judges
/// shells exactly as [`survey`] does — without the pane lock — and closes nothing.
#[allow(clippy::too_many_arguments)]
pub fn retire_idle_tabs<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    actor: &str,
    now: f64,
    dry_run: bool,
    exclude_pane_id: Option<&str>,
    proc_root: &Path,
) -> IdleSweep {
    let (workspace_id, panes) = match list_workspace(client, config) {
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
    // the kept list can be emitted in listing order even though tabs are examined in finish
    // order.
    let mut kept_reasons: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut expired: Vec<&TtlCandidate> = Vec::new();
    let mut pool: Vec<&TtlCandidate> = Vec::new();
    for verdict in &verdicts {
        if !verdict.recorded {
            continue;
        }
        let excluded = exclude_pane_id == Some(verdict.pane_id.as_str());
        if let Some(refusal) = scope_refusal(verdict, &records, config) {
            if !excluded {
                kept_reasons.insert(verdict.pane_id.clone(), refusal);
            }
            continue;
        }
        if excluded {
            // The run that just finished here makes this the most recently used idle tab. It
            // takes its place in the count and is never examined.
            if verdict.settled && verdict.tab_panes == 1 {
                pool.push(verdict);
            }
            continue;
        }
        if verdict.expired {
            expired.push(verdict);
            continue;
        }
        kept_reasons.insert(verdict.pane_id.clone(), verdict.reason.clone());
        if verdict.settled && verdict.tab_panes == 1 {
            pool.push(verdict);
        }
    }
    expired.sort_by(|left, right| {
        left.finished_at
            .unwrap_or(0.0)
            .partial_cmp(&right.finished_at.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Second pass: the idle cap ranks what the TTL left. Kept within the cap means kept for the
    // TTL's reason, which is still true; past it, the tab is examined like an expired one.
    let mut over_cap: Vec<&TtlCandidate> = Vec::new();
    if config.max_idle_tabs > 0 {
        most_recent_first(&mut pool);
        let keep = usize::try_from(config.max_idle_tabs).unwrap_or(usize::MAX);
        over_cap = pool
            .into_iter()
            .skip(keep)
            .filter(|verdict| exclude_pane_id != Some(verdict.pane_id.as_str()))
            .collect();
        over_cap.reverse();
    }
    let examine = expired
        .into_iter()
        .map(|verdict| (verdict, RetireCause::IdleTtl))
        .chain(
            over_cap
                .into_iter()
                .map(|verdict| (verdict, RetireCause::MaxIdleTabs)),
        );

    let mut closed = Vec::new();
    let mut would_close = Vec::new();
    for (verdict, cause) in examine {
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
            cause,
            reason,
        };
        if dry_run {
            match require_unsplit(&candidate)
                .and_then(|()| judge_shell(client, config, &candidate, proc_root))
            {
                Ok(reason) => {
                    kept_reasons.remove(&verdict.pane_id);
                    would_close.push(retired(reason));
                }
                Err(reason) => {
                    kept_reasons.insert(verdict.pane_id.clone(), reason);
                }
            }
            continue;
        }
        let need_expired = cause == RetireCause::IdleTtl;
        let recheck =
            |current: &Pane| recheck_unchanged(config, current, verdict, now, need_expired);
        match try_evict(
            client,
            config,
            &candidate,
            &workspace_id,
            Some(&recheck),
            proc_root,
        ) {
            Ok(reason) => {
                kept_reasons.remove(&verdict.pane_id);
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

/// Re-judge a candidate under the pane lock, from a fresh spool load.
///
/// The sweep judged every candidate from one load, then examines them one by one; a run that
/// starts AND finishes on a later candidate while earlier ones are examined would leave it idle
/// with a free lock, and the stale verdict would close (or hand over) a tab whose last run
/// finished seconds ago. Reloading here — lock held, action imminent — is the same discipline
/// the shell verdict already follows. `need_expired` is the TTL's test; the idle cap and reuse
/// need only that every run in the pane finished and its last run is still the one ranked.
fn recheck_unchanged(
    config: &Config,
    current: &Pane,
    expected: &TtlCandidate,
    now: f64,
    need_expired: bool,
) -> Option<String> {
    let fresh_records = load_run_records(config);
    let fresh = ttl_candidates(
        std::slice::from_ref(current),
        &fresh_records,
        now,
        config.idle_tab_ttl_seconds,
    )
    .into_iter()
    .next()?;
    if !fresh.recorded {
        return Some("no recorded run".to_owned());
    }
    if let Some(refusal) = scope_refusal(&fresh, &fresh_records, config) {
        return Some(refusal);
    }
    if (need_expired && !fresh.expired) || !fresh.settled {
        return Some(fresh.reason);
    }
    if fresh.last_run != expected.last_run {
        return Some(
            "the pane's last recorded run changed during the sweep; treating the tab as busy"
                .to_owned(),
        );
    }
    None
}

fn log_retirement(config: &Config, workspace_id: &str, actor: &str, retired: &RetiredTab) {
    let last_run_at = retired.last_run.as_deref().and_then(run_time);
    let bound = match retired.cause {
        RetireCause::IdleTtl => format!(
            "idle past the {}s TTL",
            number_text(config.idle_tab_ttl_seconds)
        ),
        RetireCause::MaxIdleTabs => format!(
            "less recently used than the {} idle tab(s) max_idle_tabs keeps",
            config.max_idle_tabs
        ),
    };
    output::diagnostic(&format!(
        "herdr-run: retired idle tab {} (pane {}, agent {}, last run {}) {bound}",
        retired.tab_id,
        retired.pane_id,
        retired
            .agent
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown"),
        last_run_at.as_deref().unwrap_or("none recorded"),
    ));
    let mut fields = Map::new();
    fields.insert("pane_id".to_owned(), Value::from(retired.pane_id.clone()));
    fields.insert("tab_id".to_owned(), Value::from(retired.tab_id.clone()));
    fields.insert("workspace_id".to_owned(), Value::from(workspace_id));
    fields.insert("retired_agent".to_owned(), retired.agent.clone().into());
    fields.insert("last_run".to_owned(), retired.last_run.clone().into());
    fields.insert("last_run_at".to_owned(), last_run_at.into());
    fields.insert("cause".to_owned(), Value::from(retired.cause.as_str()));
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

// --- reuse: a new agent takes over the least recently used idle tab ----------------------------
//
// When a new agent needs a tab and the workspace already holds `max_idle_tabs` idle ones, a new
// tab would only push the least recently used of them out at the end of the run. Renaming that
// tab for the new agent reaches the same population without a close and a fresh shell. Below
// the cap a new tab is opened as before, so an agent that pauses between commands finds its
// own tab again. A command shares nothing with the previous one in the pane: the runner types
// `command sh -c '...cd <dir> && <argv>...'` and collects output from its own spool files.
//
// The tab is claimed exactly as a close is: unsplit, pane lock taken without waiting, every run
// in the pane finished and its last run unchanged from a fresh spool load, shell provably idle,
// all under the lock. Before the rename the live tab must still carry its recorded label and the
// shell must be the recorded process ([`same_incarnation`]), because a command is about to be
// typed there. The lock is then KEPT and handed to the run ([`PaneReservation`]): between the
// verdict and the command nothing else can type into the pane. The previous owner's cached target
// still names the pane; its runner re-checks the tab's label once it holds the pane lock
// ([`crate::runner::execute`]) and resolves again when the tab went to another agent.

/// A pane lock held from the idle verdict that handed a tab over through the command typed into
/// it. Shared, because the resolved target is cloned; the lock goes when the last copy does.
#[derive(Clone, Debug)]
pub struct PaneReservation {
    /// The pane the lock was taken on.
    pub pane_id: String,
    /// The open, exclusively locked pane lock file.
    pub lock: std::sync::Arc<std::fs::File>,
}

impl PartialEq for PaneReservation {
    fn eq(&self, other: &Self) -> bool {
        self.pane_id == other.pane_id && std::sync::Arc::ptr_eq(&self.lock, &other.lock)
    }
}

impl Eq for PaneReservation {}

/// A recorded idle tab renamed for a new agent, with its pane lock still held.
#[derive(Debug)]
pub struct Reuse {
    /// The tab handed over.
    pub tab_id: String,
    /// Its one pane.
    pub pane_id: String,
    /// The pane lock, held since the idle verdict.
    pub reservation: PaneReservation,
}

/// Take over the least recently used idle tab for `agent` instead of opening another one, when
/// the workspace already holds `max_idle_tabs` idle tabs.
///
/// `None` when the cap is disabled, when fewer idle tabs than the cap exist, when nothing can be
/// listed, or when no candidate survives the under-lock verdict and the rename: the caller then
/// opens a tab as it always has. Counted and ranked exactly as [`retire_idle_tabs`] counts and
/// ranks, except that expired tabs count too: they are idle, and the least recently used.
pub fn reuse_idle_tab<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    workspace_id: &str,
    agent: &str,
    tab_label: &str,
    now: f64,
    proc_root: &Path,
) -> Option<Reuse> {
    if config.max_idle_tabs == 0 {
        return None;
    }
    let panes = client.panes(Some(workspace_id)).ok()?;
    let records = load_run_records(config);
    let verdicts = ttl_candidates(&panes, &records, now, config.idle_tab_ttl_seconds);
    let mut pool: Vec<&TtlCandidate> = verdicts
        .iter()
        .filter(|verdict| verdict.settled && verdict.tab_panes == 1)
        .filter(|verdict| scope_refusal(verdict, &records, config).is_none())
        .collect();
    if (pool.len() as u64) < config.max_idle_tabs {
        return None;
    }
    most_recent_first(&mut pool);
    pool.reverse();
    for verdict in pool {
        let candidate = Candidate {
            pane_id: verdict.pane_id.clone(),
            tab_id: verdict.tab_id.clone(),
            last_run: verdict.last_run.clone(),
            agent: verdict.agent.clone(),
            tab_panes: verdict.tab_panes,
        };
        let recheck = |current: &Pane| recheck_unchanged(config, current, verdict, now, false);
        let Ok((reason, lock)) = claim_idle(
            client,
            config,
            &candidate,
            workspace_id,
            Some(&recheck),
            proc_root,
        ) else {
            continue;
        };
        if same_incarnation(client, config, &candidate, workspace_id, proc_root).is_err() {
            continue;
        }
        if client.rename_tab(&candidate.tab_id, tab_label).is_err() {
            // The control call may have failed AFTER the rename took (a timeout): look the label
            // up rather than guess. Took: the tab is ours, lock still held. Did not: stop here,
            // because opening a tab under a label that may already be applied would leave two.
            match client.tab_id_for_label(workspace_id, tab_label) {
                Ok(Some(found)) if found == candidate.tab_id => {}
                _ => return None,
            }
        }
        log_reuse(config, workspace_id, agent, tab_label, &candidate, &reason);
        return Some(Reuse {
            tab_id: candidate.tab_id,
            pane_id: candidate.pane_id.clone(),
            reservation: PaneReservation {
                pane_id: candidate.pane_id,
                lock: std::sync::Arc::new(lock),
            },
        });
    }
    None
}

/// Whether the live tab and shell are the very ones the pane's last run record describes.
///
/// Scope is judged from the records, and a record names a pane and tab by ID only. Before a tab
/// is handed to another agent -- a command is about to be typed into it -- the live tab must
/// still carry the label the record gave it (a human may have renamed and adopted it), and the
/// pane's shell must be the recorded process: same pid, same start tick, same boot. A pane or
/// tab ID Herdr reissued after a restart, or a shell that was replaced, fails here. Anything
/// that cannot be read fails too.
fn same_incarnation<A: HerdrApi + ?Sized>(
    client: &A,
    config: &Config,
    candidate: &Candidate,
    workspace_id: &str,
    proc_root: &Path,
) -> std::result::Result<(), String> {
    let records = load_run_records(config);
    let latest = records
        .iter()
        .rfind(|record| record.get("pane_id").and_then(Value::as_str) == Some(&candidate.pane_id))
        .ok_or("no recorded run")?;
    let label = latest
        .get("tab")
        .and_then(|tab| tab.get("label"))
        .and_then(Value::as_str)
        .ok_or("the last run record names no tab label")?;
    match client.tab_id_for_label(workspace_id, label) {
        Ok(Some(found)) if found == candidate.tab_id => {}
        _ => {
            return Err(format!(
                "tab {} no longer carries the label {}",
                candidate.tab_id,
                quote(label)
            ))
        }
    }
    let readiness = latest
        .get("readiness")
        .ok_or("the last run record names no shell")?;
    let recorded_pid = readiness.get("shell_pid").and_then(Value::as_i64);
    let recorded_ticks = readiness.get("shell_start_ticks").and_then(Value::as_u64);
    let recorded_boot = readiness.get("boot_id").and_then(Value::as_str);
    let info = client
        .process_info(&candidate.pane_id)
        .map_err(|error| format!("process-info failed: {error}"))?;
    let live_ticks = crate::identity::process_start_ticks(info.shell_pid, proc_root);
    let live_boot = crate::identity::current_boot_id(proc_root);
    if recorded_pid != Some(info.shell_pid)
        || recorded_ticks.is_none()
        || recorded_ticks != live_ticks
        || recorded_boot.is_none()
        || recorded_boot != live_boot.as_deref()
    {
        return Err("the pane's shell is not the one its last run recorded".to_owned());
    }
    Ok(())
}

fn log_reuse(
    config: &Config,
    workspace_id: &str,
    agent: &str,
    tab_label: &str,
    candidate: &Candidate,
    reason: &str,
) {
    let last_run_at = candidate.last_run.as_deref().and_then(run_time);
    output::diagnostic(&format!(
        "herdr-run: reused idle tab {} (pane {}, agent {}, last run {}) for '{agent}' rather than open another past max_idle_tabs {}",
        candidate.tab_id,
        candidate.pane_id,
        candidate
            .agent
            .as_deref()
            .filter(|name| !name.is_empty())
            .unwrap_or("unknown"),
        last_run_at.as_deref().unwrap_or("none recorded"),
        config.max_idle_tabs,
    ));
    let mut fields = Map::new();
    fields.insert("pane_id".to_owned(), Value::from(candidate.pane_id.clone()));
    fields.insert("tab_id".to_owned(), Value::from(candidate.tab_id.clone()));
    fields.insert("tab_label".to_owned(), Value::from(tab_label));
    fields.insert("workspace_id".to_owned(), Value::from(workspace_id));
    fields.insert("previous_agent".to_owned(), candidate.agent.clone().into());
    fields.insert("last_run".to_owned(), candidate.last_run.clone().into());
    fields.insert("last_run_at".to_owned(), last_run_at.into());
    let log = audit::audit_path(
        Path::new(&config.project_root),
        Path::new(&config.spool_dir),
    );
    if !audit::record(
        &log,
        agent,
        &format!("tab rename {}", candidate.tab_id),
        "REUSED",
        reason,
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
            // Field 22, the start tick, is the pid times seven: enough for an identity check.
            format!(
                "{pid} (proc {pid}) S 1 {pid} {sid} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 {} 0",
                pid * 7
            ),
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
        /// How many times panes were listed, and panes that appear from the second listing
        /// on — a tab split or moved between a sweep's listing and its under-lock re-list.
        listings: Mutex<usize>,
        relist_add: Mutex<Vec<Pane>>,
        /// Every `tab rename`, as (tab, label), and tabs whose rename fails.
        renamed: Mutex<Vec<(String, String)>>,
        failing_rename: Mutex<BTreeSet<String>>,
        /// Tabs whose rename is applied but reported as failed, as a control-call timeout does.
        lying_rename: Mutex<BTreeSet<String>>,
        /// Tab labels, for the label lookups a takeover makes.
        labels: Mutex<BTreeMap<String, String>>,
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
        fn tab_id_for_label(&self, _workspace_id: &str, label: &str) -> Result<Option<String>> {
            Ok(self
                .labels
                .lock()
                .unwrap()
                .iter()
                .find(|(_, value)| value.as_str() == label)
                .map(|(tab, _)| tab.clone()))
        }
        fn create_tab(&self, _workspace_id: &str, _label: &str, _cwd: &str) -> Result<String> {
            unreachable!()
        }
        fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
            if self.lying_rename.lock().unwrap().contains(tab_id) {
                self.labels
                    .lock()
                    .unwrap()
                    .insert(tab_id.to_owned(), label.to_owned());
                return Err(HerdrRunError::unavailable("tab rename: timed out"));
            }
            if self.failing_rename.lock().unwrap().contains(tab_id) {
                return Err(HerdrRunError::unavailable(
                    "tab rename: herdr is not answering",
                ));
            }
            self.renamed
                .lock()
                .unwrap()
                .push((tab_id.to_owned(), label.to_owned()));
            self.labels
                .lock()
                .unwrap()
                .insert(tab_id.to_owned(), label.to_owned());
            Ok(())
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
            let mut listings = self.listings.lock().unwrap();
            *listings += 1;
            if *listings == 2 {
                let extra = std::mem::take(&mut *self.relist_add.lock().unwrap());
                self.panes.lock().unwrap().extend(extra);
            }
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
            "settled": candidate.settled,
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
    #[allow(clippy::too_many_arguments)]
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
        write_finished(
            &world.project,
            "r1",
            &old,
            &format!("tab-{old}"),
            "alpha",
            NOW - 901.0,
            Some(0),
            true,
        );
        write_finished(
            &world.project,
            "r2",
            &busy,
            &format!("tab-{busy}"),
            "beta",
            NOW - 5000.0,
            Some(0),
            true,
        );
        write_finished(
            &world.project,
            "r3",
            &fresh,
            &format!("tab-{fresh}"),
            "gamma",
            NOW - 899.0,
            Some(0),
            true,
        );

        let sweep = retire_idle_tabs(
            &world.fake,
            &world.config,
            "tester",
            NOW,
            false,
            None,
            &world.proc_root,
        );
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
        write_finished(
            &world.project,
            "r1",
            &edge,
            &format!("tab-{edge}"),
            "alpha",
            NOW - 900.0,
            Some(0),
            true,
        );
        write_finished(
            &world.project,
            "r2",
            &thinking,
            &format!("tab-{thinking}"),
            "beta",
            NOW - 5000.0,
            None,
            false,
        );
        write_finished(
            &world.project,
            "r3",
            &old,
            &format!("tab-{old}"),
            "gamma",
            NOW - 5000.0,
            Some(0),
            true,
        );

        let dry = retire_idle_tabs(
            &world.fake,
            &world.config,
            "tester",
            NOW,
            true,
            None,
            &world.proc_root,
        );
        assert!(dry.closed.is_empty());
        assert_eq!(dry.would_close.len(), 1);
        assert_eq!(dry.would_close[0].pane_id, old);
        assert!(world.fake.closed().is_empty());
        let kept: std::collections::HashMap<_, _> = dry.kept.into_iter().collect();
        assert_eq!(kept[&edge], "last run finished within the idle TTL");
        assert_eq!(
            kept[&thinking],
            "1 run(s) for this pane have no recorded exit_code"
        );

        let disabled = Config {
            idle_tab_ttl_seconds: 0.0,
            ..world.config.clone()
        };
        let sweep = retire_idle_tabs(
            &world.fake,
            &disabled,
            "tester",
            NOW,
            false,
            None,
            &world.proc_root,
        );
        assert!(sweep.closed.is_empty());
        assert!(sweep
            .kept
            .iter()
            .any(|(pane, reason)| pane == &old && reason == "the idle TTL is disabled"));
    }

    #[test]
    fn a_tab_split_after_the_sweep_listing_is_kept() {
        // Tab identity is re-judged under the lock from a fresh listing: a sibling pane
        // that appears between the sweep's listing and the close was never judged, so
        // closing the tab would take it down unjudged.
        let world = World::new("ttl-split-after-listing");
        let old = world.tab("old", Some(101));
        write_finished(
            &world.project,
            "r1",
            &old,
            &format!("tab-{old}"),
            "alpha",
            NOW - 5000.0,
            Some(0),
            true,
        );
        world.fake.relist_add.lock().unwrap().push(Pane {
            pane_id: "sibling".to_owned(),
            tab_id: format!("tab-{old}"),
            workspace_id: "w1".to_owned(),
        });
        let sweep = retire_idle_tabs(
            &world.fake,
            &world.config,
            "tester",
            NOW,
            false,
            None,
            &world.proc_root,
        );
        assert!(sweep.closed.is_empty());
        assert!(world.fake.closed().is_empty());
        assert_eq!(sweep.kept.len(), 1);
        assert!(
            sweep.kept[0].1.contains("holds 2 panes"),
            "{}",
            sweep.kept[0].1
        );
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

    // --- idle cap and reuse ---------------------------------------------------------------------

    /// Plant `count` settled idle tabs finishing one minute apart (oldest first) and return
    /// their panes, oldest first.
    fn idle_tabs(world: &World, prefix: &str, count: usize, first_pid: i64) -> Vec<String> {
        (0..count)
            .map(|index| {
                let pid = first_pid + index as i64;
                let pane = world.tab(&format!("{prefix}{index}"), Some(pid));
                write_finished(
                    &world.project,
                    &format!("r-{prefix}{index}"),
                    &pane,
                    &format!("tab-{pane}"),
                    &format!("{prefix}{index}"),
                    NOW - 240.0 + 60.0 * index as f64,
                    Some(0),
                    true,
                );
                record_shell(world, &format!("r-{prefix}{index}"), pid, pid * 7);
                world
                    .fake
                    .labels
                    .lock()
                    .unwrap()
                    .insert(format!("tab-{pane}"), format!("{prefix}{index}"));
                pane
            })
            .collect()
    }

    /// Add the shell identity a real run records to an already planted run record.
    fn record_shell(world: &World, run_id: &str, pid: i64, ticks: i64) {
        let path = world
            .project
            .join(".herdr-run")
            .join("runs")
            .join(run_id)
            .join("meta.json");
        let mut record: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        record["readiness"] =
            json!({"shell_pid": pid, "shell_start_ticks": ticks, "boot_id": "boot-test"});
        fs::write(&path, record.to_string()).unwrap();
    }

    fn sweep(world: &World, config: &Config, dry_run: bool, exclude: Option<&str>) -> IdleSweep {
        retire_idle_tabs(
            &world.fake,
            config,
            "tester",
            NOW,
            dry_run,
            exclude,
            &world.proc_root,
        )
    }

    #[test]
    fn past_max_idle_tabs_the_least_recently_used_idle_tabs_are_retired() {
        let world = World::new("cap-close");
        let panes = idle_tabs(&world, "a", 5, 301);
        let config = Config {
            max_idle_tabs: 3,
            ..world.config.clone()
        };
        let result = sweep(&world, &config, false, None);
        let closed: Vec<_> = result
            .closed
            .iter()
            .map(|tab| tab.pane_id.clone())
            .collect();
        assert_eq!(closed, [panes[0].clone(), panes[1].clone()]);
        assert!(result
            .closed
            .iter()
            .all(|tab| tab.cause == RetireCause::MaxIdleTabs));
        let kept: std::collections::HashMap<_, _> = result.kept.into_iter().collect();
        assert_eq!(kept.len(), 3);
        assert_eq!(kept[&panes[4]], "last run finished within the idle TTL");
        let audit = world.audit();
        assert_eq!(audit.len(), 2);
        assert_eq!(audit[0]["cause"], "max_idle_tabs");
        assert_eq!(audit[0]["retired_agent"], "a0");

        // Already within the cap: a second sweep closes nothing.
        assert!(sweep(&world, &config, false, None).closed.is_empty());
    }

    #[test]
    fn tabs_with_a_command_in_flight_are_never_counted_against_the_idle_cap() {
        let world = World::new("cap-in-flight");
        let idle = idle_tabs(&world, "idle", 2, 311);
        for index in 0..3 {
            let pane = world.tab(&format!("busy{index}"), None);
            write_finished(
                &world.project,
                &format!("r-busy{index}"),
                &pane,
                &format!("tab-{pane}"),
                &format!("busy{index}"),
                NOW - 10.0,
                None,
                false,
            );
        }
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        let result = sweep(&world, &config, false, None);
        assert!(result.closed.is_empty(), "{:?}", result.closed);
        assert!(
            world.fake.probed().is_empty(),
            "kept tabs are counted, never probed"
        );
        assert_eq!(idle.len(), 2);
    }

    #[test]
    fn a_busy_shell_past_the_idle_cap_is_kept_and_reported() {
        let world = World::new("cap-busy");
        let old_busy = world.tab("old-busy", None);
        write_finished(
            &world.project,
            "r-old-busy",
            &old_busy,
            &format!("tab-{old_busy}"),
            "human",
            NOW - 280.0,
            Some(0),
            true,
        );
        let newer = idle_tabs(&world, "n", 1, 321);
        let config = Config {
            max_idle_tabs: 1,
            ..world.config.clone()
        };
        let result = sweep(&world, &config, false, None);
        assert!(result.closed.is_empty());
        let kept: std::collections::HashMap<_, _> = result.kept.into_iter().collect();
        assert!(
            kept[&old_busy].contains("running: git"),
            "{}",
            kept[&old_busy]
        );
        assert_eq!(newer.len(), 1);
    }

    #[test]
    fn the_tab_a_run_just_used_counts_as_idle_but_is_never_closed() {
        let world = World::new("cap-exclude");
        let panes = idle_tabs(&world, "e", 3, 331);
        let config = Config {
            max_idle_tabs: 1,
            ..world.config.clone()
        };
        // The newest is the run's own: it fills the one slot, so both older tabs go.
        let result = sweep(&world, &config, false, Some(&panes[2]));
        let closed: Vec<_> = result
            .closed
            .iter()
            .map(|tab| tab.pane_id.clone())
            .collect();
        assert_eq!(closed, [panes[0].clone(), panes[1].clone()]);
        assert!(!result.kept.iter().any(|(pane, _)| pane == &panes[2]));
        assert!(!world.fake.probed().contains(&panes[2]));
    }

    #[test]
    fn a_zero_idle_cap_is_disabled_and_a_dry_run_names_the_cause() {
        let world = World::new("cap-zero-dry");
        let panes = idle_tabs(&world, "z", 3, 341);
        let disabled = Config {
            max_idle_tabs: 0,
            ..world.config.clone()
        };
        assert!(sweep(&world, &disabled, false, None).closed.is_empty());
        let capped = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        let dry = sweep(&world, &capped, true, None);
        assert!(dry.closed.is_empty());
        assert_eq!(dry.would_close.len(), 1);
        assert_eq!(dry.would_close[0].pane_id, panes[0]);
        assert_eq!(dry.would_close[0].cause, RetireCause::MaxIdleTabs);
        assert!(!dry.kept.iter().any(|(pane, _)| pane == &panes[0]));
        assert!(world.fake.closed().is_empty());
    }

    #[test]
    fn ttl_expiries_and_the_idle_cap_close_together_each_with_its_cause() {
        let world = World::new("cap-and-ttl");
        let expired = world.tab("expired", Some(351));
        write_finished(
            &world.project,
            "r-expired",
            &expired,
            &format!("tab-{expired}"),
            "old",
            NOW - 5000.0,
            Some(0),
            true,
        );
        let fresh = idle_tabs(&world, "f", 3, 352);
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        let result = sweep(&world, &config, false, None);
        let closed: Vec<_> = result
            .closed
            .iter()
            .map(|tab| (tab.pane_id.clone(), tab.cause))
            .collect();
        assert_eq!(
            closed,
            [
                (expired, RetireCause::IdleTtl),
                (fresh[0].clone(), RetireCause::MaxIdleTabs)
            ]
        );
    }

    fn reuse(world: &World, config: &Config, label: &str) -> Option<Reuse> {
        reuse_idle_tab(
            &world.fake,
            config,
            "w1",
            label,
            label,
            NOW,
            &world.proc_root,
        )
    }

    #[test]
    fn below_the_idle_cap_no_tab_is_taken_over() {
        let world = World::new("reuse-below");
        idle_tabs(&world, "b", 2, 361);
        let config = Config {
            max_idle_tabs: 3,
            ..world.config.clone()
        };
        assert!(reuse(&world, &config, "newcomer").is_none());
        assert!(world.fake.renamed.lock().unwrap().is_empty());
        assert!(world.fake.probed().is_empty());
        let disabled = Config {
            max_idle_tabs: 0,
            ..world.config.clone()
        };
        idle_tabs(&world, "c", 3, 365);
        assert!(reuse(&world, &disabled, "newcomer").is_none());
    }

    #[test]
    fn at_the_idle_cap_the_least_recently_used_idle_tab_is_renamed_and_stays_locked() {
        let world = World::new("reuse-at-cap");
        let panes = idle_tabs(&world, "r", 3, 371);
        let config = Config {
            max_idle_tabs: 3,
            ..world.config.clone()
        };
        let taken = reuse(&world, &config, "newcomer").expect("the oldest idle tab");
        assert_eq!(taken.pane_id, panes[0]);
        assert_eq!(taken.tab_id, format!("tab-{}", panes[0]));
        assert_eq!(
            *world.fake.renamed.lock().unwrap(),
            [(format!("tab-{}", panes[0]), "newcomer".to_owned())]
        );
        // The verdict's lock is still held: nobody else can take the pane before the run.
        let other = open_lock_file(&pane_lock_path(&panes[0]).unwrap()).unwrap();
        assert_eq!(
            other.try_lock_exclusive().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        let audit = world.audit();
        assert_eq!(audit[0]["verdict"], "REUSED");
        assert_eq!(audit[0]["previous_agent"], "r0");
        assert_eq!(audit[0]["tab_label"], "newcomer");
        assert!(world.fake.closed().is_empty());
        drop(taken);
        // Polled briefly: a process another test forks in parallel holds a copy of every open
        // descriptor, this lock's included, until it reaches `exec` and close-on-exec drops it.
        let released = (0..200).any(|_| {
            other.try_lock_exclusive().is_ok() || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                false
            }
        });
        assert!(released, "the reservation's lock outlived it");
    }

    #[test]
    fn reuse_skips_busy_and_locked_tabs() {
        let world = World::new("reuse-skips");
        let busy = world.tab("busy", None);
        write_finished(
            &world.project,
            "r-busy",
            &busy,
            &format!("tab-{busy}"),
            "human",
            NOW - 500.0,
            Some(0),
            true,
        );
        let panes = idle_tabs(&world, "s", 3, 381);
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        let held = open_lock_file(&pane_lock_path(&panes[0]).unwrap()).unwrap();
        held.try_lock_exclusive().unwrap();
        let taken = reuse(&world, &config, "newcomer").expect("the second idle tab");
        assert_eq!(taken.pane_id, panes[1]);
        drop(held);
    }

    #[test]
    fn a_failed_rename_stops_reuse_unless_the_label_took() {
        let world = World::new("reuse-rename");
        let panes = idle_tabs(&world, "f", 2, 386);
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        // Refused outright: never try the next tab, the caller opens one of its own.
        world
            .fake
            .failing_rename
            .lock()
            .unwrap()
            .insert(format!("tab-{}", panes[0]));
        assert!(reuse(&world, &config, "newcomer").is_none());
        assert!(world.fake.renamed.lock().unwrap().is_empty());
        // Applied but reported as failed: the label lookup finds it, and the tab is ours.
        // A fresh world: the first attempt's pane lock may outlive its drop for a moment in a
        // process another test forks in parallel (see the reservation test above).
        let world = World::new("reuse-rename-took");
        let panes = idle_tabs(&world, "g", 2, 388);
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        world
            .fake
            .lying_rename
            .lock()
            .unwrap()
            .insert(format!("tab-{}", panes[0]));
        let taken = reuse(&world, &config, "newcomer").expect("the rename took");
        assert_eq!(taken.pane_id, panes[0]);
    }

    #[test]
    fn a_tab_relabelled_or_a_shell_replaced_since_its_record_is_never_taken_over() {
        let world = World::new("reuse-incarnation");
        let panes = idle_tabs(&world, "i", 2, 391);
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        // A human renamed the oldest tab; the shell in the next is not the recorded one.
        world
            .fake
            .labels
            .lock()
            .unwrap()
            .insert(format!("tab-{}", panes[0]), "mine-now".to_owned());
        record_shell(&world, "r-i1", 392, 1);
        assert!(reuse(&world, &config, "newcomer").is_none());
        assert!(world.fake.renamed.lock().unwrap().is_empty());
    }

    #[test]
    fn tabs_out_of_scope_or_in_flight_never_fill_the_idle_pool() {
        let world = World::new("reuse-scope");
        let mine = idle_tabs(&world, "m", 1, 395);
        let foreign = world.tab("foreign", Some(396));
        let directory = world
            .project
            .join(".herdr-run")
            .join("runs")
            .join("r-foreign");
        fs::create_dir_all(&directory).unwrap();
        let record = json!({
            "agent": "x", "pane_id": foreign, "run_id": "r-foreign", "exit_code": 0,
            "tab": {"id": format!("tab-{foreign}"), "label": "x"},
            "workspace": {"id": "w1", "label": "someone-elses"},
        });
        fs::write(directory.join("meta.json"), record.to_string()).unwrap();
        fs::write(directory.join("exit_code"), "0\n").unwrap();
        let config = Config {
            max_idle_tabs: 2,
            ..world.config.clone()
        };
        assert!(reuse(&world, &config, "newcomer").is_none());
        assert_eq!(mine.len(), 1);
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
            fs::create_dir_all(proc_root.join("sys/kernel/random")).unwrap();
            fs::write(proc_root.join(crate::identity::BOOT_ID_PATH), "boot-test\n").unwrap();
            // The TTL cases below were written around a fifteen-minute TTL; pinned here so the
            // shipped default can move without rewriting every boundary.
            let config = Config {
                idle_tab_ttl_seconds: 900.0,
                ..config(&project)
            };
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
