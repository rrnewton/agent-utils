//! `agentctl inbox watch`: observe workers and post inbox notices when their state changes.
//!
//! One sample lists Herdr panes, asks Claude Code for the busy or idle state of each Claude
//! session, and joins the two by process id through `HERDR_PANE_ID` in `/proc/<pid>/environ`.
//! Claude's own state is used for Claude panes because Herdr's screen rules can classify a working
//! Claude pane as idle. Other panes use Herdr's state, and an idle edge must hold for
//! `--idle-samples` consecutive samples before it is announced.
//!
//! The pure core is [`step`]: previous watch state plus one sample gives the notices to post. The
//! caller adds text (the last assistant message of a Claude transcript since the last notice, or
//! the tail of the terminal) and posts through the inbox, so all coalescing rules apply.
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use clap::Args;
use serde::{Deserialize, Serialize};

use super::{
    check_name, cut, now_ms, print_json, Cursor, Inbox, InboxError, NoticeKind, PostRequest,
    Result, Target, DEFAULT_MAX_LIVE, DEFAULT_STALE_SECONDS, MAX_TEXT_BYTES,
};

/// Default seconds between samples.
pub(crate) const DEFAULT_INTERVAL_SECONDS: u64 = 30;
/// Default consecutive idle samples required before a non-Claude pane is announced idle.
pub(crate) const DEFAULT_IDLE_SAMPLES: u32 = 2;
/// Default minutes before the first still-idle reminder; each later one waits twice as long.
pub(crate) const DEFAULT_REMIND_MINUTES: u64 = 30;
/// Most still-idle reminders for one idle period.
pub(crate) const MAX_REMINDERS: u32 = 3;
/// Most transcript bytes read to find the last assistant message.
const TRANSCRIPT_TAIL_BYTES: u64 = 8 * 1024 * 1024;
/// Seconds one `herdr` or `claude` query may run.
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);

/// `agentctl inbox watch` arguments.
#[derive(Args)]
pub(crate) struct WatchArgs {
    #[command(flatten)]
    target: Target,
    /// Take one sample, post its notices, save state, and exit (for a cron job or loop)
    #[arg(long)]
    once: bool,
    /// Seconds between samples when running continuously
    #[arg(long, default_value_t = DEFAULT_INTERVAL_SECONDS, value_name = "SECONDS")]
    interval: u64,
    /// Consecutive idle samples before a pane without Claude state is announced idle
    #[arg(long, default_value_t = DEFAULT_IDLE_SAMPLES, value_name = "N")]
    idle_samples: u32,
    /// Minutes an announced idle worker waits before the first still-idle reminder (then 2x, 4x; at most three)
    #[arg(long, default_value_t = DEFAULT_REMIND_MINUTES, value_name = "MINUTES")]
    remind_minutes: u64,
    /// Worker name to ignore; repeatable. The coordinator's own pane (named like --to) is always ignored
    #[arg(long, value_name = "NAME")]
    exclude: Vec<String>,
    /// Claude Code executable used for `claude agents --json`
    #[arg(long, default_value = "claude", value_name = "PATH")]
    claude_bin: PathBuf,
    /// Directory holding Claude Code transcripts as <project>/<session-id>.jsonl (default: ~/.claude/projects)
    #[arg(long, value_name = "DIR")]
    claude_projects: Option<PathBuf>,
    /// Process filesystem used to map Claude processes to Herdr panes
    #[arg(long, default_value = "/proc", value_name = "DIR")]
    proc_root: PathBuf,
}

/// What a worker is doing, as far as the watcher can tell.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WorkerState {
    /// Mid-turn.
    Working,
    /// Finished a turn and waiting for input.
    Idle,
    /// Waiting on a permission dialog, a question, or other outside input.
    Blocked,
}

/// One worker in one sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Observation {
    agent: String,
    pane: String,
    state: WorkerState,
    /// True when the state came from Claude Code itself rather than from screen rules.
    authoritative: bool,
    transcript: Option<PathBuf>,
}

