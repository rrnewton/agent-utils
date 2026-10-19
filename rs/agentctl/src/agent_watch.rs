//! Tell the coordinator when a registered agent needs a human (`agentctl watch-agents`).
//!
//! `agentctl watch` keeps the coordinator's environment alive; this watch keeps an eye on
//! the agents themselves. Every pass it lists the registry, reads each running agent's
//! visible screen, and classifies the agent as at a harness approval or permission prompt,
//! idle without a verdict, stalled, dead, or healthy. A condition that holds past its
//! threshold becomes one notice to the coordinator, sent through the same durable queue
//! as `agentctl send`, so a busy coordinator loses nothing: the notice waits in its queue.
//!
//! The watcher never answers a prompt and never types into any agent except the
//! coordinator. Its episode memory (when each condition was first seen, whether it was
//! already reported, notices still waiting to be sent) is persisted after every pass, so
//! restarting the watcher does not report the same episode twice.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{AgentError, DrainOptions, QueueMessageState};
use crate::subagents::{ManagedAgents, ManagedApi};

/// Schema of the persisted watcher state and of the per-pass report.
pub const STATE_SCHEMA: &str = "agentctl-agent-watch/v1";

/// Visible screen lines read per agent per pass.
const SCREEN_LINES: usize = 200;
/// Bottom screen lines scanned for an approval or permission prompt. Harnesses draw
/// prompts at the bottom of the screen; scanning the whole transcript would match an
/// agent merely discussing a prompt.
const PROMPT_WINDOW_LINES: usize = 12;
/// Bottom screen lines scanned for a completion marker.
const MARKER_WINDOW_LINES: usize = 30;
/// Seconds the coordinator is given to accept a notice before it simply stays queued.
const NOTICE_READY_SECONDS: u64 = 10;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// How `agentctl watch-agents` was asked to run.
#[derive(Clone, Debug)]
pub struct WatchAgentsOptions {
    /// Registered name of the coordinator that receives notices.
    pub coordinator: String,
    /// Seconds between two passes.
    pub interval: Duration,
    /// Run one pass, print it, and exit.
    pub once: bool,
    /// Report only: send no notice.
    pub check_only: bool,
    /// How long an approval or permission prompt may hold before it is reported.
    pub approval: Duration,
    /// How long an agent may sit idle without a verdict before it is reported.
    pub idle: Duration,
    /// How long a working agent may show no new output before it is reported.
    pub stall: Duration,
    /// How long a running record's harness may be unreachable before it is reported dead.
    pub dead: Duration,
    /// Line prefixes that count as a verdict, such as `DONE:` and `BLOCKED:`.
    pub markers: Vec<String>,
    /// Minimum seconds between two messages to the coordinator; notices batch and wait.
    pub min_notice_interval: Duration,
    /// Where `agent-watch-state.json` and `agent-watch.json` are written
    /// (default `$XDG_STATE_HOME/agentctl/agent-watch`).
    pub state_dir: Option<PathBuf>,
}

impl WatchAgentsOptions {
    fn threshold(&self, kind: EpisodeKind) -> Duration {
        match kind {
            EpisodeKind::ApprovalPrompt => self.approval,
            EpisodeKind::IdleNoVerdict => self.idle,
            EpisodeKind::Stalled => self.stall,
            EpisodeKind::Dead => self.dead,
        }
    }
}

/// One condition that wants a human. Serialized spellings are the report's contract.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EpisodeKind {
    /// The agent sits at a harness approval, permission, or trust prompt.
    ApprovalPrompt,
    /// The agent went idle or done without a verdict marker in its last reply.
    IdleNoVerdict,
    /// The agent is marked working but its screen has not changed.
    Stalled,
    /// The record says running but the harness cannot be reached.
    Dead,
}

impl EpisodeKind {
    /// Stable machine-readable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApprovalPrompt => "approval-prompt",
            Self::IdleNoVerdict => "idle-no-verdict",
            Self::Stalled => "stalled",
            Self::Dead => "dead",
        }
    }

    fn describe(self, options: &WatchAgentsOptions) -> String {
        let minutes = |duration: Duration| duration.as_secs() / 60;
        match self {
            Self::ApprovalPrompt => format!(
                "at a harness approval or permission prompt for over {} minutes",
                minutes(options.approval)
            ),
            Self::IdleNoVerdict => format!(
                "idle for over {} minutes without a {} verdict",
                minutes(options.idle),
                options.markers.join(" or ")
            ),
            Self::Stalled => format!(
                "marked working with no new output for over {} minutes",
                minutes(options.stall)
            ),
            Self::Dead => format!(
                "unreachable for over {} minutes while its record says running",
                minutes(options.dead)
            ),
        }
    }
}

/// What one pass observed about one agent. The episode logic reads only this, so tests
/// drive it with fixtures instead of a terminal server.
#[derive(Clone, Debug)]
pub struct Observation {
    /// Registered agent name.
    pub name: String,
    /// Live agent state (`working`, `idle`, `done`, ...), or `gone` when a running
    /// record's harness could not be reached.
    pub status: String,
    /// Visible screen text, when it could be read.
    pub screen: Option<String>,
    /// Whether the agent has ever confirmed a delivery (its queue's processed set is
    /// non-empty). An agent that was never sent work is idle legitimately.
    pub has_delivered: bool,
    /// Whether prompts still wait in the agent's queue. Idle with work queued is the
    /// delivery path's business, not a missing verdict.
    pub has_pending: bool,
    /// Probe detail carried into a notice, such as the error that made it `gone`.
    pub detail: String,
}

