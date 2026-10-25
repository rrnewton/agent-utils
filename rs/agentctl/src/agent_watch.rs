//! Tell the coordinator when a registered agent needs a human (`agentctl watch-agents`).
//!
//! `agentctl watch` keeps the coordinator's environment alive; this watch keeps an eye on
//! the agents themselves. Every pass it lists the registry, reads each running agent's
//! visible screen, and classifies the agent as at a harness approval or permission prompt,
//! finished with a verdict, idle without one, stalled, dead, or healthy. A condition that
//! holds past its threshold becomes one notice to the coordinator, sent through the same
//! durable queue as `agentctl send`, so a busy coordinator loses nothing: the notice waits in
//! its queue. A finished child is reported at once, in a message of its own, so the parent can
//! give it its next task without waiting for its own sweep.
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
/// Bottom screen lines scanned for a completion marker on a screen that draws no reply bullets.
const MARKER_WINDOW_LINES: usize = 30;
/// Characters kept of a proving line quoted in a notice.
const QUOTED_LINE_CHARS: usize = 160;
/// Seconds a reported finish of an agent no longer listed is remembered.
const FINISHED_MEMORY_SECONDS: f64 = 3600.0;
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
    /// How long an agent may sit idle with a verdict, its screen unchanged, before it is
    /// reported finished; zero reports it in the pass that first sees it.
    pub child_done: Duration,
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
    /// The registry's landing queue (`agentctl hold`), read when a child finishes so its
    /// notice can say who lands next; `None` leaves notices without that line.
    pub landing_queue: Option<PathBuf>,
}

impl WatchAgentsOptions {
    fn threshold(&self, kind: EpisodeKind) -> Duration {
        match kind {
            EpisodeKind::ApprovalPrompt => self.approval,
            EpisodeKind::IdleNoVerdict => self.idle,
            EpisodeKind::ChildDone => self.child_done,
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
    /// The agent went idle or done with a verdict marker in its last reply.
    ChildDone,
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
            Self::ChildDone => "child-done",
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
                "working, or idle while its screen shows work running, with no new output for over {} minutes",
                minutes(options.stall)
            ),
            Self::Dead => format!(
                "unreachable for over {} minutes while its record says running",
                minutes(options.dead)
            ),
            Self::ChildDone => "finished its turn with a verdict".to_owned(),
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
            .then(|| line.trim().chars().take(QUOTED_LINE_CHARS).collect())
    })
}

/// Bullets a harness draws at column 0 to open an assistant block: Claude Code's `●` (`⏺`
/// in older builds), Muse's `◆`, and Codex's `•`.
const REPLY_BULLETS: [char; 4] = ['●', '⏺', '◆', '•'];

/// `text` without the reply bullet and spacing that open it, when it has one.
fn strip_bullet(text: &str) -> &str {
    text.strip_prefix(REPLY_BULLETS)
        .map_or(text, |rest| rest.trim_start_matches([' ', '\u{a0}']))
}

/// Whether `line` opens an assistant block.
fn is_reply_row(line: &str) -> bool {
    line.starts_with(REPLY_BULLETS)
}

/// Whether `line` echoes a user turn in the transcript: Claude Code and Muse draw `❯ text`,
/// Codex `› text`, and older Claude Code builds `> text`. An empty composer is not one.
fn is_user_turn(line: &str) -> bool {
    ['❯', '›', '>'].iter().any(|glyph| {
        line.strip_prefix(*glyph).is_some_and(|rest| {
            rest.starts_with([' ', '\u{a0}']) && !rest.trim_matches([' ', '\u{a0}']).is_empty()
        })
    })
}

/// Whether `line` opens or continues a tool call's output rather than prose: Claude Code
/// draws `⎿` under a tool call and Muse `└`; the rows below are that output until the next
/// block opens.
fn is_tool_output(line: &str) -> bool {
    line.trim_start().starts_with(['⎿', '└'])
}

/// The verdict line of the agent's last reply, trimmed and bounded, when there is one.
///
/// The last reply runs from the first assistant block after the last user turn above the
/// newest block (so the composer under it, even with a draft or Codex's placeholder, is not
/// taken for a turn) to the bottom of the screen. A verdict is a prose line of that reply that
/// starts with a marker once its indentation and any reply bullet are stripped: a reply that
/// opens with `DONE:` reads `◆ DONE: ...` in Muse and `● DONE: ...` in Claude Code, and a long
/// reply puts that line far above the bottom. Tool output (under `⎿` or `└`) is not prose, so a
/// brief or a log the agent printed cannot finish its turn. The verdict is the first marker
/// line of the last block that has one, since a reply states its verdict first. With no user
/// turn in view the whole screen from the first block on is the last reply. An older turn's
/// verdict above a newer prompt does not count, and nor does a prompt's own text. A screen that
/// draws no reply bullet is read the old way: a marker starting a line near the bottom.
fn verdict_line(screen: &str, markers: &[String]) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let marked = |line: &str| {
        let text = strip_bullet(line.trim_start());
        markers
            .iter()
            .any(|marker| text.starts_with(marker.as_str()))
            .then(|| {
                text.trim_end()
                    .chars()
                    .take(QUOTED_LINE_CHARS)
                    .collect::<String>()
            })
    };
    let Some(newest) = lines.iter().rposition(|line| is_reply_row(line)) else {
        let start = lines.len().saturating_sub(MARKER_WINDOW_LINES);
        return lines[start..].iter().rev().find_map(|line| marked(line));
    };
    let turn = lines[..newest]
        .iter()
        .rposition(|line| is_user_turn(line))
        .map_or(0, |index| index + 1);
    let start = turn + lines[turn..].iter().position(|line| is_reply_row(line))?;
    let mut verdict = None;
    let mut block_verdict = None;
    let mut in_tool = false;
    for line in &lines[start..] {
        if is_reply_row(line) {
            verdict = block_verdict.take().or(verdict);
            in_tool = false;
        } else if is_tool_output(line) {
            in_tool = true;
        }
        if !in_tool && block_verdict.is_none() {
            block_verdict = marked(line);
        }
    }
    block_verdict.or(verdict)
}

/// Bottom screen lines scanned for background work that is still running.
const BACKGROUND_WINDOW_LINES: usize = 12;

/// The part of a status row near the bottom of `screen` that says work is still running, when
/// one shows: a running turn, or background work the agent started. Claude Code counts its background
/// shells and sub-agents in the footer under its input box, as in
/// `⏵⏵ auto mode on · 2 shells · ← 1 agent · ↓ to manage`, and in the row that closes a
/// turn, as in `✻ Sautéed for 5m 51s · done 12:44 AM · 2 shells still running`. The turn's
/// row is history: it still says `still running` after the work ends. So when the input box
/// (a `❯` row) is in view, only the footer rows under it are read, and the turn's row only
/// when no input box is. An agent that went idle waiting on such work, such as a validation
/// gate it runs in a background shell, has not stopped without a verdict; only one whose
/// screen then stops changing for the stall threshold is reported, as stalled.
fn background_work(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let mut start = lines.len().saturating_sub(BACKGROUND_WINDOW_LINES);
    if let Some(input) = lines[start..]
        .iter()
        .rposition(|line| line.trim_start().starts_with('❯'))
    {
        start += input + 1;
    }
    lines[start..].iter().rev().find_map(|line| {
        if !line.contains('·') {
            return None;
        }
        line.split('·').find_map(|segment| {
            let segment = segment.trim().trim_start_matches('←').trim_start();
            background_count(segment).then(|| segment.to_owned())
        })
    })
}