/// Persistent memory of one worker between samples.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WorkerRecord {
    pane: String,
    state: WorkerState,
    since_unix_ms: u64,
    idle_samples: u32,
    /// The state most recently announced; `None` after `working` was posted.
    announced: Option<WorkerState>,
    reminders: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transcript: Option<PathBuf>,
    /// Transcript bytes already covered by an earlier notice.
    #[serde(default)]
    offset: u64,
}

/// Everything the watcher remembers for one coordinator.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct WatchState {
    schema: u32,
    workers: BTreeMap<String, WorkerRecord>,
}

/// A notice the core decided to post; the caller supplies the text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Decision {
    agent: String,
    kind: NoticeKind,
}

/// Tunables for [`step`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    idle_samples: u32,
    remind_after_ms: u64,
}

/// Advance the watch state by one sample and decide which notices to post.
///
/// A worker seen for the first time is recorded without a notice unless it is blocked, so starting
/// the watcher does not announce every idle worker at once. A worker that disappears is announced
/// as exited once and forgotten.
pub(crate) fn step(
    state: &mut WatchState,
    sample: &[Observation],
    now: u64,
    policy: Policy,
) -> Vec<Decision> {
    let mut decisions = Vec::new();
    let mut decide = |agent: &str, kind| {
        decisions.push(Decision {
            agent: agent.to_owned(),
            kind,
        });
    };
    for observation in sample {
        let required = if observation.authoritative {
            1
        } else {
            policy.idle_samples.max(1)
        };
        let Some(record) = state.workers.get_mut(&observation.agent) else {
            let announced = (observation.state == WorkerState::Blocked).then(|| {
                decide(&observation.agent, NoticeKind::Blocked);
                WorkerState::Blocked
            });
            state.workers.insert(
                observation.agent.clone(),
                WorkerRecord {
                    pane: observation.pane.clone(),
                    state: observation.state,
                    since_unix_ms: now,
                    idle_samples: u32::from(observation.state == WorkerState::Idle),
                    // A worker first seen idle counts as already announced: its idle period began
                    // before the watcher could observe it.
                    announced: announced
                        .or((observation.state == WorkerState::Idle).then_some(WorkerState::Idle)),
                    reminders: 0,
                    transcript: observation.transcript.clone(),
                    offset: observation
                        .transcript
                        .as_deref()
                        .and_then(|path| fs::metadata(path).ok())
                        .map_or(0, |metadata| metadata.len()),
                },
            );
            continue;
        };
        record.pane.clone_from(&observation.pane);
        if record.transcript != observation.transcript {
            record.transcript.clone_from(&observation.transcript);
            record.offset = 0;
        }
        if observation.state != record.state {
            record.state = observation.state;
            record.since_unix_ms = now;
            record.reminders = 0;
            record.idle_samples = u32::from(observation.state == WorkerState::Idle);
            match observation.state {
                WorkerState::Working => {
                    if record.announced.take().is_some() {
                        decide(&observation.agent, NoticeKind::Working);
                    }
                }
                WorkerState::Blocked => {
                    record.announced = Some(WorkerState::Blocked);
                    decide(&observation.agent, NoticeKind::Blocked);
                }
                WorkerState::Idle => {
                    record.announced = None;
                    if record.idle_samples >= required {
                        record.announced = Some(WorkerState::Idle);
                        decide(&observation.agent, NoticeKind::Idle);
                    }
                }
            }
            continue;
        }
        if observation.state != WorkerState::Idle {
            continue;
        }
        record.idle_samples = record.idle_samples.saturating_add(1);
        if record.announced != Some(WorkerState::Idle) {
            if record.idle_samples >= required {
                record.announced = Some(WorkerState::Idle);
                decide(&observation.agent, NoticeKind::Idle);
            }
            continue;
        }
        let wait = policy
            .remind_after_ms
            .saturating_mul(1_u64 << record.reminders.min(16));
        let elapsed_since_last = now.saturating_sub(record.since_unix_ms);
        if record.reminders < MAX_REMINDERS
            && policy.remind_after_ms > 0
            && elapsed_since_last >= wait
        {
            record.reminders += 1;
            record.since_unix_ms = now;
            decide(&observation.agent, NoticeKind::StillIdle);
        }
    }
    let seen = sample
        .iter()
        .map(|observation| observation.agent.as_str())
        .collect::<std::collections::HashSet<_>>();
    let gone = state
        .workers
        .keys()
        .filter(|agent| !seen.contains(agent.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for agent in gone {
        state.workers.remove(&agent);
        decide(&agent, NoticeKind::Exited);
    }
    decisions
}

/// Inbox worker name for a Herdr pane: its agent name when that is a valid inbox name, else a
/// name derived from the pane id.
fn worker_name(name: Option<&str>, pane: &str) -> String {
    if let Some(name) = name.filter(|name| check_name("--from", name).is_ok()) {
        return name.to_owned();
    }
    let derived = pane
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("pane-{derived}")
}

#[derive(Deserialize)]
struct HerdrList {
    result: HerdrResult,
}

#[derive(Deserialize)]
struct HerdrResult {
    agents: Vec<HerdrAgent>,
}

#[derive(Deserialize)]
struct HerdrAgent {
    pane_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    agent_status: Option<String>,
}

#[derive(Deserialize)]
struct ClaudeSession {
    #[serde(default)]
    pid: Option<serde_json::Value>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Run a query command with a deadline and return its stdout.
fn query(program: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| {
            InboxError::unavailable(format!("cannot run {}: {error}", program.display()))
        })?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(InboxError::unavailable(format!(
                    "cannot wait for {}: {error}",
                    program.display()
                )));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| InboxError::unavailable("query reader panicked"))?
        .map_err(|error| InboxError::io(&format!("cannot read {}", program.display()), &error))?;
    match status {
        Some(status) if status.success() => Ok(bytes),
        Some(status) => Err(InboxError::unavailable(format!(
            "{} {} exited {status}",
            program.display(),
            arguments.join(" ")
        ))),
        None => Err(InboxError::unavailable(format!(
            "{} {} did not finish within {} seconds",
            program.display(),
            arguments.join(" "),
            QUERY_TIMEOUT.as_secs()
        ))),
    }
}

/// `HERDR_PANE_ID` of a process, read from `<proc_root>/<pid>/environ`.
fn pane_of_process(proc_root: &Path, pid: &str) -> Option<String> {
    let environ = fs::read(proc_root.join(pid).join("environ")).ok()?;
    environ.split(|byte| *byte == 0).find_map(|entry| {
        entry
            .strip_prefix(b"HERDR_PANE_ID=")
            .map(|value| String::from_utf8_lossy(value).into_owned())
    })
}

/// `<projects>/<any project>/<session>.jsonl`, if it exists.
fn transcript_of_session(projects: &Path, session: &str) -> Option<PathBuf> {
    fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|project| project.path().join(format!("{session}.jsonl")))
        .find(|path| path.is_file())
}