/// Prompt phrases, lowercased, matched against the bottom of the screen. Each is a
/// literal a supported harness is already known to draw: Muse's approval and trust
/// prompts, Claude Code's permission dialog, and the folder-trust screens.
const APPROVAL_PATTERNS: [&str; 10] = [
    "would you like to run",
    "do you want to proceed",
    "approval required",
    "requires approval",
    "allow command?",
    "approve this",
    "yes, i trust this folder",
    "trust this workspace",
    "do you trust",
    "workspace trust",
];

/// The prompt line at the bottom of `screen`, trimmed and bounded, when one is showing.
fn approval_prompt_line(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(PROMPT_WINDOW_LINES);
    lines[start..].iter().rev().find_map(|line| {
        let lowered = line.to_ascii_lowercase();
        APPROVAL_PATTERNS
            .iter()
            .any(|pattern| lowered.contains(pattern))
            .then(|| line.trim().chars().take(160).collect())
    })
}

/// Whether a verdict marker starts a line near the bottom of `screen`.
fn has_verdict(screen: &str, markers: &[String]) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(MARKER_WINDOW_LINES);
    lines[start..].iter().any(|line| {
        let line = line.trim_start();
        markers.iter().any(|marker| line.starts_with(marker))
    })
}

/// FNV-1a over `text`. Persisted hashes must read the same in every process, so the
/// standard library's per-process-keyed hasher cannot be used for them.
fn stable_hash(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

fn screen_hash(screen: Option<&str>) -> u64 {
    // The leading byte keeps an unreadable screen distinct from an empty one.
    match screen {
        Some(text) => stable_hash(&format!("s{text}")),
        None => stable_hash("n"),
    }
}

/// One live episode: a condition first seen at `since`, reported at most once.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct Episode {
    kind: EpisodeKind,
    since: f64,
    notified: bool,
}

/// What the watcher remembers about one agent between passes.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AgentMemory {
    episode: Option<Episode>,
    screen_hash: u64,
    screen_changed_at: f64,
}

/// One notice that is owed to the coordinator and waiting to be durably queued for it.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PendingNotice {
    agent: String,
    kind: EpisodeKind,
    since: f64,
    detail: String,
}

impl PendingNotice {
    fn key(&self) -> String {
        format!(
            "{}:{}:{}",
            self.agent,
            self.kind.as_str(),
            self.since as u64
        )
    }
}

/// A batch of notices whose send was attempted; retried under the same message ID, so a
/// crash after queueing cannot send it twice.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct InflightBatch {
    message_id: String,
    keys: Vec<String>,
}

/// The persisted watcher state.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct WatchState {
    agents: BTreeMap<String, AgentMemory>,
    pending: Vec<PendingNotice>,
    inflight: Option<InflightBatch>,
    last_notice_at: Option<f64>,
}

/// Where observations come from; the production source reads the registry through Herdr.
pub trait Probe {
    /// Observe every agent that should be watched this pass. An error means the team
    /// could not be listed at all; the pass reports it and starts no episodes.
    fn observe(&mut self) -> Result<Vec<Observation>>;
}