/// Whether a footer segment shows work running: Claude Code's `esc to interrupt`, which it
/// draws while a turn runs, or a count of background shells or agents: `1 shell`, `2 shells`,
/// `1 agent`, `3 agents`, each optionally followed by `still running`. Herdr's status can say
/// `idle` while the footer says `esc to interrupt` and output streams (live finding
/// 2026-10-20), so the screen wins.
fn background_count(segment: &str) -> bool {
    if segment == "esc to interrupt" {
        return true;
    }
    let segment = segment.strip_suffix(" still running").unwrap_or(segment);
    let Some((count, noun)) = segment.split_once(' ') else {
        return false;
    };
    !count.is_empty()
        && count.bytes().all(|byte| byte.is_ascii_digit())
        && count.parse::<u64>().is_ok_and(|count| count > 0)
        && matches!(noun, "shell" | "shells" | "agent" | "agents")
}

/// Whether a row near the bottom of `screen` says a turn is running: `esc to interrupt` as a
/// `·`-separated or parenthesized segment, as in Claude Code's footer
/// (`⏵⏵ auto mode on · 2 shells · esc to interrupt`) and its spinner row, or Muse's running row
/// above its composer (`◇ Calling tools (30s · esc to interrupt)`). Prose that merely mentions
/// the phrase is not a segment.
fn turn_running(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(BACKGROUND_WINDOW_LINES);
    lines[start..].iter().any(|line| {
        line.split(['·', '(', ')'])
            .any(|segment| segment.trim() == "esc to interrupt")
    })
}

/// How many new messages Claude Code says wait below its view, when the view is scrolled up.
/// Scrolled up, Claude Code freezes the transcript where the reader left it and ends the last
/// row above its input box with `16 new messages (ctrl+End) ↓`, raising the count as messages
/// arrive below (live panes, 2026-10-20). Its pane keeps no terminal scrollback, so nothing
/// below the view can be read: not the running turn's timer, not the newest output, not a
/// closing verdict. A growing count changes the screen, which is progress like any change; a
/// count that holds still proves nothing, since one long tool call or sub-agent run is a
/// single message. The pill is read only at the end of that one row, between the input box's
/// rule and any blank rows, so a transcript that quotes it is not a scrolled view.
fn scrolled_below(screen: &str) -> Option<u64> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(BACKGROUND_WINDOW_LINES);
    let input = start
        + lines[start..]
            .iter()
            .rposition(|line| line.trim_start().starts_with('❯'))?;
    let row = lines[..input].iter().rev().find(|line| {
        let line = line.trim();
        !line.is_empty() && !line.chars().all(|character| character == '─')
    })?;
    let row = row.trim_end().strip_suffix('↓')?.trim_end();
    let row = match row.strip_suffix(')') {
        Some(hinted) => hinted.rsplit_once('(')?.0.trim_end(),
        None => row,
    };
    let row = row
        .strip_suffix(" new messages")
        .or_else(|| row.strip_suffix(" new message"))?;
    // The pill can be drawn flush against the transcript row it covers, so the count is the
    // row's trailing digits, not its last word.
    let count = &row[row.len() - row.bytes().rev().take_while(u8::is_ascii_digit).count()..];
    count.parse::<u64>().ok().filter(|count| *count > 0)
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
    /// For `child-done`, the hash of the verdict line: a different verdict is a new turn,
    /// and so a new episode, even when no working pass was seen between the two.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    turn: Option<u64>,
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
    /// For `child-done`, the landing-queue holders this finish releases, as `NAME (NOTE)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    next: Vec<String>,
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
        let screen = observation.screen.as_deref().unwrap_or_default();
        let running = turn_running(screen);
        let work =
            background_work(screen).or_else(|| running.then(|| "esc to interrupt".to_owned()));
        // A running turn means a verdict on screen is not this turn's last word yet, so only
        // the stall rule applies. Background shells or agents left running do not: a child
        // that said it is done is done.
        if !running {
            if let Some(verdict) = verdict_line(screen, &options.markers) {
                return Some((EpisodeKind::ChildDone, verdict));
            }
        }
        // A running turn or its own background work is not idle, whatever Herdr's
        // status says: the stall rule still reports it if the screen stops changing.
        if let Some(work) = work {
            return Some((
                EpisodeKind::Stalled,
                format!(
                    "status {} while the screen shows work running ({work}); screen unchanged",
                    observation.status
                ),
            ));
        }
        return Some((
            EpisodeKind::IdleNoVerdict,
            format!("status {} with no verdict marker", observation.status),
        ));
    }
    if observation.status == "working" && observation.screen.is_some() {
        return Some((EpisodeKind::Stalled, "screen unchanged".to_owned()));
    }
    None
}

/// Words of a verdict line that identify its turn. Only the leading words count, so a pane
/// resized between two passes, which rewraps the line, is still the same turn.
const TURN_WORDS: usize = 8;