struct Sources<'a> {
    herdr: &'a Path,
    claude: &'a Path,
    projects: &'a Path,
    proc_root: &'a Path,
}

/// Take one sample of every Herdr pane that hosts an agent.
fn sample(sources: &Sources<'_>, ignored: &[String]) -> Result<Vec<Observation>> {
    let listing: HerdrList = serde_json::from_slice(&query(sources.herdr, &["agent", "list"])?)
        .map_err(|error| {
            InboxError::unavailable(format!("cannot parse `herdr agent list`: {error}"))
        })?;
    // Claude's view is best-effort: without it every pane falls back to Herdr's state.
    let mut claude_by_pane = BTreeMap::new();
    if let Ok(bytes) = query(sources.claude, &["agents", "--json"]) {
        if let Ok(sessions) = serde_json::from_slice::<Vec<ClaudeSession>>(&bytes) {
            for session in sessions {
                let pid = match &session.pid {
                    Some(serde_json::Value::String(pid)) => pid.clone(),
                    Some(serde_json::Value::Number(pid)) => pid.to_string(),
                    _ => continue,
                };
                if let Some(pane) = pane_of_process(sources.proc_root, &pid) {
                    claude_by_pane.insert(pane, session);
                }
            }
        }
    }
    let mut observations = Vec::new();
    for agent in listing.result.agents {
        let Some(status) = agent.agent_status.as_deref() else {
            continue;
        };
        if agent.agent.is_none() {
            continue;
        }
        let name = worker_name(agent.name.as_deref(), &agent.pane_id);
        if ignored.contains(&name) {
            continue;
        }
        let herdr_state = match status {
            "blocked" => WorkerState::Blocked,
            "idle" | "done" => WorkerState::Idle,
            _ => WorkerState::Working,
        };
        let claude = claude_by_pane.get(&agent.pane_id);
        let (state, authoritative) = match claude.and_then(|session| session.status.as_deref()) {
            Some("idle") if herdr_state == WorkerState::Blocked => (WorkerState::Blocked, true),
            Some("idle") => (WorkerState::Idle, true),
            Some(_) if herdr_state == WorkerState::Blocked => (WorkerState::Blocked, true),
            Some(_) => (WorkerState::Working, true),
            None => (herdr_state, false),
        };
        let transcript = claude
            .and_then(|session| session.session_id.as_deref())
            .and_then(|session| transcript_of_session(sources.projects, session));
        observations.push(Observation {
            agent: name,
            pane: agent.pane_id,
            state,
            authoritative,
            transcript,
        });
    }
    Ok(observations)
}