/// Where notices go; the production sink queues one message for the coordinator.
pub trait Notifier {
    /// Durably queue `text` for `coordinator` under `message_id` and return the queue
    /// outcome spelling. An error means nothing was queued and the batch is retried.
    fn notify(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String>;
}

/// The watcher's clock, injectable for tests.
pub trait Clock {
    /// Seconds since the Unix epoch.
    fn now(&self) -> f64;
    /// Pause between passes.
    fn sleep(&self, duration: Duration);
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The condition one observation shows, with the line or error that proves it.
/// Screen changes are handled by the caller: a changed screen ends a stall episode.
fn classify(
    observation: &Observation,
    options: &WatchAgentsOptions,
) -> Option<(EpisodeKind, String)> {
    if observation.status == "gone" {
        return Some((EpisodeKind::Dead, observation.detail.clone()));
    }
    if let Some(screen) = observation.screen.as_deref() {
        if let Some(line) = approval_prompt_line(screen) {
            return Some((EpisodeKind::ApprovalPrompt, line));
        }
    }
    if matches!(observation.status.as_str(), "idle" | "done")
        && observation.has_delivered
        && !observation.has_pending
        && observation.screen.is_some()
    {
        let verdict = observation
            .screen
            .as_deref()
            .is_some_and(|screen| has_verdict(screen, &options.markers));
        if !verdict {
            return Some((
                EpisodeKind::IdleNoVerdict,
                format!("status {} with no verdict marker", observation.status),
            ));
        }
    }
    if observation.status == "working" && observation.screen.is_some() {
        return Some((EpisodeKind::Stalled, "screen unchanged".to_owned()));
    }
    None
}

/// Withdraw the unsent notice owed for `episode`, if any is still waiting.
fn withdraw_pending(state: &mut WatchState, agent: &str, episode: &Episode) {
    let key = format!(
        "{}:{}:{}",
        agent,
        episode.kind.as_str(),
        episode.since as u64
    );
    state.pending.retain(|notice| notice.key() != key);
}

/// Append at most one new notice for `observation`, after updating its episode memory.
/// Returns the live episode row for the report, past-threshold or not.
fn update_episode(
    state: &mut WatchState,
    observation: &Observation,
    now: f64,
    options: &WatchAgentsOptions,
) -> Option<Value> {
    let hash = screen_hash(observation.screen.as_deref());
    let mut memory = state
        .agents
        .get(&observation.name)
        .cloned()
        .unwrap_or_default();
    // An unreadable screen is no evidence either way: a transient read failure must
    // neither clear a live episode nor restart its clock. The episode freezes until
    // the screen can be read again; only `gone` classifies without one.
    if observation.screen.is_none() && observation.status != "gone" {
        return memory.episode.as_ref().map(|episode| {
            json!({
                "agent": observation.name,
                "kind": episode.kind.as_str(),
                "status": observation.status,
                "since": episode.since,
                "age_seconds": now - episode.since,
                "reported": episode.notified,
                "due": now - episode.since >= options.threshold(episode.kind).as_secs_f64(),
                "detail": "screen unreadable this pass; episode held",
            })
        });
    }
    if memory.screen_hash != hash {
        memory.screen_hash = hash;
        memory.screen_changed_at = now;
        // New output is progress: a stall, if any, is a new episode from here,
        // and its unsent notice is withdrawn with it.
        if let Some(episode) = &memory.episode {
            if episode.kind == EpisodeKind::Stalled {
                let cleared = episode.clone();
                withdraw_pending(state, &observation.name, &cleared);
                memory.episode = None;
            }
        }
    }
    let found = classify(observation, options);
    // Recovered, or moved to a different condition: an unsent notice for the old
    // episode is withdrawn, so a rate-limit window cannot deliver stale news. A
    // notice already queued for the coordinator cannot be unsent.
    if let Some(cleared) = &memory.episode {
        if found.as_ref().is_none_or(|(kind, _)| *kind != cleared.kind) {
            let cleared = cleared.clone();
            withdraw_pending(state, &observation.name, &cleared);
        }
    }
    let Some((kind, detail)) = found else {
        memory.episode = None;
        state.agents.insert(observation.name.clone(), memory);
        return None;
    };
    // A stall is timed from the last screen change, not from the first working poll:
    // an agent that works for an hour with changing output, then stops changing,
    // has been stalled only since it stopped.
    let since = if kind == EpisodeKind::Stalled {
        memory.screen_changed_at
    } else {
        match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since,
            _ => now,
        }
    };
    let notified =
        matches!(&memory.episode, Some(episode) if episode.kind == kind && episode.notified);
    memory.episode = Some(Episode {
        kind,
        since,
        notified,
    });
    let age = now - since;
    let due = age >= options.threshold(kind).as_secs_f64();
    // A check-only pass reports what is due without consuming the episode's one
    // report: nothing is queued and `notified` is left alone.
    if due && !notified && !options.check_only {
        let notice = PendingNotice {
            agent: observation.name.clone(),
            kind,
            since,
            detail: detail.clone(),
        };
        if !state
            .pending
            .iter()
            .any(|pending| pending.key() == notice.key())
            && !state
                .inflight
                .as_ref()
                .is_some_and(|batch| batch.keys.contains(&notice.key()))
        {
            state.pending.push(notice);
        }
        if let Some(episode) = &mut memory.episode {
            episode.notified = true;
        }
    }
    let row = memory.episode.as_ref().map(|episode| {
        json!({
            "agent": observation.name,
            "kind": episode.kind.as_str(),
            "status": observation.status,
            "since": episode.since,
            "age_seconds": age,
            "reported": episode.notified,
            "due": due,
            "detail": detail,
        })
    });
    state.agents.insert(observation.name.clone(), memory);
    row
}

/// Send every pending notice as one message, when the rate limit allows. Returns the
/// report row; on failure the batch stays in flight and is retried next pass.
fn flush_notices(
    state: &mut WatchState,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Value {
    if options.check_only {
        return json!({"sent": false, "reason": "check only", "waiting": state.pending.len()});
    }
    if state.inflight.is_none() {
        if state.pending.is_empty() {
            return json!({"sent": false, "reason": "nothing to report"});
        }
        if state
            .last_notice_at
            .is_some_and(|last| now - last < options.min_notice_interval.as_secs_f64())
        {
            return json!({"sent": false, "reason": "rate limited", "waiting": state.pending.len()});
        }
        // The batch ID is derived from the notices themselves, so a watcher killed
        // between queueing a batch and persisting that fact re-derives the same ID
        // after a restart and the coordinator's queue recognises it as sent.
        let keys: Vec<String> = state.pending.iter().map(PendingNotice::key).collect();
        state.inflight = Some(InflightBatch {
            message_id: format!("agent-watch-{:016x}", stable_hash(&keys.join("\n"))),
            keys,
        });
    }
    let batch = state.inflight.clone().expect("batch prepared");
    let notices: Vec<&PendingNotice> = state
        .pending
        .iter()
        .filter(|notice| batch.keys.contains(&notice.key()))
        .collect();
    if notices.is_empty() {
        state.inflight = None;
        return json!({"sent": false, "reason": "nothing to report"});
    }
    let lines = notices
        .iter()
        .map(|notice| {
            format!(
                "- {}: {} ({})",
                notice.agent,
                notice.kind.describe(options),
                notice.detail
            )
        })
        .collect::<Vec<_>>();
    let text = format!(
        "[agentctl watch-agents] {} agent(s) need attention:\n{}\nInspect with `agentctl status NAME` and `agentctl read NAME`. The watcher never answers prompts itself.",
        lines.len(),
        lines.join("\n")
    );
    match notifier.notify(&options.coordinator, &text, &batch.message_id) {
        Ok(outcome) => {
            state
                .pending
                .retain(|notice| !batch.keys.contains(&notice.key()));
            state.inflight = None;
            state.last_notice_at = Some(now);
            json!({"sent": true, "message_id": batch.message_id, "outcome": outcome, "notices": lines.len()})
        }
        Err(error) => {
            json!({"sent": false, "message_id": batch.message_id, "reason": error.to_string(), "waiting": state.pending.len()})
        }
    }
}

/// One pass: observe, update episodes, flush due notices, and build the report.
fn watch_agents_once(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    state: &mut WatchState,
    now: f64,
    options: &WatchAgentsOptions,
) -> Value {
    let (observations, probe_error) = match probe.observe() {
        Ok(observations) => (observations, Value::Null),
        Err(error) => (Vec::new(), json!(error.to_string())),
    };
    let seen: Vec<String> = observations.iter().map(|row| row.name.clone()).collect();
    let mut episodes = Vec::new();
    if probe_error.is_null() {
        for observation in &observations {
            if let Some(row) = update_episode(state, observation, now, options) {
                episodes.push(row);
            }
        }
        // Memory for an agent missing from one pass (paused, mid-move, a listing
        // hiccup) is kept while it still owes or holds a report, so one absent
        // pass can neither drop an owed notice nor restart an episode clock.
        // Pending notices are never pruned here: recovery withdraws them, and a
        // notice about an agent that has since stopped is still true.
        state.agents.retain(|name, memory| {
            seen.contains(name)
                || memory.episode.is_some()
                || state.pending.iter().any(|notice| &notice.agent == name)
        });
    }
    let notice = flush_notices(state, notifier, now, options);
    json!({
        "schema": STATE_SCHEMA,
        "checked_at": now,
        "coordinator": options.coordinator,
        "ok": probe_error.is_null()
            && episodes.iter().all(|row| row["reported"] != true && row["due"] != true),
        "probe_error": probe_error,
        "agents": seen.len(),
        "episodes": episodes,
        "notice": notice,
        "waiting": state.pending.len(),
    })
}

fn state_paths(options: &WatchAgentsOptions) -> Option<(PathBuf, PathBuf)> {
    let directory = options.state_dir.clone().or_else(|| {
        crate::restore::default_report_dir()
            .and_then(|dir| dir.parent().map(|parent| parent.join("agent-watch")))
    })?;
    Some((
        directory.join("agent-watch-state.json"),
        directory.join("agent-watch.json"),
    ))
}

fn load_state(path: &Path) -> WatchState {
    let mut text = String::new();
    if File::open(path)
        .ok()
        .and_then(|file| file.take(1 << 20).read_to_string(&mut text).ok())
        .is_none()
    {
        return WatchState::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_json_file(path: &Path, value: &Value) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| fail("state path has no directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| fail(format!("cannot create {}: {error}", directory.display())))?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent-watch"),
        std::process::id()
    ));
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| fail(error.to_string()))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    fs::rename(&temporary, path)
        .map_err(|error| fail(format!("cannot publish {}: {error}", path.display())))
}