/// The turn identity of a verdict line: a hash of its first [`TURN_WORDS`] words.
fn verdict_turn(verdict: &str) -> u64 {
    stable_hash(
        &verdict
            .split_whitespace()
            .take(TURN_WORDS)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

/// Withdraw the unsent notice owed for `episode`, if any is still waiting. An agent has one
/// episode at a time, so any notice of its kind for the agent is that episode's, even one
/// queued before its clock moved with a screen change.
fn withdraw_pending(state: &mut WatchState, agent: &str, episode: &Episode) {
    state
        .pending
        .retain(|notice| notice.agent != agent || notice.kind != episode.kind);
}

/// The landing-queue holders still waiting on `agent`, as `NAME (NOTE)`, read when its finish
/// becomes a notice. A `BLOCKED` verdict names nobody. Nothing is released here: the holds are
/// marked released only once the notice naming them is queued for the coordinator. A queue
/// that cannot be read costs only this line: the finish is still reported.
fn waiting_holders(options: &WatchAgentsOptions, agent: &str, verdict: &str) -> Vec<String> {
    let Some(path) = options.landing_queue.as_deref() else {
        return Vec::new();
    };
    if verdict.starts_with("BLOCKED") {
        return Vec::new();
    }
    match crate::landing_queue::waiting_on(path, agent) {
        Ok(holds) => holds
            .iter()
            .map(|hold| {
                if hold.note.is_empty() {
                    hold.name.clone()
                } else {
                    format!("{} ({})", hold.name, hold.note)
                }
            })
            .collect(),
        Err(error) => {
            eprintln!("agentctl watch-agents: landing queue: {error}");
            Vec::new()
        }
    }
}

/// The report row for an episode this pass could not judge, left exactly as it was.
fn held_row(
    observation: &Observation,
    episode: &Episode,
    now: f64,
    options: &WatchAgentsOptions,
    detail: &str,
) -> Value {
    json!({
        "agent": observation.name,
        "kind": episode.kind.as_str(),
        "status": observation.status,
        "since": episode.since,
        "age_seconds": now - episode.since,
        "reported": episode.notified,
        "due": now - episode.since >= options.threshold(episode.kind).as_secs_f64(),
        "detail": detail,
    })
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
            held_row(
                observation,
                episode,
                now,
                options,
                "screen unreadable this pass; episode held",
            )
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
    // A view scrolled up hides whatever would show progress or a verdict, so neither a stall
    // nor idleness nor a finish can be judged from it, however long it holds still. No clock
    // runs while it shows: an episode of the condition it seems to show is kept only once
    // reported, so it is not reported twice, and any other episode ends here with its unsent
    // notice. Scrolling back to the bottom changes the screen, so a stall, idle, or finish
    // clock starts again from there.
    // A growing count already changed the screen above, which ended any stall. A prompt is
    // still read: it is drawn at the bottom.
    if let Some(below) = observation.screen.as_deref().and_then(scrolled_below) {
        if let Some((
            kind @ (EpisodeKind::Stalled | EpisodeKind::IdleNoVerdict | EpisodeKind::ChildDone),
            _,
        )) = found
        {
            if let Some(cleared) = memory
                .episode
                .take_if(|episode| !(episode.notified && episode.kind == kind))
            {
                withdraw_pending(state, &observation.name, &cleared);
            }
            let row = memory.episode.as_ref().map(|episode| {
                held_row(
                    observation,
                    episode,
                    now,
                    options,
                    &format!("view scrolled up with {below} new message(s) below it; episode held"),
                )
            });
            state.agents.insert(observation.name.clone(), memory);
            return row;
        }
    }
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
    // A finish with a different verdict line is a later turn: the earlier episode ends here,
    // with its unsent notice, and the new one is reported in its own right.
    let turn = (kind == EpisodeKind::ChildDone).then(|| verdict_turn(&detail));
    if let Some(cleared) = memory
        .episode
        .take_if(|episode| episode.kind == kind && episode.turn != turn)
    {
        withdraw_pending(state, &observation.name, &cleared);
    }
    // A stall is timed from the last screen change, not from the first working poll:
    // an agent that works for an hour with changing output, then stops changing,
    // has been stalled only since it stopped.
    // Idle is timed from the last screen change too: Herdr can report a Claude Code agent
    // idle while its output still streams, and new output is not idleness.
    let since = match kind {
        EpisodeKind::Stalled => memory.screen_changed_at,
        EpisodeKind::IdleNoVerdict | EpisodeKind::ChildDone => match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since.max(memory.screen_changed_at),
            _ => now,
        },
        _ => match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since,
            _ => now,
        },
    };
    let notified =
        matches!(&memory.episode, Some(episode) if episode.kind == kind && episode.notified);
    memory.episode = Some(Episode {
        kind,
        since,
        notified,
        turn,
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
            next: if turn.is_some() {
                waiting_holders(options, &observation.name, &detail)
            } else {
                Vec::new()
            },
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
    let batched = |notice: &PendingNotice| notice.kind != EpisodeKind::ChildDone;
    if state.inflight.is_none() {
        if !state.pending.iter().any(batched) {
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
        let keys: Vec<String> = state
            .pending
            .iter()
            .filter(|notice| batched(notice))
            .map(PendingNotice::key)
            .collect();
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

/// Send each finished child's notice as a message of its own, every pass: a finish is not
/// batched with other kinds, held by the rate limit, or counted against it. Each message ID is
/// derived from its notice, so a retry after a failure or a restart is recognised by the
/// coordinator's queue instead of delivered twice. Returns one report row per notice tried.
fn flush_child_done(
    state: &mut WatchState,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Vec<Value> {
    if options.check_only {
        return Vec::new();
    }
    let due: Vec<PendingNotice> = state
        .pending
        .iter()
        .filter(|notice| notice.kind == EpisodeKind::ChildDone)
        .cloned()
        .collect();
    let mut rows = Vec::new();
    for notice in due {
        let key = notice.key();
        let message_id = format!("agent-watch-done-{:016x}", stable_hash(&key));
        let mut text = format!(
            "[agentctl watch-agents] {} finished its turn:\n- {}: {} ({})",
            notice.agent,
            notice.agent,
            notice.kind.describe(options),
            notice.detail
        );
        if !notice.next.is_empty() {
            text.push_str(&format!(
                "\nLanding queue: {} waited on {} and is next.",
                notice.next.join(", "),
                notice.agent
            ));
        }
        text.push_str(&format!(
            "\nRead its reply with `agentctl read {}`, then give it its next task or stop it.",
            notice.agent
        ));
        let target = notice_target(options, &notice.agent);
        match notifier.notify(target, &text, &message_id) {
            Ok(outcome) => {
                state.pending.retain(|pending| pending.key() != key);
                // The holders this notice named have now been told about; a later finish of
                // the same agent must not name them again.
                if let (false, Some(path)) =
                    (notice.next.is_empty(), options.landing_queue.as_deref())
                {
                    if let Err(error) =
                        crate::landing_queue::release_after(path, &notice.agent, &key, now)
                    {
                        eprintln!("agentctl watch-agents: landing queue: {error}");
                    }
                }
                rows.push(json!({"agent": notice.agent, "sent": true, "message_id": message_id, "outcome": outcome, "next": notice.next}));
            }
            Err(error) => {
                rows.push(json!({"agent": notice.agent, "sent": false, "message_id": message_id, "reason": error.to_string()}));
            }
        }
    }
    rows
}

/// Who is told about `agent`. Every notice goes to the configured coordinator for now; this is
/// the one place a manager tree would route a child's notice to its nearest manager instead.
fn notice_target<'a>(options: &'a WatchAgentsOptions, _agent: &str) -> &'a str {
    &options.coordinator
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
        // A reported finish of an agent that is gone (stopped, most often) is dropped an hour
        // after it was reported, so every child stopped after finishing does not stay in the
        // state for ever; within that hour an absent pass still cannot repeat its notice.
        state.agents.retain(|name, memory| {
            seen.contains(name)
                || state.pending.iter().any(|notice| &notice.agent == name)
                || memory.episode.as_ref().is_some_and(|episode| {
                    !(episode.kind == EpisodeKind::ChildDone
                        && episode.notified
                        && now - episode.since > FINISHED_MEMORY_SECONDS)
                })
        });
    }
    let child_done = flush_child_done(state, notifier, now, options);
    let notice = flush_notices(state, notifier, now, options);
    // Agents whose view is scrolled up cannot be judged stalled or idle; say which, and do
    // not call the pass ok, so a quiet report is not read as a clean bill.
    let scrolled: Vec<Value> = observations
        .iter()
        .filter_map(|row| {
            let below = row.screen.as_deref().and_then(scrolled_below)?;
            Some(json!({"agent": row.name, "new_messages": below}))
        })
        .collect();
    json!({
        "schema": STATE_SCHEMA,
        "checked_at": now,
        "coordinator": options.coordinator,
        "ok": probe_error.is_null()
            && scrolled.is_empty()
            && episodes.iter().all(|row| {
                // A finished child is news for its coordinator, not a fault in the team.
                row["kind"] == EpisodeKind::ChildDone.as_str()
                    || (row["reported"] != true && row["due"] != true)
            }),
        "probe_error": probe_error,
        "agents": seen.len(),
        "episodes": episodes,
        "scrolled": scrolled,
        "notice": notice,
        "child_done": child_done,
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
            child_done: Duration::ZERO,
            stall: Duration::from_secs(1800),
            dead: Duration::from_secs(300),
            markers: vec!["DONE:".to_owned(), "BLOCKED:".to_owned()],
            min_notice_interval: Duration::ZERO,
            state_dir: None,
            landing_queue: None,
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

        // A verdict in the last reply: reported once as finished, never as idle.
        let mut probe = ScriptedProbe(vec![
            vec![observation("done-agent", "idle", Some(DONE_SCREEN))],
            vec![observation("done-agent", "idle", Some(DONE_SCREEN))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 7200.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "child-done", "{report}");
        assert_eq!(notifier.sent.len(), 1, "{:?}", notifier.sent);
        notifier.sent.clear();

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

    // Rows drawn by Claude Code at the bottom of an idle pane while two background shells
    // run, read from a live pane on 2026-10-20: the row that closes the turn, the empty
    // input box, and the footer.
    const BACKGROUND_TURN_END: &str = "● Waiting for the validation gate in the background.\n\n✻ Sautéed for 5m 51s · done 12:44 AM · 2 shells still running\n\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · 2 shells · ← 1 agent · ↓ to manage\n";
    // The same pane once the turn row has scrolled away: only the footer counts the work.
    const BACKGROUND_FOOTER_ONLY: &str = "● Waiting for the validation gate in the background.\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · 1 shell · ↓ to manage\n";
    const BACKGROUND_AGENT_ONLY: &str = "● A reviewer sub-agent is checking the change.\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · ← 1 agent · ↓ to manage\n";
    // Claude Code's footer while a turn runs, read from a live pane on 2026-10-20 that Herdr
    // reported idle.
    const WORKING_FOOTER: &str = "● Reading the code.\n\n✽ Orbiting… (5m 38s · ↓ 20.5k tokens)\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · esc to interrupt · ↓ to manage\n";
    // Claude Code's footer with no background work.
    const QUIET_FOOTER: &str = "● Finished the change.\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on\n";

    #[test]
    fn background_work_is_read_from_claude_code_rows() {
        assert_eq!(
            background_work(BACKGROUND_TURN_END).as_deref(),
            Some("2 shells")
        );
        assert_eq!(
            background_work(BACKGROUND_FOOTER_ONLY).as_deref(),
            Some("1 shell")
        );
        assert_eq!(
            background_work(BACKGROUND_AGENT_ONLY).as_deref(),
            Some("1 agent")
        );
        assert_eq!(
            background_work("✻ Baked for 2m · done 1:00 AM · 3 shells still running\n").as_deref(),
            Some("3 shells still running")
        );
        assert_eq!(background_work(QUIET_FOOTER), None);
        assert_eq!(
            background_work(WORKING_FOOTER).as_deref(),
            Some("esc to interrupt")
        );
        // The turn's row is history: once the footer no longer counts the work, it is over.
        let ended = BACKGROUND_TURN_END.replace(" · 2 shells · ← 1 agent", "");
        assert_eq!(background_work(&ended), None, "{ended}");
        assert_eq!(background_work(IDLE_SCREEN), None);
        // Prose that mentions shells is not a count, and zero is no work.
        assert_eq!(
            background_work("● I opened 2 shells · then closed them\n"),
            None
        );
        assert_eq!(background_work("  ⏵⏵ auto mode on · 0 shells\n"), None);
        // A count far above the bottom of the screen is old.
        let old = format!("{BACKGROUND_FOOTER_ONLY}{}", "● more text\n".repeat(20));
        assert_eq!(background_work(&old), None);
    }

    #[test]
    fn idle_with_background_work_is_held_to_the_stall_rule() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let mut probe = ScriptedProbe(vec![
            vec![observation("gating", "idle", Some(BACKGROUND_TURN_END))],
            vec![observation("gating", "idle", Some(BACKGROUND_TURN_END))],
            vec![observation("gating", "idle", Some(BACKGROUND_FOOTER_ONLY))],
            vec![observation("gating", "idle", Some(BACKGROUND_FOOTER_ONLY))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        // Past the idle threshold: not idle-no-verdict, and not yet a stall.
        let held = pass(&mut probe, &mut notifier, &mut state, 1000.0, &options);
        assert_eq!(held["episodes"][0]["kind"], "stalled", "{held}");
        assert_eq!(held["episodes"][0]["due"], false, "{held}");
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        // The screen changed at 1100, so the stall is timed from there.
        pass(&mut probe, &mut notifier, &mut state, 1100.0, &options);
        let early = pass(&mut probe, &mut notifier, &mut state, 2800.0, &options);
        assert_eq!(early["episodes"][0]["due"], false, "{early}");
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        let mut probe = ScriptedProbe(vec![vec![observation(
            "gating",
            "idle",
            Some(BACKGROUND_FOOTER_ONLY),
        )]]);
        let due = pass(&mut probe, &mut notifier, &mut state, 2900.0, &options);
        assert_eq!(due["episodes"][0]["kind"], "stalled", "{due}");
        assert_eq!(notifier.sent.len(), 1);
        assert!(
            notifier.sent[0].1.contains("work running (1 shell)"),
            "{}",
            notifier.sent[0].1
        );

        // Once the background work ends, the idle rule applies again from that pass.
        let mut probe = ScriptedProbe(vec![
            vec![observation("gating", "idle", Some(QUIET_FOOTER))],
            vec![observation("gating", "idle", Some(QUIET_FOOTER))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 3000.0, &options);
        let idle = pass(&mut probe, &mut notifier, &mut state, 3900.0, &options);
        assert_eq!(idle["episodes"][0]["kind"], "idle-no-verdict", "{idle}");
        assert_eq!(notifier.sent.len(), 2);
    }

    #[test]
    fn idle_is_timed_from_the_last_screen_change() {
        // Herdr said idle while the agent's output kept changing: no idle report until the
        // screen has held still for the idle threshold.
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let screens = ["● step one\n❯\n", "● step two\n❯\n", "● step three\n❯\n"];
        for (index, screen) in screens.iter().enumerate() {
            let mut probe = ScriptedProbe(vec![vec![observation("busy", "idle", Some(screen))]]);
            let at = 600.0 * index as f64;
            let report = pass(&mut probe, &mut notifier, &mut state, at, &options);
            assert_eq!(report["episodes"][0]["kind"], "idle-no-verdict", "{report}");
            assert_eq!(report["episodes"][0]["since"], at, "{report}");
        }
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        let mut probe = ScriptedProbe(vec![vec![observation("busy", "idle", Some(screens[2]))]]);
        pass(
            &mut probe,
            &mut notifier,
            &mut state,
            1200.0 + 900.0,
            &options,
        );
        assert_eq!(notifier.sent.len(), 1);

        // A footer that says a turn runs holds an idle-status agent to the stall rule.
        let mut state = WatchState::default();
        let mut probe = ScriptedProbe(vec![
            vec![observation("streaming", "idle", Some(WORKING_FOOTER))],
            vec![observation("streaming", "idle", Some(WORKING_FOOTER))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let held = pass(&mut probe, &mut notifier, &mut state, 1000.0, &options);
        assert_eq!(held["episodes"][0]["kind"], "stalled", "{held}");
        assert_eq!(notifier.sent.len(), 1);
    }

    // Claude Code scrolled up while it works, from a live pane on 2026-10-20: the frozen
    // transcript, the divider where the new messages begin, the pill at the end of the last row
    // above the input box, the input box between its rules, and the footer.
    const SCROLLED_WORKING: &str = "● Bash(python3 - \"$P\" <<'EOF'\n      import sys…)\n  ⎿  # DBG3 two channels read\n     … +2 lines\n\n──────────────── 16 new messages ────────────────\n\n● Bash(python3 - \"$P\" <<'EOF'\n      import sys,re…)                    16 new messages (ctrl+End) ↓\n\n────────────────────────────────────────\n❯\u{a0}\n────────────────────────────────────────\n  ⏵⏵ auto mode on (shift+tab to cycle) · ← 1 agent\n";
    // A pane of the shape the watcher reported stalled on 2026-10-20: a live capture's turn
    // row with its pill drawn over it, and a footer counting the validation run in a
    // background shell.
    const SCROLLED_GATING: &str = "● The validation gate is running on the rebased HEAD. I'll push only if it passes.\n\n✻ Sautéed for 23m 29s · done 10:02 PM                         7 new messages (ctrl+End) ↓\n\n────────────────────────────────────────\n❯\u{a0}\n────────────────────────────────────────\n  ⏵⏵ auto mode on · 1 shell · ↓ to manage\n";
    // The same kind of pane at the bottom of its view, from a live pane on 2026-10-20: the
    // running turn's timer is the last row above the input box.
    const LIVE_WORKING: &str = "● Foreground long-running command with streaming output · 44s\n  ⎿  $ for i in $(seq 1 65); do echo \"tick $i\"; sleep 1; done (45s · 46 lines)\n     (ctrl+b to run in background)\n\n✢ Processing… (4m 19s · ↓ 13.1k tokens)\n\n────────────────────────────────────────\n❯\n────────────────────────────────────────\n  ⏵⏵ auto mode on · 1 shell · esc to interrupt · ← 1 agent · ↓ to manage\n";
    const RULED_IDLE: &str = "● Finished the change.\n\n────────────────────────────────────────\n❯\u{a0}\n────────────────────────────────────────\n  ⏵⏵ auto mode on\n";

    /// [`SCROLLED_WORKING`] with nothing running in its footer, as an idle agent's would be.
    fn scrolled_quiet() -> String {
        SCROLLED_WORKING.replace(" · ← 1 agent", "")
    }

    #[test]
    fn a_scrolled_view_is_read_from_the_row_above_the_input_box() {
        assert_eq!(scrolled_below(SCROLLED_WORKING), Some(16));
        assert_eq!(scrolled_below(SCROLLED_GATING), Some(7));
        assert_eq!(
            scrolled_below(&SCROLLED_GATING.replace("7 new messages", "1 new message")),
            Some(1)
        );
        // The footer under the input box is still live while the view is scrolled.
        assert_eq!(background_work(SCROLLED_GATING).as_deref(), Some("1 shell"));
        for screen in [
            LIVE_WORKING,
            RULED_IDLE,
            IDLE_SCREEN,
            DONE_SCREEN,
            QUIET_FOOTER,
            WORKING_FOOTER,
            BACKGROUND_TURN_END,
            BACKGROUND_FOOTER_ONLY,
        ] {
            assert_eq!(scrolled_below(screen), None, "{screen}");
        }
        // The divider alone, which marks where new messages begin, is not the pill.
        let divider_only =
            SCROLLED_WORKING.replace("                    16 new messages (ctrl+End) ↓", "");
        assert_eq!(scrolled_below(&divider_only), None, "{divider_only}");
        // A transcript that quotes the pill anywhere but at the end of that one row is not a
        // scrolled view.
        let quoted = RULED_IDLE.replace(
            "● Finished the change.",
            "● Scrolled up it reads 7 new messages (ctrl+End) ↓\n● Finished the change.",
        );
        assert_eq!(scrolled_below(&quoted), None, "{quoted}");
        let inline = RULED_IDLE.replace(
            "● Finished the change.",
            "● It reads `7 new messages (ctrl+End) ↓` there.",
        );
        assert_eq!(scrolled_below(&inline), None, "{inline}");
        // No input box in view, and a zero count, are not a scrolled view either.
        assert_eq!(
            scrolled_below("● x    16 new messages (ctrl+End) ↓\n"),
            None
        );
        // A pill drawn flush against the row it covers still gives its count.
        assert_eq!(
            scrolled_below(&SCROLLED_WORKING.replace("…)                    16 new", "…)16 new")),
            Some(16)
        );
        assert_eq!(
            scrolled_below(&SCROLLED_GATING.replace("7 new", "0 new")),
            None
        );
    }

    #[test]
    fn a_scrolled_view_is_never_reported_stalled_or_idle() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        // Working, idle with a background shell (the 2026-10-20 report), and idle with no
        // verdict in view, each frozen for two hours: the verdict, the turn's timer, and the
        // newest output are all below the view.
        let screens = [
            observation("working", "working", Some(SCROLLED_WORKING)),
            observation("gating", "idle", Some(SCROLLED_GATING)),
            observation("quiet", "idle", Some(&scrolled_quiet())),
        ];
        for at in [0.0, 1800.0, 3600.0, 7200.0] {
            let mut probe = ScriptedProbe(vec![screens.to_vec()]);
            let report = pass(&mut probe, &mut notifier, &mut state, at, &options);
            assert_eq!(report["episodes"], json!([]), "{report}");
            assert_eq!(report["ok"], false, "{report}");
            assert_eq!(report["scrolled"][1]["agent"], "gating", "{report}");
            assert_eq!(report["scrolled"][1]["new_messages"], 7, "{report}");
        }
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        assert!(state.pending.is_empty());

        // A prompt is drawn at the bottom, so it is still reported from a scrolled pane.
        let prompted = SCROLLED_GATING.replace(
            "  ⏵⏵ auto mode on · 1 shell · ↓ to manage\n",
            "Do you want to proceed?\n❯ 1. Yes\n",
        );
        let mut probe = ScriptedProbe(vec![
            vec![observation("asking", "idle", Some(&prompted))],
            vec![observation("asking", "idle", Some(&prompted))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(due["episodes"][0]["kind"], "approval-prompt", "{due}");
        assert_eq!(notifier.sent.len(), 1);
    }

    #[test]
    fn a_growing_new_message_count_is_progress() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let more =
            SCROLLED_WORKING.replace("16 new messages (ctrl+End)", "17 new messages (ctrl+End)");
        let mut probe = ScriptedProbe(vec![
            vec![observation("working", "working", Some(SCROLLED_WORKING))],
            vec![observation("working", "working", Some(SCROLLED_WORKING))],
            vec![observation("working", "working", Some(&more))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 600.0, &options);
        assert_eq!(state.agents["working"].screen_changed_at, 0.0);
        pass(&mut probe, &mut notifier, &mut state, 1200.0, &options);
        assert_eq!(state.agents["working"].screen_changed_at, 1200.0);
    }

    #[test]
    fn scrolling_back_down_starts_the_clocks_again() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        // Scrolled for two hours, then back at the bottom of a stalled screen: the stall is
        // timed from the return.
        for at in [0.0, 3600.0, 7200.0] {
            let mut probe = ScriptedProbe(vec![vec![observation(
                "working",
                "working",
                Some(SCROLLED_WORKING),
            )]]);
            pass(&mut probe, &mut notifier, &mut state, at, &options);
        }
        for at in [7260.0, 7260.0 + 1799.0] {
            let mut probe = ScriptedProbe(vec![vec![observation(
                "working",
                "working",
                Some(LIVE_WORKING),
            )]]);
            let report = pass(&mut probe, &mut notifier, &mut state, at, &options);
            assert_eq!(report["episodes"][0]["since"], 7260.0, "{report}");
        }
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        let mut probe = ScriptedProbe(vec![vec![observation(
            "working",
            "working",
            Some(LIVE_WORKING),
        )]]);
        let due = pass(
            &mut probe,
            &mut notifier,
            &mut state,
            7260.0 + 1800.0,
            &options,
        );
        assert_eq!(due["episodes"][0]["kind"], "stalled", "{due}");
        assert_eq!(notifier.sent.len(), 1);

        // Idle with no verdict, timed past its threshold before the view was scrolled: the
        // scroll drops the unreported episode, so the return starts a fresh idle clock.
        let mut state = WatchState::default();
        let mut notifier = RecordingNotifier::default();
        for (at, screen) in [
            (0.0, RULED_IDLE),
            (899.0, scrolled_quiet().as_str()),
            (960.0, RULED_IDLE),
            (960.0 + 899.0, RULED_IDLE),
        ] {
            let mut probe = ScriptedProbe(vec![vec![observation("quiet", "idle", Some(screen))]]);
            pass(&mut probe, &mut notifier, &mut state, at, &options);
        }
        assert!(notifier.sent.is_empty(), "{:?}", notifier.sent);
        let mut probe = ScriptedProbe(vec![vec![observation("quiet", "idle", Some(RULED_IDLE))]]);
        pass(
            &mut probe,
            &mut notifier,
            &mut state,
            960.0 + 900.0,
            &options,
        );
        assert_eq!(notifier.sent.len(), 1);
    }

    #[test]
    fn a_reported_episode_is_held_through_a_scroll_and_not_reported_twice() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        for at in [0.0, 900.0] {
            let mut probe =
                ScriptedProbe(vec![vec![observation("quiet", "idle", Some(RULED_IDLE))]]);
            pass(&mut probe, &mut notifier, &mut state, at, &options);
        }
        assert_eq!(notifier.sent.len(), 1);
        let mut probe = ScriptedProbe(vec![vec![observation(
            "quiet",
            "idle",
            Some(&scrolled_quiet()),
        )]]);
        let held = pass(&mut probe, &mut notifier, &mut state, 1000.0, &options);
        assert_eq!(held["episodes"][0]["kind"], "idle-no-verdict", "{held}");
        assert_eq!(held["episodes"][0]["reported"], true, "{held}");
        assert!(
            held["episodes"][0]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("16 new message(s)")),
            "{held}"
        );
        for at in [1100.0, 5000.0] {
            let mut probe =
                ScriptedProbe(vec![vec![observation("quiet", "idle", Some(RULED_IDLE))]]);
            pass(&mut probe, &mut notifier, &mut state, at, &options);
        }
        assert_eq!(notifier.sent.len(), 1, "{:?}", notifier.sent);
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
        assert_eq!(report["episodes"][0]["kind"], "child-done", "{report}");
        assert_eq!(notifier.sent.len(), 1, "{:?}", notifier.sent);
        assert!(
            !notifier.sent[0].1.contains("Would you like"),
            "{}",
            notifier.sent[0].1
        );
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

    const RULE: &str = "────────────────────────────────────────";

    /// A Muse pane after a long closing reply, shaped like the 2026-10-21 case the watcher
    /// misread: the reply opens with its verdict behind Muse's bullet, far above the bottom.
    fn muse_long_reply() -> String {
        let details: String = (1..=60)
            .map(|index| format!("  - detail line {index} of the closing report\n"))
            .collect();
        format!(
            "  Muse Code 1.4.14\n\n❯ Land both items and report.\n  DONE: is how the reply must start, or BLOCKED:.\n\n◆ Finished · Run the gate · 38m · ctrl+o\n└ validate: OK\n\n◆ DONE: Both items are on main.\n{details}\n◆ Worked for 41m 2s · 11:30 PM · 21/Oct\n\n{RULE}\n❯\n{RULE}\n  model · xhigh · ~/work/project\n"
        )
    }

    #[test]
    fn the_verdict_is_read_from_the_start_of_the_last_reply() {
        let markers = options().markers;
        let verdict = |screen: &str| verdict_line(screen, &markers);
        assert_eq!(
            verdict(&muse_long_reply()).as_deref(),
            Some("DONE: Both items are on main.")
        );
        let claude = format!(
            "❯ do it\n\n● Bash(make validate)\n  ⎿  validate: OK\n\n● DONE: landed abc123\n\n  More detail.\n\n✻ Baked for 5m · done 1:00 AM\n\n{RULE}\n❯\u{a0}\n{RULE}\n  ⏵⏵ auto mode on\n"
        );
        assert_eq!(verdict(&claude).as_deref(), Some("DONE: landed abc123"));
        let codex = "› fix the bug\n\n• Edited src/lib.rs (+3 -1)\n\n• BLOCKED: the schema needs the owner's call\n\n› Ask Codex to do anything\n\n  80% context left\n";
        assert_eq!(
            verdict(codex).as_deref(),
            Some("BLOCKED: the schema needs the owner's call")
        );
        // A verdict that closes the reply as its own paragraph still counts.
        assert_eq!(
            verdict(DONE_SCREEN).as_deref(),
            Some("DONE: the thing is complete")
        );
        // A screen with no reply bullets is read the old way, near the bottom.
        assert_eq!(
            verdict("plain output\nDONE: ok\n$ \n").as_deref(),
            Some("DONE: ok")
        );
        // An older turn's verdict above a newer prompt is not this turn's.
        let older = format!(
            "● DONE: the first task is finished\n\n❯ also do the second thing\n\n● Which branch should the second change go on?\n\n{RULE}\n❯\u{a0}\n{RULE}\n  ⏵⏵ auto mode on\n"
        );
        assert_eq!(verdict(&older), None);
        // Nor is the prompt's own text, even a line of it that starts with a marker.
        let brief = format!(
            "❯ You are a worker.\n  DONE:\n  1. write a handoff\n\n● Read the brief; starting.\n\n{RULE}\n❯\u{a0}\n{RULE}\n"
        );
        assert_eq!(verdict(&brief), None);
        // A draft in the composer is not a user turn that hides the reply above it.
        let drafted = claude.replace("❯\u{a0}\n", "❯ next: rebase\n");
        assert_eq!(verdict(&drafted).as_deref(), Some("DONE: landed abc123"));
    }

    #[test]
    fn a_long_muse_reply_that_opens_with_its_verdict_is_a_finish_not_silence() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let screen = muse_long_reply();
        for at in [0.0, 900.0, 1800.0] {
            let mut probe =
                ScriptedProbe(vec![vec![observation("mu-panettl", "idle", Some(&screen))]]);
            let report = pass(&mut probe, &mut notifier, &mut state, at, &options);
            assert_eq!(report["episodes"][0]["kind"], "child-done", "{report}");
        }
        assert_eq!(notifier.sent.len(), 1, "{:?}", notifier.sent);
        assert!(
            notifier.sent[0].1.contains("DONE: Both items are on main."),
            "{}",
            notifier.sent[0].1
        );
    }

    #[test]
    fn a_finished_child_is_reported_at_once_in_a_message_of_its_own() {
        let mut options = options();
        options.min_notice_interval = Duration::from_secs(600);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let asking = || observation("asker", "idle", Some(PROMPT_SCREEN));
        let finished = || observation("finisher", "idle", Some(DONE_SCREEN));
        let late = || observation("late-asker", "idle", Some(PROMPT_SCREEN));
        let mut probe = ScriptedProbe(vec![
            vec![asking()],
            vec![asking()],
            vec![asking(), finished(), late()],
            vec![asking(), finished(), late()],
            vec![asking(), finished(), late()],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let first = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(first["notice"]["sent"], true, "{first}");
        // Inside the rate-limit window opened at 300, the finish goes out in its first pass.
        let done = pass(&mut probe, &mut notifier, &mut state, 360.0, &options);
        assert_eq!(done["child_done"][0]["sent"], true, "{done}");
        assert_eq!(done["notice"]["reason"], "nothing to report", "{done}");
        assert_eq!(notifier.sent.len(), 2);
        let text = &notifier.sent[1].1;
        assert!(
            text.starts_with("[agentctl watch-agents] finisher finished its turn:\n- finisher: finished its turn with a verdict (DONE: the thing is complete)"),
            "{text}"
        );
        assert!(!text.contains("asker"), "{text}");
        assert_eq!(notifier.sent[1].0, "coordinator");
        assert!(notifier.sent[1].2.starts_with("agent-watch-done-"));
        // The finish did not restart the window: `late-asker`, due at 660, goes out at 900.
        let held = pass(&mut probe, &mut notifier, &mut state, 660.0, &options);
        assert_eq!(held["notice"]["reason"], "rate limited", "{held}");
        pass(&mut probe, &mut notifier, &mut state, 900.0, &options);
        assert_eq!(notifier.sent.len(), 3, "{:?}", notifier.sent);
        assert!(notifier.sent[2].1.contains("late-asker"));
        assert!(!notifier.sent[2].1.contains("finisher"));
    }

    #[test]
    fn a_finish_is_reported_once_per_turn() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let second = DONE_SCREEN.replace("the thing is complete", "the follow-up is complete");
        for (at, status, screen, sent) in [
            (0.0, "idle", DONE_SCREEN, 1),
            (60.0, "idle", DONE_SCREEN, 1),
            // A new verdict line is a new turn, even with no working pass seen between.
            (120.0, "idle", second.as_str(), 2),
            (180.0, "idle", second.as_str(), 2),
            (240.0, "working", "◆ working on the next task\n", 2),
            // The same line again after a working pass is a new turn too.
            (300.0, "idle", second.as_str(), 3),
        ] {
            let mut probe = ScriptedProbe(vec![vec![observation("worker", status, Some(screen))]]);
            pass(&mut probe, &mut notifier, &mut state, at, &options);
            assert_eq!(notifier.sent.len(), sent, "at {at}: {:?}", notifier.sent);
        }
    }

    #[test]
    fn a_finish_waits_out_a_running_turn_a_scrolled_view_and_a_failed_send() {
        let options = options();
        let mut notifier = RecordingNotifier {
            fail_attempts: 1,
            ..RecordingNotifier::default()
        };
        let mut state = WatchState::default();
        // Herdr says idle, but the footer says a turn runs: the stall rule, not a finish.
        let running = "● DONE: an earlier answer\n\n✽ Orbiting… (5m 38s)\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · esc to interrupt · ↓ to manage\n";
        // A view scrolled up shows a verdict that may be long superseded below it.
        let scrolled = scrolled_quiet().replace(
            "● Bash(python3 - \"$P\" <<'EOF'\n      import sys…)",
            "● DONE: an old verdict",
        );
        assert!(scrolled_below(&scrolled).is_some() && scrolled.contains("● DONE: an old verdict"));
        let mut probe = ScriptedProbe(vec![vec![
            observation("running", "idle", Some(running)),
            observation("scrolled", "idle", Some(&scrolled)),
            observation("finisher", "idle", Some(DONE_SCREEN)),
        ]]);
        let report = pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "stalled", "{report}");
        assert_eq!(report["episodes"][1]["kind"], "child-done", "{report}");
        assert_eq!(report["episodes"].as_array().unwrap().len(), 2, "{report}");
        assert_eq!(report["child_done"][0]["sent"], false, "{report}");
        assert!(notifier.sent.is_empty());
        assert_eq!(state.pending.len(), 1, "the failed finish is kept");
        let mut probe = ScriptedProbe(vec![vec![observation(
            "finisher",
            "idle",
            Some(DONE_SCREEN),
        )]]);
        pass(&mut probe, &mut notifier, &mut state, 60.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        assert_eq!(
            notifier.attempts[0], notifier.attempts[1],
            "the retry reuses the message ID"
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn a_finish_names_who_lands_next() {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-agent-watch-landing-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let queue = crate::landing_queue::path(&directory);
        crate::landing_queue::hold(&queue, "second", "first", "gate green", 0.0).unwrap();
        crate::landing_queue::hold(&queue, "third", "stuck", "", 0.0).unwrap();
        let mut options = options();
        options.landing_queue = Some(queue.clone());
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let blocked = "● BLOCKED: the schema needs a decision\n❯\n";
        let mut probe = ScriptedProbe(vec![vec![
            observation("first", "idle", Some(DONE_SCREEN)),
            observation("stuck", "idle", Some(blocked)),
        ]]);
        let report = pass(&mut probe, &mut notifier, &mut state, 10.0, &options);
        assert_eq!(
            report["child_done"][0]["next"],
            json!(["second (gate green)"]),
            "{report}"
        );
        assert_eq!(notifier.sent.len(), 2);
        assert!(
            notifier.sent[0]
                .1
                .contains("\nLanding queue: second (gate green) waited on first and is next.\n"),
            "{}",
            notifier.sent[0].1
        );
        assert!(
            !notifier.sent[1].1.contains("Landing queue"),
            "{}",
            notifier.sent[1].1
        );
        let holds = crate::landing_queue::load(&queue).unwrap().holds;
        assert!(holds[0]
            .released_by
            .as_deref()
            .is_some_and(|by| by.starts_with("first:")));
        assert_eq!(
            holds[1].released_by, None,
            "a BLOCKED finish releases nobody"
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn tool_output_is_not_a_verdict_and_a_reply_states_its_verdict_first() {
        let markers = options().markers;
        let verdict = |screen: &str| verdict_line(screen, &markers);
        // A brief or a log the agent printed is tool output, not its reply.
        let claude = format!(
            "❯ go\n\n● Bash(cat brief.md)\n  ⎿  Finish with a verdict:\n     DONE: what landed\n\n● Which branch should this go on?\n\n{RULE}\n❯\u{a0}\n{RULE}\n"
        );
        assert_eq!(verdict(&claude), None);
        let muse = format!(
            "❯ go\n\n◆ Finished · Run the script · 3s · ctrl+o\n└ step 1\n  DONE: 12 steps\n\n◆ Which branch should this go on?\n\n{RULE}\n❯\n{RULE}\n"
        );
        assert_eq!(verdict(&muse), None);
        // Within a block the first marker is the verdict; across blocks the last block's.
        let mixed = "● BLOCKED: the owner must choose\n\n  DONE: the tests written so far\n❯\n";
        assert_eq!(
            verdict(mixed).as_deref(),
            Some("BLOCKED: the owner must choose")
        );
        let later = "● BLOCKED: waiting on the gate\n\n● Bash(make validate)\n  ⎿  DONE: not this\n\n● DONE: landed abc123\n  BLOCKED: none\n❯\n";
        assert_eq!(verdict(later).as_deref(), Some("DONE: landed abc123"));
        // A rewrapped verdict line is the same turn; a different one is not.
        assert_eq!(
            verdict_turn(
                "DONE: #509 agentctl-child-done-notice landed as abc1234 with the gate green"
            ),
            verdict_turn("DONE: #509 agentctl-child-done-notice landed as abc1234 with the")
        );
        assert_ne!(verdict_turn("DONE: one"), verdict_turn("DONE: two"));
    }

    #[test]
    fn a_running_turn_anywhere_near_the_bottom_holds_back_a_finish() {
        let options = options();
        let running = [
            "● DONE: an earlier answer\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · 2 shells · esc to interrupt · ↓ to manage\n".to_owned(),
            format!("◆ DONE: an earlier answer\n\n◇ Calling tools (30s · esc to interrupt)\n\n{RULE}\n❯\n{RULE}\n  model · xhigh\n"),
        ];
        for screen in &running {
            assert!(turn_running(screen), "{screen}");
            let found = classify(&observation("worker", "idle", Some(screen)), &options);
            assert_eq!(
                found.map(|(kind, _)| kind),
                Some(EpisodeKind::Stalled),
                "{screen}"
            );
        }
        assert!(!turn_running(
            "● I will press esc to interrupt it later.\n❯\n"
        ));
        assert!(!turn_running(QUIET_FOOTER));
    }

    #[test]
    fn a_finish_keeps_the_pass_ok_and_is_forgotten_an_hour_after_its_agent_left() {
        let options = options();
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let mut probe = ScriptedProbe(vec![
            vec![observation("finisher", "idle", Some(DONE_SCREEN))],
            vec![],
            vec![],
        ]);
        let report = pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert_eq!(report["episodes"][0]["reported"], true, "{report}");
        assert_eq!(report["ok"], true, "{report}");
        pass(&mut probe, &mut notifier, &mut state, 600.0, &options);
        assert!(
            state.agents.contains_key("finisher"),
            "an absent pass keeps it"
        );
        pass(&mut probe, &mut notifier, &mut state, 3700.0, &options);
        assert!(!state.agents.contains_key("finisher"), "{:?}", state.agents);
        assert_eq!(notifier.sent.len(), 1);
    }

    #[test]
    fn a_hold_is_released_only_when_its_notice_is_queued() {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-agent-watch-release-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let queue = crate::landing_queue::path(&directory);
        crate::landing_queue::hold(&queue, "second", "first", "", 0.0).unwrap();
        let mut options = options();
        options.landing_queue = Some(queue.clone());
        let mut notifier = RecordingNotifier {
            fail_attempts: 1,
            ..RecordingNotifier::default()
        };
        let mut state = WatchState::default();
        let mut probe = ScriptedProbe(vec![
            vec![observation("first", "idle", Some(DONE_SCREEN))],
            vec![observation("first", "working", Some("◆ back at work\n"))],
        ]);
        pass(&mut probe, &mut notifier, &mut state, 10.0, &options);
        assert!(notifier.sent.is_empty());
        assert_eq!(
            crate::landing_queue::waiting_on(&queue, "first")
                .unwrap()
                .len(),
            1
        );
        // Back at work before the retry: the notice is withdrawn and the hold still waits.
        pass(&mut probe, &mut notifier, &mut state, 70.0, &options);
        assert!(state.pending.is_empty());
        assert_eq!(
            crate::landing_queue::waiting_on(&queue, "first")
                .unwrap()
                .len(),
            1
        );
        let finish = "● DONE: the real landing\n❯\n";
        let mut probe = ScriptedProbe(vec![vec![observation("first", "idle", Some(finish))]]);
        pass(&mut probe, &mut notifier, &mut state, 130.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        assert!(notifier.sent[0]
            .1
            .contains("Landing queue: second waited on first"));
        assert!(crate::landing_queue::waiting_on(&queue, "first")
            .unwrap()
            .is_empty());
        let _ = fs::remove_dir_all(&directory);
    }
}