/// The last assistant prose in a Claude transcript between `start` and its end, and the end
/// offset (the byte after the last complete line). Reads at most the final 8 MiB of the range.
pub(crate) fn last_assistant_text(path: &Path, start: u64) -> io::Result<(Option<String>, u64)> {
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let start = start.min(length);
    let from = start.max(length.saturating_sub(TRANSCRIPT_TAIL_BYTES));
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(length - from).read_to_end(&mut bytes)?;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    let mut last = None;
    for line in bytes[..complete].split(|byte| *byte == b'\n') {
        let Ok(record) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("type").and_then(serde_json::Value::as_str) != Some("assistant") {
            continue;
        }
        let Some(content) = record
            .pointer("/message/content")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        let text = content
            .iter()
            .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            last = Some(text);
        }
    }
    Ok((last, from + complete as u64))
}

/// The last non-empty lines of a pane, above its bottom rows (prompt box and footer).
fn terminal_tail(herdr: &Path, pane: &str) -> Option<String> {
    let bytes = query(
        herdr,
        &[
            "pane",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            "80",
        ],
    )
    .ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let lines = text.lines().map(str::trim_end).collect::<Vec<_>>();
    let body = &lines[..lines.len().saturating_sub(6)];
    let nonempty = body
        .iter()
        .copied()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    let kept = &nonempty[nonempty.len().saturating_sub(12)..];
    (!kept.is_empty()).then(|| kept.join("\n"))
}