/// Run the agent watch: one pass with `once`, otherwise forever, one pass per interval.
pub fn watch_agents(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    clock: &dyn Clock,
    options: &WatchAgentsOptions,
) -> Result<Value> {
    use fs2::FileExt;
    let paths = state_paths(options);
    // One watcher per state directory: two writers would take turns overwriting
    // each other's episode memory. The lock is held for the process's lifetime.
    let _lock = match paths.as_ref() {
        Some((state_path, _)) => {
            let directory = state_path.parent().expect("state path has a directory");
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)
                .map_err(|error| fail(format!("cannot create {}: {error}", directory.display())))?;
            let lock_path = directory.join("agent-watch.lock");
            let lock = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&lock_path)
                .map_err(|error| fail(format!("cannot open {}: {error}", lock_path.display())))?;
            lock.try_lock_exclusive().map_err(|_| {
                fail(format!(
                    "another watch-agents already holds {}; refusing to run a second watcher on the same state",
                    lock_path.display()
                ))
            })?;
            Some(lock)
        }
        None => None,
    };
    let mut state = paths
        .as_ref()
        .map(|(state_path, _)| load_state(state_path))
        .unwrap_or_default();
    loop {
        let now = clock.now();
        let report = watch_agents_once(probe, notifier, &mut state, now, options);
        if let Some(error) = report["probe_error"].as_str() {
            eprintln!("agentctl watch-agents: {error}");
        }
        for row in report["episodes"].as_array().into_iter().flatten() {
            if row["reported"] == true {
                eprintln!(
                    "agentctl watch-agents: {} {}: {}",
                    row["agent"].as_str().unwrap_or("?"),
                    row["kind"].as_str().unwrap_or("?"),
                    row["detail"].as_str().unwrap_or("")
                );
            }
        }
        if let Some((state_path, report_path)) = paths.as_ref() {
            let state_value =
                serde_json::to_value(&state).map_err(|error| fail(error.to_string()))?;
            if let Err(error) = write_json_file(state_path, &state_value)
                .and_then(|()| write_json_file(report_path, &report))
            {
                eprintln!("agentctl watch-agents: {error}");
            }
        }
        if options.once {
            return Ok(report);
        }
        clock.sleep(options.interval);
    }
}

/// The production probe: one registry, read through Herdr. The coordinator itself,
/// paused agents, cloud sessions viewed through a terminal, and records that are not
/// running are not watched; a `launch_failed` record was already reported by `start`.
pub struct ManagedProbe<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
    coordinator: String,
}

impl<'a, A: ManagedApi + ?Sized> ManagedProbe<'a, A> {
    /// Watch `registry` through `client`, exempting `coordinator`.
    pub fn new(client: &'a A, registry: &Path, coordinator: &str) -> Self {
        Self {
            client,
            registry: registry.to_owned(),
            coordinator: coordinator.to_owned(),
        }
    }
}

fn queue_has_entries(queue: &Path, child: &str) -> bool {
    fs::read_dir(queue.join(child)).is_ok_and(|mut entries| entries.next().is_some())
}

impl<A: ManagedApi + ?Sized> Probe for ManagedProbe<'_, A> {
    fn observe(&mut self) -> Result<Vec<Observation>> {
        let manager = ManagedAgents::new(self.client, &self.registry)?;
        let rows = manager.list()?;
        let watched: Vec<&Value> = rows
            .iter()
            .filter(|row| {
                if row["name"].as_str() == Some(self.coordinator.as_str())
                    || row["paused"] == true
                    // A mid-move agent is being handled by `agentctl move`; its
                    // unknown status is not a death.
                    || row["move_pending"] == true
                {
                    return false;
                }
                // A record this build cannot read is watched as unreachable rather
                // than silently dropped: the coordinator should hear about it.
                row["record_error"] == true
                    || (matches!(row["lifecycle"].as_str(), Some("running" | "starting"))
                        && row["adapter"].as_str() != Some("agentcloud"))
            })
            .collect();
        // When every watched agent is unreachable at once the terminal server, not
        // every harness, is gone; `agentctl watch` reports and repairs that.
        let all_gone = !watched.is_empty()
            && watched
                .iter()
                .all(|row| row["agent_status"] == "unknown" && !row["probe_error"].is_null());
        if all_gone {
            return Err(fail(
                "no watched agent could be probed; the terminal server may be down",
            ));
        }
        let mut observations = Vec::with_capacity(watched.len());
        for row in watched {
            let name = row["name"].as_str().unwrap_or_default().to_owned();
            let gone = row["agent_status"] == "unknown" && !row["probe_error"].is_null();
            let queue = row["queue"]
                .as_str()
                .map_or_else(|| self.registry.join(&name).join("queue"), PathBuf::from);
            observations.push(Observation {
                detail: if row["record_error"] == true {
                    format!(
                        "record could not be read: {}",
                        row["probe_error"].as_str().unwrap_or("unknown error")
                    )
                    .chars()
                    .take(200)
                    .collect()
                } else if gone {
                    row["probe_error"]
                        .as_str()
                        .unwrap_or("unreachable")
                        .chars()
                        .take(200)
                        .collect()
                } else {
                    String::new()
                },
                status: if gone {
                    "gone".to_owned()
                } else {
                    row["agent_status"].as_str().unwrap_or("unknown").to_owned()
                },
                screen: if gone {
                    None
                } else {
                    manager.read(&name, SCREEN_LINES).ok()
                },
                has_delivered: queue_has_entries(&queue, "processed"),
                has_pending: ["pending", "inflight"].iter().any(|field| {
                    row[field]
                        .as_array()
                        .is_some_and(|values| !values.is_empty())
                }),
                name,
            });
        }
        Ok(observations)
    }
}

/// The production notifier: the same durable queue as `agentctl send`. A coordinator
/// that is busy keeps the notice queued; only the coordinator is ever typed into.
pub struct ManagedNotifier<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
}

impl<'a, A: ManagedApi + ?Sized> ManagedNotifier<'a, A> {
    /// Queue notices for coordinators registered in `registry`.
    pub fn new(client: &'a A, registry: &Path) -> Self {
        Self {
            client,
            registry: registry.to_owned(),
        }
    }
}

/// Attempts a batch is re-queued under a fresh ID after landing in the queue's
/// failed set. A failed entry may have been typed, so agentctl never replays it
/// under its own ID; a watcher notice is safe to repeat, but not forever.
const NOTICE_REQUEUE_ATTEMPTS: u32 = 3;

/// The queue ID for attempt `attempt` of one batch: the batch ID itself, then
/// deterministic re-queue suffixes, so a restart walks the same chain.
fn attempt_message_id(message_id: &str, attempt: u32) -> String {
    if attempt == 0 {
        message_id.to_owned()
    } else {
        format!("{message_id}-r{attempt}")
    }
}