fn notice_text(
    decision: &Decision,
    record: Option<&mut WorkerRecord>,
    herdr: &Path,
) -> (String, Option<Cursor>) {
    let Some(record) = record else {
        return (
            "the worker's pane is no longer listed by Herdr".to_owned(),
            None,
        );
    };
    let header = match decision.kind {
        NoticeKind::Idle => "finished its turn and is waiting for input".to_owned(),
        NoticeKind::Blocked => {
            "is waiting on outside input (permission dialog or question)".to_owned()
        }
        NoticeKind::StillIdle => format!(
            "is still idle (reminder {} of {MAX_REMINDERS})",
            record.reminders
        ),
        _ => String::new(),
    };
    let mut cursor = None;
    let mut body = None;
    if decision.kind == NoticeKind::Idle {
        if let Some(path) = record.transcript.clone() {
            if let Ok((text, end)) = last_assistant_text(&path, record.offset) {
                if end > record.offset {
                    cursor = Some(Cursor {
                        path: path.to_string_lossy().into_owned(),
                        start: record.offset,
                        end,
                    });
                }
                record.offset = end;
                body = text.map(|text| format!("last message:\n{text}"));
            }
        }
    }
    if body.is_none() && decision.kind != NoticeKind::StillIdle {
        body = terminal_tail(herdr, &record.pane).map(|tail| format!("terminal tail:\n{tail}"));
    }
    let mut text = format!("pane {}: {header}", record.pane);
    if let Some(body) = body {
        text.push('\n');
        text.push_str(&body);
    }
    let (text, _) = cut(&text, MAX_TEXT_BYTES);
    (text.to_owned(), cursor)
}

#[derive(Serialize)]
struct Posted {
    agent: String,
    kind: NoticeKind,
    outcome: &'static str,
}