impl<A: ManagedApi + ?Sized> Notifier for ManagedNotifier<'_, A> {
    fn notify(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String> {
        let manager = ManagedAgents::new(self.client, &self.registry)?;
        for attempt in 0..=NOTICE_REQUEUE_ATTEMPTS {
            let attempt_id = attempt_message_id(message_id, attempt);
            // A previous attempt may have queued this batch just before the watcher
            // was stopped; the recorded ID proves it either way.
            match manager.message_state(coordinator, &attempt_id) {
                Ok(Some(QueueMessageState::Pending)) => return Ok("already-queued".to_owned()),
                Ok(Some(QueueMessageState::Processed)) => {
                    return Ok("already-delivered".to_owned());
                }
                // Still crossing the injection barrier: neither sent nor lost yet.
                // Keep the batch and recheck next pass instead of forgetting it.
                Ok(Some(QueueMessageState::Inflight)) => {
                    return Err(fail(
                        "the notice is still in flight in the coordinator's queue; will recheck",
                    ));
                }
                // Quarantined as possibly submitted: that ID can never deliver, so
                // fall through to the next re-queue ID rather than dropping the
                // notice. A repeated notice is a lesser harm than a lost one.
                Ok(Some(QueueMessageState::Failed)) => continue,
                Ok(None) => {
                    let result = manager.send_identified(
                        coordinator,
                        text,
                        DrainOptions {
                            ready_timeout: Duration::from_secs(NOTICE_READY_SECONDS),
                            deliver_while_working: true,
                            ..DrainOptions::default()
                        },
                        Some(&attempt_id),
                    )?;
                    return Ok(if attempt == 0 {
                        result.outcome.as_str().to_owned()
                    } else {
                        format!("requeued-after-failed:{}", result.outcome.as_str())
                    });
                }
                Err(error) => return Err(error),
            }
        }
        Err(fail(format!(
            "the notice landed in the coordinator's failed set {NOTICE_REQUEUE_ATTEMPTS} times; giving up this pass"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> WatchAgentsOptions {
        WatchAgentsOptions {
            coordinator: "coordinator".to_owned(),
            interval: Duration::from_secs(60),
            once: true,
            check_only: false,
            approval: Duration::from_secs(300),
            idle: Duration::from_secs(900),
            stall: Duration::from_secs(1800),
            dead: Duration::from_secs(300),
            markers: vec!["DONE:".to_owned(), "BLOCKED:".to_owned()],
            min_notice_interval: Duration::ZERO,
            state_dir: None,
        }
    }

    fn observation(name: &str, status: &str, screen: Option<&str>) -> Observation {
        Observation {
            name: name.to_owned(),
            status: status.to_owned(),
            screen: screen.map(str::to_owned),
            has_delivered: true,
            has_pending: false,
            detail: String::new(),
        }
    }

    struct ScriptedProbe(Vec<Vec<Observation>>);

    impl Probe for ScriptedProbe {
        fn observe(&mut self) -> Result<Vec<Observation>> {
            Ok(if self.0.is_empty() {
                Vec::new()
            } else {
                self.0.remove(0)
            })
        }
    }

    #[derive(Default)]
    struct RecordingNotifier {
        sent: Vec<(String, String, String)>,
        attempts: Vec<String>,
        fail_attempts: usize,
    }

    impl Notifier for RecordingNotifier {
        fn notify(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String> {
            self.attempts.push(message_id.to_owned());
            if self.fail_attempts > 0 {
                self.fail_attempts -= 1;
                return Err(fail("coordinator unreachable"));
            }
            self.sent.push((
                coordinator.to_owned(),
                text.to_owned(),
                message_id.to_owned(),
            ));
            Ok("pending".to_owned())
        }
    }

    fn pass(
        probe: &mut ScriptedProbe,
        notifier: &mut RecordingNotifier,
        state: &mut WatchState,
        now: f64,
        options: &WatchAgentsOptions,
    ) -> Value {
        watch_agents_once(probe, notifier, state, now, options)
    }

    const PROMPT_SCREEN: &str = "◆ some earlier work\nWould you like to run the following command?\n  $ make validate\n❯ Yes\n  No\n";
    const IDLE_SCREEN: &str = "◆ finished the thing\n❯\n";
    const DONE_SCREEN: &str = "◆ finished the thing\nDONE: the thing is complete\n❯\n";

    #[test]
    fn an_approval_prompt_is_reported_once_per_episode() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "working", Some("◆ running again\n"))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let first = pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert_eq!(first["episodes"][0]["reported"], false, "{first}");
        assert!(notifier.sent.is_empty());
        pass(&mut probe, &mut notifier, &mut state, 299.0, &options);
        assert!(
            notifier.sent.is_empty(),
            "below the threshold nothing is sent"
        );
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(due["notice"]["notices"], 1, "{due}");
        assert_eq!(notifier.sent.len(), 1);
        assert!(
            notifier.sent[0].1.contains("worker"),
            "{}",
            notifier.sent[0].1
        );
        assert!(
            notifier.sent[0].1.contains("Would you like to run"),
            "{}",
            notifier.sent[0].1
        );
        assert_eq!(
            notifier.sent[0].0, "coordinator",
            "only the coordinator is typed into"
        );
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 420.0, &options);
        assert_eq!(
            notifier.sent.len(),
            1,
            "a fresh prompt below its threshold sends nothing"
        );
        pass(&mut probe, &mut notifier, &mut state, 720.0, &options);
        assert_eq!(
            notifier.sent.len(),
            2,
            "a cleared episode can be reported again"
        );
    }

    #[test]
    fn a_working_agent_with_changing_output_is_never_reported() {
        let options = options();
        let mut probe = ScriptedProbe(
            (0..10)
                .map(|index| {
                    vec![observation(
                        "worker",
                        "working",
                        Some(&format!("◆ step {index} of the build\n")),
                    )]
                })
                .collect(),
        );
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        for step in 0..10 {
            let report = pass(
                &mut probe,
                &mut notifier,
                &mut state,
                f64::from(step) * 3600.0,
                &options,
            );
            assert_eq!(report["ok"], true, "{report}");
        }
        assert!(
            notifier.sent.is_empty(),
            "ten hours of changing output sent {:?}",
            notifier.sent
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn a_stall_is_timed_from_the_last_output_not_the_first_working_poll() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "working", Some("◆ one\n"))],
            vec![observation("worker", "working", Some("◆ two\n"))],
            vec![observation("worker", "working", Some("◆ two\n"))],
            vec![observation("worker", "working", Some("◆ two\n"))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        // Output changes an hour in: the stall clock starts here, not at 0.
        pass(&mut probe, &mut notifier, &mut state, 3600.0, &options);
        pass(
            &mut probe,
            &mut notifier,
            &mut state,
            3600.0 + 1799.0,
            &options,
        );
        assert!(notifier.sent.is_empty());
        let due = pass(
            &mut probe,
            &mut notifier,
            &mut state,
            3600.0 + 1800.0,
            &options,
        );
        assert_eq!(due["episodes"][0]["kind"], "stalled", "{due}");
        assert_eq!(notifier.sent.len(), 1);
        assert!(
            notifier.sent[0].1.contains("no new output"),
            "{}",
            notifier.sent[0].1
        );
    }

    #[test]
    fn idle_needs_a_verdict_only_after_work_was_delivered_and_drained() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();

        // A verdict at the bottom of the screen: never reported.
        let mut probe = ScriptedProbe(vec![
            vec![observation("done-agent", "idle", Some(DONE_SCREEN))],
            vec![observation("done-agent", "idle", Some(DONE_SCREEN))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 7200.0, &options);
        assert_eq!(report["episodes"], json!([]), "{report}");

        // Never sent work, and work still queued: never reported.
        let mut fresh = observation("fresh", "idle", Some(IDLE_SCREEN));
        fresh.has_delivered = false;
        let mut queued = observation("queued", "idle", Some(IDLE_SCREEN));
        queued.has_pending = true;
        let mut probe = ScriptedProbe(vec![
            vec![fresh.clone(), queued.clone()],
            vec![fresh, queued],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 7200.0, &options);
        assert!(notifier.sent.is_empty());

        // Delivered, drained, idle, no verdict: reported after the idle threshold.
        let mut probe = ScriptedProbe(vec![
            vec![observation("quiet", "done", Some(IDLE_SCREEN))],
            vec![observation("quiet", "done", Some(IDLE_SCREEN))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 900.0, &options);
        assert_eq!(due["episodes"][0]["kind"], "idle-no-verdict", "{due}");
        assert_eq!(notifier.sent.len(), 1);
        assert!(
            notifier.sent[0].1.contains("DONE: or BLOCKED:"),
            "{}",
            notifier.sent[0].1
        );
    }

    #[test]
    fn a_dead_harness_is_reported_once() {
        let options = options();
        let gone = || {
            let mut row = observation("worker", "gone", None);
            row.detail = "pane is gone".to_owned();
            row
        };
        let mut probe = ScriptedProbe(vec![vec![gone()], vec![gone()], vec![gone()]]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(due["episodes"][0]["kind"], "dead", "{due}");
        pass(&mut probe, &mut notifier, &mut state, 600.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        assert!(
            notifier.sent[0].1.contains("pane is gone"),
            "{}",
            notifier.sent[0].1
        );
    }

    #[test]
    fn a_failed_send_keeps_the_notice_queued_under_the_same_id() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier {
            fail_attempts: 1,
            ..RecordingNotifier::default()
        };
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let failed = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(failed["notice"]["sent"], false, "{failed}");
        assert_eq!(state.pending.len(), 1, "the notice is not lost");
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        assert_eq!(notifier.attempts.len(), 2);
        assert_eq!(
            notifier.attempts[0], notifier.attempts[1],
            "the retry reuses the message ID"
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn notices_batch_and_wait_out_the_rate_limit() {
        let mut options = options();
        options.min_notice_interval = Duration::from_secs(600);
        let pair = || {
            vec![
                observation("one", "idle", Some(PROMPT_SCREEN)),
                observation("two", "idle", Some(PROMPT_SCREEN)),
            ]
        };
        let trio = || {
            let mut rows = pair();
            rows.push(observation("three", "idle", Some(PROMPT_SCREEN)));
            rows
        };
        let mut probe = ScriptedProbe(vec![pair(), pair(), trio(), trio(), trio()]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(due["notice"]["notices"], 2, "{due}");
        assert_eq!(notifier.sent.len(), 1, "both episodes ride in one message");
        // `three` has been stuck since its first poll at 360; it comes due at 660,
        // inside the rate-limit window that ends at 900.
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        let held = pass(&mut probe, &mut notifier, &mut state, 660.0, &options);
        assert_eq!(held["notice"]["reason"], "rate limited", "{held}");
        assert_eq!(notifier.sent.len(), 1);
        pass(&mut probe, &mut notifier, &mut state, 900.0, &options);
        assert_eq!(notifier.sent.len(), 2);
        assert!(
            notifier.sent[1].1.contains("three"),
            "{}",
            notifier.sent[1].1
        );
    }

    #[test]
    fn a_prompt_phrase_deep_in_the_transcript_is_not_a_prompt() {
        // An agent discussing approval prompts quotes the phrase far above its
        // composer; only the bottom of the screen can hold a live prompt.
        let mut screen = String::from("Would you like to run the following command?\n");
        for index in 0..40 {
            screen.push_str(&format!("◆ discussed line {index}\n"));
        }
        screen.push_str("DONE: discussed it\n❯\n");
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(&screen))],
            vec![observation("worker", "idle", Some(&screen))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 7200.0, &options);
        assert_eq!(report["episodes"], json!([]), "{report}");
        assert!(notifier.sent.is_empty());
    }

    #[test]
    fn a_restart_does_not_report_an_episode_twice() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        // The watcher restarts: state comes back from its JSON file unchanged.
        let restored: WatchState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        let mut state = restored;
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        assert_eq!(notifier.sent.len(), 1);
    }

    #[test]
    fn check_only_never_sends_and_never_consumes_the_report() {
        let mut options = options();
        options.check_only = true;
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(report["episodes"][0]["reported"], false, "{report}");
        assert_eq!(report["episodes"][0]["due"], true, "{report}");
        assert_eq!(report["ok"], false, "{report}");
        assert_eq!(report["notice"]["reason"], "check only", "{report}");
        assert!(notifier.sent.is_empty());
        assert!(state.pending.is_empty(), "a dry run queues nothing");
        // The same episode is still owed its one report by a real run.
        options.check_only = false;
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        assert_eq!(notifier.sent.len(), 1);
    }

    #[test]
    fn an_unreadable_screen_holds_the_episode_clock() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", None)],
            vec![observation("worker", "idle", None)],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let held = pass(&mut probe, &mut notifier, &mut state, 120.0, &options);
        assert_eq!(held["episodes"][0]["kind"], "approval-prompt", "{held}");
        pass(&mut probe, &mut notifier, &mut state, 240.0, &options);
        assert!(notifier.sent.is_empty());
        // The clock ran from the first sighting, through the unreadable passes.
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(
            notifier.sent.len(),
            1,
            "transient read failures did not reset the clock"
        );
    }

    #[test]
    fn a_recovered_agent_withdraws_its_unsent_notice() {
        let mut options = options();
        options.min_notice_interval = Duration::from_secs(3600);
        // Seed a sent notice so the rate limit holds the next one back.
        let mut probe = ScriptedProbe(vec![
            vec![observation("first", "idle", Some(PROMPT_SCREEN))],
            vec![observation("first", "idle", Some(PROMPT_SCREEN))],
            vec![observation("second", "idle", Some(PROMPT_SCREEN))],
            vec![observation("second", "idle", Some(PROMPT_SCREEN))],
            vec![observation(
                "second",
                "working",
                Some("◆ recovered and working\n"),
            )],
            vec![observation("second", "working", Some("◆ still working\n"))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        let held = pass(&mut probe, &mut notifier, &mut state, 660.0, &options);
        assert_eq!(held["notice"]["reason"], "rate limited", "{held}");
        assert_eq!(state.pending.len(), 1);
        pass(&mut probe, &mut notifier, &mut state, 720.0, &options);
        assert!(
            state.pending.is_empty(),
            "recovery withdrew the owed notice"
        );
        pass(&mut probe, &mut notifier, &mut state, 7200.0, &options);
        assert_eq!(
            notifier.sent.len(),
            1,
            "the recovered agent was never reported"
        );
    }

    #[test]
    fn an_absent_pass_neither_drops_nor_restarts_an_episode() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 60.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.sent.len(), 1, "the clock survived the absent pass");
    }

    #[test]
    fn a_crash_before_the_state_write_rederives_the_same_batch_id() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
            vec![observation("worker", "idle", Some(PROMPT_SCREEN))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        // The watcher queues the batch, then dies before persisting the pass:
        // the restored state is the snapshot from before the due pass.
        let snapshot: WatchState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.attempts.len(), 1);
        let mut state = snapshot;
        pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.attempts.len(), 2);
        assert_eq!(
            notifier.attempts[0], notifier.attempts[1],
            "the same batch ID lets the coordinator's queue dedupe the retry"
        );
    }

    #[test]
    fn requeue_attempt_ids_are_a_stable_chain() {
        assert_eq!(attempt_message_id("agent-watch-x", 0), "agent-watch-x");
        assert_eq!(attempt_message_id("agent-watch-x", 1), "agent-watch-x-r1");
        assert_eq!(attempt_message_id("agent-watch-x", 3), "agent-watch-x-r3");
    }
}