/// Run `agentctl inbox watch`.
pub(crate) fn run(registry: &Path, herdr: &Path, args: WatchArgs) -> Result<i32> {
    let inbox = Inbox::open(registry, &args.target.coordinator)?;
    let _watching = inbox.lock(".watch.lock", false).map_err(|_| {
        InboxError::busy(format!(
            "another watcher already runs for {}",
            args.target.coordinator
        ))
    })?;
    let projects = match args.claude_projects {
        Some(path) => path,
        None => std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join(".claude")
            .join("projects"),
    };
    let sources = Sources {
        herdr,
        claude: &args.claude_bin,
        projects: &projects,
        proc_root: &args.proc_root,
    };
    let mut ignored = args.exclude.clone();
    ignored.push(args.target.coordinator.clone());
    let policy = Policy {
        idle_samples: args.idle_samples,
        remind_after_ms: args.remind_minutes.saturating_mul(60_000),
    };
    let state_path = inbox.root.join("watch.json");
    loop {
        let mut state = fs::read(&state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WatchState>(&bytes).ok())
            .unwrap_or_default();
        state.schema = 1;
        let now = now_ms()?;
        let observations = sample(&sources, &ignored)?;
        let decisions = step(&mut state, &observations, now, policy);
        let mut posted = Vec::new();
        for decision in &decisions {
            let (text, cursor) = if decision.kind == NoticeKind::Working {
                (String::new(), None)
            } else {
                notice_text(decision, state.workers.get_mut(&decision.agent), herdr)
            };
            let outcome = inbox.post(
                &PostRequest {
                    agent: &decision.agent,
                    kind: decision.kind,
                    text: &text,
                    key: None,
                    cursor,
                    max_live: DEFAULT_MAX_LIVE,
                    stale_after: DEFAULT_STALE_SECONDS,
                },
                now,
            )?;
            posted.push(Posted {
                agent: decision.agent.clone(),
                kind: decision.kind,
                outcome: outcome.outcome,
            });
        }
        inbox.write_json(".", "watch.json", &state)?;
        print_json(&serde_json::json!({
            "sampled_unix_ms": now,
            "workers": observations.len(),
            "posted": posted,
        }))?;
        if args.once {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_secs(args.interval.max(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    const POLICY: Policy = Policy {
        idle_samples: 2,
        remind_after_ms: 1_000,
    };

    fn seen(agent: &str, state: WorkerState, authoritative: bool) -> Observation {
        Observation {
            agent: agent.into(),
            pane: format!("w:{agent}"),
            state,
            authoritative,
            transcript: None,
        }
    }

    fn kinds(decisions: &[Decision]) -> Vec<(&str, NoticeKind)> {
        decisions
            .iter()
            .map(|decision| (decision.agent.as_str(), decision.kind))
            .collect()
    }

    #[test]
    fn a_first_sample_announces_only_blocked_workers() {
        let mut state = WatchState::default();
        let sample = [
            seen("a", WorkerState::Idle, true),
            seen("b", WorkerState::Working, true),
            seen("c", WorkerState::Blocked, false),
        ];
        assert_eq!(
            kinds(&step(&mut state, &sample, 0, POLICY)),
            [("c", NoticeKind::Blocked)]
        );
        assert_eq!(state.workers.len(), 3);
    }

    #[test]
    fn a_claude_idle_edge_is_announced_at_once_and_a_resume_withdraws_it() {
        let mut state = WatchState::default();
        step(
            &mut state,
            &[seen("a", WorkerState::Working, true)],
            0,
            POLICY,
        );
        let decisions = step(
            &mut state,
            &[seen("a", WorkerState::Idle, true)],
            10,
            POLICY,
        );
        assert_eq!(kinds(&decisions), [("a", NoticeKind::Idle)]);
        let decisions = step(
            &mut state,
            &[seen("a", WorkerState::Idle, true)],
            20,
            POLICY,
        );
        assert!(decisions.is_empty(), "one idle period is announced once");
        let decisions = step(
            &mut state,
            &[seen("a", WorkerState::Working, true)],
            30,
            POLICY,
        );
        assert_eq!(kinds(&decisions), [("a", NoticeKind::Working)]);
        let decisions = step(
            &mut state,
            &[seen("a", WorkerState::Working, true)],
            40,
            POLICY,
        );
        assert!(decisions.is_empty());
    }

    #[test]
    fn a_screen_rule_idle_edge_must_hold_for_the_required_samples() {
        let mut state = WatchState::default();
        step(
            &mut state,
            &[seen("x", WorkerState::Working, false)],
            0,
            POLICY,
        );
        assert!(step(
            &mut state,
            &[seen("x", WorkerState::Idle, false)],
            10,
            POLICY
        )
        .is_empty());
        let flicker = step(
            &mut state,
            &[seen("x", WorkerState::Working, false)],
            20,
            POLICY,
        );
        assert!(
            flicker.is_empty(),
            "an unannounced idle blip withdraws nothing"
        );
        assert!(step(
            &mut state,
            &[seen("x", WorkerState::Idle, false)],
            30,
            POLICY
        )
        .is_empty());
        let decisions = step(
            &mut state,
            &[seen("x", WorkerState::Idle, false)],
            40,
            POLICY,
        );
        assert_eq!(kinds(&decisions), [("x", NoticeKind::Idle)]);
    }

    #[test]
    fn reminders_back_off_and_stop_after_three() {
        let mut state = WatchState::default();
        step(
            &mut state,
            &[seen("a", WorkerState::Working, true)],
            0,
            POLICY,
        );
        step(&mut state, &[seen("a", WorkerState::Idle, true)], 0, POLICY);
        let mut reminded_at = Vec::new();
        for now in (100..20_000).step_by(100) {
            if !step(
                &mut state,
                &[seen("a", WorkerState::Idle, true)],
                now,
                POLICY,
            )
            .is_empty()
            {
                reminded_at.push(now);
            }
        }
        assert_eq!(
            reminded_at,
            [1_000, 3_000, 7_000],
            "waits of 1 s, 2 s and 4 s, then none"
        );
    }

    #[test]
    fn blocked_and_exited_are_announced_and_a_vanished_worker_is_forgotten() {
        let mut state = WatchState::default();
        step(
            &mut state,
            &[seen("a", WorkerState::Working, true)],
            0,
            POLICY,
        );
        let decisions = step(
            &mut state,
            &[seen("a", WorkerState::Blocked, true)],
            10,
            POLICY,
        );
        assert_eq!(kinds(&decisions), [("a", NoticeKind::Blocked)]);
        let decisions = step(&mut state, &[], 20, POLICY);
        assert_eq!(kinds(&decisions), [("a", NoticeKind::Exited)]);
        assert!(state.workers.is_empty());
        assert!(step(&mut state, &[], 30, POLICY).is_empty());
    }

    #[test]
    fn worker_names_fall_back_to_the_pane_id() {
        assert_eq!(worker_name(Some("kvm"), "wJ:p38"), "kvm");
        assert_eq!(worker_name(None, "wJ:p3A"), "pane-wj-p3a");
        assert_eq!(worker_name(Some("Has Space"), "w1:p2"), "pane-w1-p2");
    }

    fn scratch() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-watch-test-{}-{}",
            std::process::id(),
            super::super::hash_hex(
                &[format!("{:?}", std::time::SystemTime::now()).as_bytes()],
                6
            )
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn the_last_assistant_text_since_the_offset_is_extracted() {
        let directory = scratch();
        let path = directory.join("s.jsonl");
        let first =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old answer"}]}}"#;
        let tool =
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#;
        let last = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Pushed 3f2a9c1."}]}}"#;
        fs::write(&path, format!("{first}\n{tool}\n{last}\n{{\"partial")).unwrap();
        let (text, end) = last_assistant_text(&path, 0).unwrap();
        assert_eq!(text.as_deref(), Some("Pushed 3f2a9c1."));
        let complete = format!("{first}\n{tool}\n{last}\n").len() as u64;
        assert_eq!(
            end, complete,
            "a partial final line is left for the next read"
        );
        let (text, again) = last_assistant_text(&path, end).unwrap();
        assert_eq!((text, again), (None, end));
        let after_first = (first.len() + 1) as u64;
        let (text, _) = last_assistant_text(&path, after_first).unwrap();
        assert_eq!(text.as_deref(), Some("Pushed 3f2a9c1."));
    }

    #[test]
    fn a_sample_joins_claude_state_to_herdr_panes_through_proc() {
        let directory = scratch();
        let herdr = directory.join("herdr");
        let claude = directory.join("claude");
        let listing = r#"{"result":{"agents":[
            {"pane_id":"w:p1","name":"coord","agent":"claude","agent_status":"idle"},
            {"pane_id":"w:p2","name":"busy-one","agent":"claude","agent_status":"idle"},
            {"pane_id":"w:p3","name":"codex-one","agent":"codex","agent_status":"blocked"},
            {"pane_id":"w:p4","agent_status":"idle"}
        ]}}"#;
        fs::write(directory.join("list.json"), listing).unwrap();
        fs::write(
            &herdr,
            format!("#!/bin/sh\ncat {}\n", directory.join("list.json").display()),
        )
        .unwrap();
        fs::write(
            &claude,
            "#!/bin/sh\necho '[{\"pid\":\"101\",\"sessionId\":\"s-1\",\"status\":\"busy\"},{\"id\":\"bg\",\"state\":\"blocked\"}]'\n",
        )
        .unwrap();
        for program in [&herdr, &claude] {
            fs::set_permissions(program, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let proc_root = directory.join("proc");
        fs::create_dir_all(proc_root.join("101")).unwrap();
        fs::write(
            proc_root.join("101/environ"),
            b"A=1\0HERDR_PANE_ID=w:p2\0B=2\0",
        )
        .unwrap();
        let projects = directory.join("projects");
        fs::create_dir_all(projects.join("p")).unwrap();
        fs::write(projects.join("p/s-1.jsonl"), b"").unwrap();
        let sources = Sources {
            herdr: &herdr,
            claude: &claude,
            projects: &projects,
            proc_root: &proc_root,
        };
        let observations = sample(&sources, &["coord".to_owned()]).unwrap();
        let summary = observations
            .iter()
            .map(|observation| {
                (
                    observation.agent.as_str(),
                    observation.state,
                    observation.authoritative,
                    observation.transcript.is_some(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            summary,
            [
                ("busy-one", WorkerState::Working, true, true),
                ("codex-one", WorkerState::Blocked, false, false)
            ],
            "Claude's busy overrides Herdr's idle; the coordinator and agentless panes are skipped"
        );
    }
}
