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
//! "Working" is not one state (#570 agent-progress-monitoring). The watcher masks the harness's
//! running-turn timer before it compares screens, so a clock that ticks is not mistaken for
//! output, and looks under the agent's pinned harness process in `/proc`
//! ([`crate::work_state`]): a working agent is then progressing (a tool uses CPU or moves bytes,
//! or the screen or token counter moves), blocked (its tool sleeps, on a file lock whose holder
//! the notice names, terminal input, a network socket, a pipe or an idle child), silent (no tool
//! runs and nothing moves), past the p95 of a gate step's recorded history, looping (the same
//! command or error over and over, or a retry banner), or inside a turn that has run for hours
//! with input queued behind it that the harness will not submit until the turn ends. A declared
//! progress metric ([`crate::progress_metric`]) that stops moving is reported even while all of
//! that activity continues.
//!
//! The watcher never answers a prompt and never types into any agent except the coordinator,
//! with one opt-in exception: an agent named with `--auto-deliver` whose long turn holds queued
//! input has that turn interrupted and the input submitted. Its episode memory (when each condition was first seen, whether it was
//! already reported, notices still waiting to be sent) is persisted after every pass, so
//! restarting the watcher does not report the same episode twice.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{AgentError, DrainOptions, QueueMessageState};
use crate::subagents::{controller_of, ManagedAgents, ManagedApi};

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
    /// How long a sender may wait for an agent's lifecycle lock before the jam is reported;
    /// zero turns the check off.
    pub delivery_wait: Duration,
    /// How long a turn may run, with input queued behind it, before it is reported.
    pub turn: Duration,
    /// Agents whose long turn with queued input is interrupted and the input submitted, not
    /// only reported (off for every agent unless named).
    pub auto_deliver: Vec<String>,
    /// How many times in a row the same command or error must show to count as a loop.
    pub loop_repeats: usize,
    /// How long a loop or a retry banner may hold before it is reported.
    pub loop_hold: Duration,
    /// Recorded gate step durations, for a step running past its p95.
    pub step_history: crate::work_state::StepHistory,
}

impl WatchAgentsOptions {
    fn threshold(&self, kind: EpisodeKind) -> Duration {
        match kind {
            EpisodeKind::ApprovalPrompt => self.approval,
            EpisodeKind::IdleNoVerdict => self.idle,
            EpisodeKind::ChildDone => self.child_done,
            EpisodeKind::Stalled
            | EpisodeKind::ToolBlocked
            | EpisodeKind::StreamSilent
            | EpisodeKind::BackgroundWait => self.stall,
            EpisodeKind::Dead => self.dead,
            EpisodeKind::TurnQueued => self.turn,
            EpisodeKind::Looping => self.loop_hold,
            // Each carries its own window: the step's p95, the metric's declared hours.
            EpisodeKind::OverExpected | EpisodeKind::NoProgress => Duration::ZERO,
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
    /// The agent has been inside one turn past `--turn-minutes` with input queued behind it.
    TurnQueued,
    /// The agent's declared progress metric has not moved for its declared hours while it works.
    NoProgress,
    /// A tool or gate step under the agent has used no CPU and moved no bytes past the stall
    /// threshold, and the kernel says what it waits on.
    ToolBlocked,
    /// No tool runs and neither the transcript nor the token counter has moved.
    StreamSilent,
    /// A gate step has run past the p95 of its recorded history.
    OverExpected,
    /// The same command or error repeats, or a retry banner holds.
    Looping,
    /// The agent waits on background work that is gone or idle.
    BackgroundWait,
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
            Self::TurnQueued => "turn-queued",
            Self::NoProgress => "no-progress",
            Self::ToolBlocked => "tool-blocked",
            Self::StreamSilent => "stream-silent",
            Self::OverExpected => "over-expected",
            Self::Looping => "looping",
            Self::BackgroundWait => "background-wait",
        }
    }

    /// Whether a notice of this kind is its detail alone: one line that already says what is
    /// wrong, such as `gate step Y blocked 40 min: ... held by ...`.
    const fn self_describing(self) -> bool {
        matches!(
            self,
            Self::TurnQueued
                | Self::NoProgress
                | Self::ToolBlocked
                | Self::StreamSilent
                | Self::OverExpected
                | Self::Looping
                | Self::BackgroundWait
        )
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
            Self::TurnQueued => format!(
                "inside one turn for over {} minutes with input queued behind it",
                minutes(options.turn)
            ),
            Self::NoProgress => "working while its declared progress metric stands still".to_owned(),
            Self::ToolBlocked => format!(
                "a tool or gate step blocked for over {} minutes",
                minutes(options.stall)
            ),
            Self::StreamSilent => format!(
                "working with no tool running and no output or tokens for over {} minutes",
                minutes(options.stall)
            ),
            Self::OverExpected => "a gate step running past its recorded p95".to_owned(),
            Self::Looping => format!(
                "repeating one command or error, or retrying, for over {} minutes",
                minutes(options.loop_hold)
            ),
            Self::BackgroundWait => "waiting on background work that is gone or idle".to_owned(),
        }
    }

    /// The notice line for one agent.
    fn line(self, agent: &str, detail: &str, options: &WatchAgentsOptions) -> String {
        if self.self_describing() {
            format!("- {agent}: {detail}")
        } else {
            format!("- {agent}: {} ({detail})", self.describe(options))
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
    /// The agent to tell about this one, when it is not the coordinator: the nearest reachable
    /// agent up its `managed_by` chain.
    pub recipient: Option<String>,
    /// The agent's agentctl queue, read against the screen's running turn.
    pub backlog: crate::turn_state::Backlog,
    /// What runs under the agent's pinned harness process, when it could be sampled.
    pub work: Option<crate::work_state::WorkSample>,
    /// The agent's declared progress metric, read this pass.
    pub progress: Option<crate::progress_metric::Reading>,
}

/// Prompt phrases, lowercased, matched against the bottom of the screen: every harness's
/// approval, permission and trust phrases ([`crate::harness::approval_patterns`]).
use crate::harness::approval_patterns;

/// The prompt line at the bottom of `screen`, trimmed and bounded, when one is showing.
pub(crate) fn approval_prompt_line(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(PROMPT_WINDOW_LINES);
    lines[start..].iter().rev().find_map(|line| {
        let lowered = line.to_ascii_lowercase();
        approval_patterns()
            .iter()
            .any(|pattern| lowered.contains(pattern))
            .then(|| line.trim().chars().take(QUOTED_LINE_CHARS).collect())
    })
}

/// Bullets a harness draws at column 0 to open an assistant block: Claude Code's `●` (`⏺`
/// in older builds), Muse's `◆`, and Codex's `•` ([`crate::harness::reply_bullets`]).
use crate::harness::reply_bullets;

/// `text` without the reply bullet and spacing that open it, when it has one.
fn strip_bullet(text: &str) -> &str {
    text.strip_prefix(reply_bullets())
        .map_or(text, |rest| rest.trim_start_matches([' ', '\u{a0}']))
}

/// Whether `line` opens an assistant block.
fn is_reply_row(line: &str) -> bool {
    line.starts_with(reply_bullets())
}

/// Whether `line` echoes a user turn in the transcript: Claude Code and Muse draw `❯ text`,
/// Codex `› text`, and older Claude Code builds `> text`. An empty composer is not one.
fn is_user_turn(line: &str) -> bool {
    crate::harness::user_turn_glyphs().iter().any(|glyph| {
        line.strip_prefix(*glyph).is_some_and(|rest| {
            rest.starts_with([' ', '\u{a0}']) && !rest.trim_matches([' ', '\u{a0}']).is_empty()
        })
    })
}

/// Whether `line` opens or continues a tool call's output rather than prose: Claude Code
/// draws `⎿` under a tool call and Muse `└`; the rows below are that output until the next
/// block opens.
fn is_tool_output(line: &str) -> bool {
    line.trim_start()
        .starts_with(crate::harness::tool_output_glyphs())
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

/// Bottom screen lines scanned for a running turn.
const BACKGROUND_WINDOW_LINES: usize = 12;

// Claude Code's footer counts of background work and its scrolled-view pill (#574
// harness-adapters): read from every screen, since only Claude Code draws them.
use crate::harness::claude::{background_work, scrolled_below};

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
    // The leading byte keeps an unreadable screen distinct from an empty one. The running-turn
    // timer is masked: a clock that ticks every second is not new output (its token counter,
    // which moves only while the model streams, is kept).
    match screen {
        Some(text) => stable_hash(&format!("s{}", crate::turn_state::without_timer(text))),
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
    /// Seconds the condition may hold before it is due, when the finding sets its own (a gate
    /// step's p95, a progress metric's hours); otherwise the kind's threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<f64>,
}

/// The last counters seen for one tool or gate step, and when they last moved.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
struct WorkMark {
    cpu_ticks: u64,
    io_bytes: u64,
    changed_at: f64,
}

/// What the watcher remembers about one agent between passes.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AgentMemory {
    episode: Option<Episode>,
    screen_hash: u64,
    screen_changed_at: f64,
    /// Counters of each tool and gate step under the harness, by [`crate::work_state::WorkItem::key`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    work: BTreeMap<String, WorkMark>,
    /// When a tool or step last used CPU, moved bytes, started or ended.
    #[serde(default)]
    work_changed_at: f64,
    /// When the watcher first saw the current stretch of work (status working or a turn running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_since: Option<f64>,
    /// Hash of the declared progress metric's last value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    progress_hash: Option<u64>,
    /// When the progress metric last changed (or was first read).
    #[serde(default)]
    progress_changed_at: f64,
}

/// A condition one observation shows: its kind and the line proving it, and, when the condition
/// carries its own clock, when it began and how long it may hold.
struct Finding {
    kind: EpisodeKind,
    detail: String,
    since: Option<f64>,
    window: Option<f64>,
}

impl Finding {
    fn new(kind: EpisodeKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            since: None,
            window: None,
        }
    }

    fn timed(
        kind: EpisodeKind,
        detail: impl Into<String>,
        since: f64,
        window: Option<f64>,
    ) -> Self {
        Self {
            kind,
            detail: detail.into(),
            since: Some(since),
            window,
        }
    }
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
    /// Who is told, when not the coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipient: Option<String>,
    /// A manager could not take this notice, so it goes to the coordinator from now on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    redirected: bool,
    /// For `turn-queued` under `--auto-deliver`, what interrupting the turn did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery: Option<String>,
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
    /// Who the batch is for, when not the coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recipient: Option<String>,
}

/// The persisted watcher state.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct WatchState {
    agents: BTreeMap<String, AgentMemory>,
    pending: Vec<PendingNotice>,
    inflight: Option<InflightBatch>,
    /// When the coordinator was last sent a batch.
    last_notice_at: Option<f64>,
    /// When each other recipient was last sent a batch.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    last_notice_by: BTreeMap<String, f64>,
    /// For each agent whose deliveries are jammed and were reported, when the jam was
    /// reported, so one jam is reported once.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    jams_reported: BTreeMap<String, f64>,
}

/// Where observations come from; the production source reads the registry through Herdr.
pub trait Probe {
    /// Observe every agent that should be watched this pass. An error means the team
    /// could not be listed at all; the pass reports it and starts no episodes.
    fn observe(&mut self) -> Result<Vec<Observation>>;
    /// For each registered agent that senders are waiting to deliver to, its name and the
    /// [`crate::lock_wait::contention`] report. The default sees no waits.
    fn delivery_waits(&mut self) -> Vec<(String, Value)> {
        Vec::new()
    }
}

/// Where notices go; the production sink queues one message for the coordinator.
pub trait Notifier {
    /// Durably queue `text` for `coordinator` under `message_id` and return the queue
    /// outcome spelling. An error means nothing was queued and the batch is retried.
    fn notify(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String>;
    /// [`Self::notify`] for a notice that must not wait behind the jam it reports: it is sent
    /// as a priority sender. The default sends it as any other.
    fn notify_urgent(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String> {
        self.notify(coordinator, text, message_id)
    }
    /// Interrupt `agent`'s running turn and submit the input its harness holds for later
    /// (`--auto-deliver`), returning one line saying what happened. The default cannot.
    fn interrupt_and_submit(&mut self, agent: &str) -> Result<String> {
        Err(fail(format!("this watcher cannot interrupt {agent}")))
    }
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

/// Rows near the bottom of the screen searched for a retry banner.
const RETRY_WINDOW_LINES: usize = 15;

/// Why an agent seems to go round in circles, from its screen: a retry banner near the bottom
/// (`Retrying in 5 seconds… (attempt 3/10)`), the same tool command as its last `repeats` actions
/// (Codex `• Ran CMD`, Claude Code `● Tool(ARGS)`), or one error line in `repeats` different
/// action blocks (one compiler run that prints the same error for several places is one block).
fn repeated_activity(screen: &str, repeats: usize) -> Option<String> {
    let lines: Vec<&str> = screen.lines().collect();
    let start = lines.len().saturating_sub(RETRY_WINDOW_LINES);
    for line in &lines[start..] {
        let lowered = line.to_ascii_lowercase();
        let attempt = lowered.split("attempt ").skip(1).any(|rest| {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest[digits..].starts_with('/')
        });
        // Harness banners only: Claude Code's `API Error ... Retrying in 8 seconds… (attempt
        // 4/10)` and Codex's `■`/`⚠` status rows; a tool printing "reconnecting" is not one.
        let harness_row = line.trim_start().starts_with(['■', '⚠']);
        if (lowered.contains("retrying in") && (attempt || lowered.contains("error")))
            || (harness_row && (lowered.contains("retrying") || lowered.contains("reconnecting")))
        {
            return Some(format!(
                "retry banner: {}",
                line.trim()
                    .chars()
                    .take(QUOTED_LINE_CHARS)
                    .collect::<String>()
            ));
        }
    }
    if repeats < 2 {
        return None;
    }
    let mut actions: Vec<String> = Vec::new();
    let mut errors: BTreeMap<String, std::collections::BTreeSet<usize>> = BTreeMap::new();
    for line in &lines {
        // Every action row counts, so a test run, an edit and the same test again is not a
        // loop: Codex's `• Ran`, `• Edited`, `• Explored` ..., Claude Code's `● Tool(ARGS)`.
        let action = line
            .strip_prefix("• ")
            .filter(|rest| {
                rest.split_whitespace().next().is_some_and(|verb| {
                    matches!(
                        verb,
                        "Ran"
                            | "Edited"
                            | "Explored"
                            | "Read"
                            | "Updated"
                            | "Added"
                            | "Deleted"
                            | "Searched"
                            | "Called"
                            | "Wrote"
                            | "Applied"
                    )
                })
            })
            .or_else(|| {
                line.strip_prefix("● ")
                    .filter(|rest| rest.contains('(') && rest.trim_end().ends_with(')'))
            });
        if let Some(action) = action {
            let action = action.strip_prefix("Ran ").unwrap_or(action);
            actions.push(action.trim().chars().take(QUOTED_LINE_CHARS).collect());
            continue;
        }
        let text = line
            .trim_start()
            .trim_start_matches(['└', '⎿', '│', ' '])
            .trim_end();
        if ["Error:", "error:", "ERROR:", "error["]
            .iter()
            .any(|prefix| text.starts_with(prefix))
        {
            errors
                .entry(text.chars().take(QUOTED_LINE_CHARS).collect())
                .or_default()
                .insert(actions.len());
        }
    }
    if actions.len() >= repeats {
        let tail = &actions[actions.len() - repeats..];
        if tail.iter().all(|action| *action == tail[0]) {
            return Some(format!(
                "the same command ran {repeats} times in a row: {}",
                tail[0]
            ));
        }
    }
    errors
        .into_iter()
        .filter(|(_, blocks)| blocks.len() >= repeats)
        .max_by_key(|(_, blocks)| blocks.len())
        .map(|(error, blocks)| format!("the same error in {} attempts: {error}", blocks.len()))
}

/// How long ago, as a notice phrase.
fn ago(now: f64, since: f64) -> String {
    crate::turn_state::format_age(now - since)
}

/// A gate step past the p95 of its recorded history, the furthest past it first.
fn over_expected(
    observation: &Observation,
    options: &WatchAgentsOptions,
    memory: &AgentMemory,
    now: f64,
) -> Option<Finding> {
    let work = observation.work.as_ref()?;
    work.steps()
        .filter(|step| step.root.is_some())
        .filter_map(|step| {
            let name = step.step.as_deref()?;
            let started = step.started_at?;
            let (p95, runs) = options.step_history.p95(name)?;
            let age = now - started;
            (age > p95 && p95 > 0.0).then_some((age / p95, step, name, started, p95, runs))
        })
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, step, name, started, p95, runs)| {
            let blocked = memory
                .work
                .get(&step.key)
                .filter(|mark| now - mark.changed_at >= options.stall.as_secs_f64())
                .and(step.waits_on.as_deref())
                .map_or_else(String::new, |why| format!("; no CPU or I/O for a while: {why}"));
            Finding::timed(
                EpisodeKind::OverExpected,
                format!(
                    "gate step {name} running {}, past its p95 of {} over {runs} recorded runs{blocked}",
                    ago(now, started),
                    crate::turn_state::format_age(p95)
                ),
                started,
                Some(p95),
            )
        })
}

/// The tool or gate step that has used no CPU and moved no bytes the longest, past the stall
/// threshold, with what the kernel says it waits on. A gate step is judged on its own, so one
/// step stuck behind a lock is reported while its siblings run; a tool command only when every
/// tool is still and the screen has not moved either.
fn blocked_work(
    observation: &Observation,
    options: &WatchAgentsOptions,
    memory: &AgentMemory,
    now: f64,
) -> Option<Finding> {
    let work = observation.work.as_ref()?;
    let stall = options.stall.as_secs_f64();
    let still_since = |key: &str| memory.work.get(key).map_or(now, |mark| mark.changed_at);
    if let Some((step, since)) = work
        .steps()
        .filter(|step| step.waits_on.is_some() && step.root.is_some())
        .map(|step| (step, still_since(&step.key)))
        .filter(|(_, since)| now - since >= stall)
        .min_by(|left, right| left.1.total_cmp(&right.1))
    {
        return Some(Finding::timed(
            EpisodeKind::ToolBlocked,
            format!(
                "gate step {} blocked {}: {}",
                step.step.as_deref().unwrap_or("?"),
                ago(now, since),
                step.waits_on.as_deref().unwrap_or("no CPU or I/O")
            ),
            since,
            None,
        ));
    }
    let tools: Vec<_> = work.tools().collect();
    if tools.is_empty() {
        return None;
    }
    let since = tools
        .iter()
        .map(|tool| still_since(&tool.key))
        .fold(memory.screen_changed_at, f64::max);
    if now - since < stall {
        return None;
    }
    // The most specific explanation: a cause other than waiting for a child, if any tool has one.
    let tool = tools
        .iter()
        .find(|tool| {
            tool.waits_on
                .as_deref()
                .is_some_and(|why| !why.ends_with("waits for a child process"))
        })
        .or_else(|| tools.first())
        .expect("tools is not empty");
    Some(Finding::timed(
        EpisodeKind::ToolBlocked,
        format!(
            "tool blocked {}: {}",
            ago(now, since),
            tool.waits_on
                .clone()
                .unwrap_or_else(|| format!("`{}` uses no CPU and moves no bytes", tool.command))
        ),
        since,
        None,
    ))
}

/// The condition one observation shows, with the line or error that proves it. `memory` is
/// already updated with this pass's screen, work counters and progress metric.
/// Screen changes are handled by the caller: a changed screen ends a stall episode.
fn classify(
    observation: &Observation,
    options: &WatchAgentsOptions,
    memory: &AgentMemory,
    now: f64,
) -> Option<Finding> {
    if observation.status == "gone" {
        return Some(Finding::new(EpisodeKind::Dead, observation.detail.clone()));
    }
    let screen = observation.screen.as_deref()?;
    if let Some(line) = approval_prompt_line(screen) {
        return Some(Finding::new(EpisodeKind::ApprovalPrompt, line));
    }
    let turn = crate::turn_state::read_screen(screen);
    // A turn past --turn-minutes with input queued behind it: a harness that submits queued
    // input only when the turn ends will not read it until then.
    let working_for = memory.working_since.map(|since| now - since);
    if let Some(age) = crate::turn_state::turn_age(&turn, working_for) {
        let queued = turn.queued > 0 || observation.backlog.waiting > 0;
        if turn.running && queued && age >= options.turn.as_secs_f64() {
            let detail = crate::turn_state::summary(&turn, &observation.backlog, Some(age))
                .unwrap_or_else(|| {
                    format!("inside one turn for {}", crate::turn_state::format_age(age))
                });
            return Some(Finding::timed(
                EpisodeKind::TurnQueued,
                detail,
                now - age,
                Some(options.turn.as_secs_f64()),
            ));
        }
    }
    if matches!(observation.status.as_str(), "idle" | "done")
        && observation.has_delivered
        && !observation.has_pending
    {
        let running = turn_running(screen);
        let background = background_work(screen);
        let work = background
            .clone()
            .or_else(|| running.then(|| "esc to interrupt".to_owned()));
        // A running turn means a verdict on screen is not this turn's last word yet, so only
        // the stall rule applies. Background shells or agents left running do not: a child
        // that said it is done is done.
        if !running {
            if let Some(verdict) = verdict_line(screen, &options.markers) {
                return Some(Finding::new(EpisodeKind::ChildDone, verdict));
            }
        }
        if let (Some(sample), Some(what), false) = (observation.work.as_ref(), &background, running)
        {
            if what.contains("shell") {
                // The agent waits on its background shells: gone is reported at the idle
                // threshold, idle ones at the stall threshold, and running ones not at all.
                let since = memory.screen_changed_at.max(memory.work_changed_at);
                if sample.tools().next().is_none() {
                    return Some(Finding::timed(
                        EpisodeKind::BackgroundWait,
                        format!(
                            "status {} waiting on background work the screen counts ({what}), but no shell runs under its harness",
                            observation.status
                        ),
                        since,
                        Some(options.idle.as_secs_f64()),
                    ));
                }
                return blocked_work(observation, options, memory, now).map(|finding| Finding {
                    kind: EpisodeKind::BackgroundWait,
                    detail: format!(
                        "status {} waiting on background work ({what}) that is idle: {}",
                        observation.status, finding.detail
                    ),
                    ..finding
                });
            }
        }
        // A running turn or its own background work is not idle, whatever Herdr's
        // status says: the stall rule still reports it if the screen stops changing.
        if let Some(what) = work {
            if running && observation.work.is_some() {
                return working_finding(observation, options, memory, now, screen, &turn);
            }
            return Some(Finding::new(
                EpisodeKind::Stalled,
                format!(
                    "status {} while the screen shows work running ({what}); screen unchanged",
                    observation.status
                ),
            ));
        }
        return Some(Finding::new(
            EpisodeKind::IdleNoVerdict,
            format!("status {} with no verdict marker", observation.status),
        ));
    }
    if observation.status == "working" {
        return working_finding(observation, options, memory, now, screen, &turn);
    }
    None
}

/// What is wrong with an agent that is inside a turn, if anything: a loop, a gate step past its
/// p95, a progress metric that stands still, a blocked tool, or a silent stream. Without a
/// `/proc` sample only the screen rule applies: unchanged screen, stalled.
fn working_finding(
    observation: &Observation,
    options: &WatchAgentsOptions,
    memory: &AgentMemory,
    now: f64,
    screen: &str,
    turn: &crate::turn_state::ScreenTurn,
) -> Option<Finding> {
    if let Some(detail) = repeated_activity(screen, options.loop_repeats) {
        return Some(Finding::new(EpisodeKind::Looping, detail));
    }
    if let Some(finding) = over_expected(observation, options, memory, now) {
        return Some(finding);
    }
    if let Some(reading) = observation.progress.as_ref() {
        let since = memory.progress_changed_at;
        if now - since >= reading.window_seconds {
            let value = reading.quoted();
            let mut detail = format!(
                "no progress for {}: {} unchanged while it works",
                ago(now, since),
                reading.source
            );
            if !value.is_empty() {
                detail.push_str(&format!(" (\"{value}\")"));
            }
            if let Some(error) = &reading.error {
                detail.push_str(&format!("; last read failed: {error}"));
            }
            return Some(Finding::timed(
                EpisodeKind::NoProgress,
                detail,
                since,
                Some(reading.window_seconds),
            ));
        }
    }
    let Some(work) = observation.work.as_ref() else {
        return Some(Finding::new(EpisodeKind::Stalled, "screen unchanged"));
    };
    if let Some(finding) = blocked_work(observation, options, memory, now) {
        return Some(finding);
    }
    if work.items.is_empty() {
        let since = memory.screen_changed_at.max(memory.work_changed_at);
        if now - since >= options.stall.as_secs_f64() {
            let moved = if turn.tokens.is_some() {
                "the transcript and the token counter have not moved"
            } else {
                "the screen has not moved"
            };
            return Some(Finding::timed(
                EpisodeKind::StreamSilent,
                format!(
                    "model stream silent {}: {moved} and no tool is running",
                    ago(now, since)
                ),
                since,
                None,
            ));
        }
    }
    None
}

/// How far a finding's own clock may move between passes and still be the same episode.
const CLOCK_SLACK_SECONDS: f64 = 600.0;

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
    // A notice that reports something the watcher did (an --auto-deliver interrupt) is news
    // even after the condition ended, and the interrupt is what usually ends it.
    state.pending.retain(|notice| {
        notice.agent != agent || notice.kind != episode.kind || notice.delivery.is_some()
    });
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
        "due": now - episode.since >= episode_window(episode, options),
        "detail": detail,
    })
}

/// Seconds `episode` may hold before it is due.
fn episode_window(episode: &Episode, options: &WatchAgentsOptions) -> f64 {
    episode
        .window
        .unwrap_or_else(|| options.threshold(episode.kind).as_secs_f64())
}

/// Bring the counters of the agent's tools and gate steps, its working stretch, and its
/// progress metric up to date with `observation`.
fn remember_work(memory: &mut AgentMemory, observation: &Observation, now: f64) {
    if let Some(work) = observation.work.as_ref() {
        let mut marks = BTreeMap::new();
        let mut moved = work.items.len() != memory.work.len();
        for item in &work.items {
            let mark = match memory.work.get(&item.key) {
                Some(old) if old.cpu_ticks == item.cpu_ticks && old.io_bytes == item.io_bytes => {
                    old.clone()
                }
                // Moved since the last pass, or first seen: its clock starts now.
                _ => {
                    moved = true;
                    WorkMark {
                        cpu_ticks: item.cpu_ticks,
                        io_bytes: item.io_bytes,
                        changed_at: now,
                    }
                }
            };
            marks.insert(item.key.clone(), mark);
        }
        if moved {
            memory.work_changed_at = now;
        }
        memory.work = marks;
    } else {
        memory.work.clear();
    }
    let working = observation.status == "working"
        || observation
            .screen
            .as_deref()
            .is_some_and(crate::turn_state::screen_running);
    memory.working_since = if working {
        Some(memory.working_since.unwrap_or(now))
    } else {
        None
    };
    match observation.progress.as_ref() {
        Some(reading) => {
            // A failed read is no evidence of change; the last value stands.
            if let Some(value) = reading.value.as_deref() {
                let hash = stable_hash(value);
                if memory.progress_hash != Some(hash) {
                    memory.progress_hash = Some(hash);
                    memory.progress_changed_at = now;
                }
            } else if memory.progress_hash.is_none() && memory.progress_changed_at == 0.0 {
                memory.progress_changed_at = now;
            }
        }
        None => {
            memory.progress_hash = None;
            memory.progress_changed_at = 0.0;
        }
    }
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
    remember_work(&mut memory, observation, now);
    let found = classify(observation, options, &memory, now)
        .map(|finding| (finding.kind, finding.detail, finding.since, finding.window));
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
            kind @ (EpisodeKind::Stalled
            | EpisodeKind::IdleNoVerdict
            | EpisodeKind::ChildDone
            | EpisodeKind::StreamSilent
            | EpisodeKind::ToolBlocked
            | EpisodeKind::BackgroundWait),
            ..,
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
        if found
            .as_ref()
            .is_none_or(|(kind, ..)| *kind != cleared.kind)
        {
            let cleared = cleared.clone();
            withdraw_pending(state, &observation.name, &cleared);
        }
    }
    let Some((kind, detail, given_since, window)) = found else {
        memory.episode = None;
        state.agents.insert(observation.name.clone(), memory);
        return None;
    };
    // A finish with a different verdict line is a later turn: the earlier episode ends here,
    // with its unsent notice, and the new one is reported in its own right. A condition that
    // carries its own clock is a new episode when that clock moved by more than a few minutes
    // (a new turn, a new step, a metric that moved and stopped again).
    let turn = (kind == EpisodeKind::ChildDone).then(|| verdict_turn(&detail));
    if let Some(cleared) = memory.episode.take_if(|episode| {
        episode.kind == kind
            && (episode.turn != turn
                // A turn's age may be read from its timer one pass and the watcher's own clock
                // the next, so a turn-queued episode keeps its first clock.
                || (kind != EpisodeKind::TurnQueued
                    && given_since
                        .is_some_and(|since| (episode.since - since).abs() >= CLOCK_SLACK_SECONDS)))
    }) {
        withdraw_pending(state, &observation.name, &cleared);
    }
    // A stall is timed from the last screen change, not from the first working poll:
    // an agent that works for an hour with changing output, then stops changing,
    // has been stalled only since it stopped.
    // Idle is timed from the last screen change too: Herdr can report a Claude Code agent
    // idle while its output still streams, and new output is not idleness.
    let since = match (kind, given_since) {
        // The finding's own clock, kept steady across passes against a jittering screen timer.
        (_, Some(given)) => match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since,
            _ => given,
        },
        (EpisodeKind::Stalled, None) => memory.screen_changed_at,
        (EpisodeKind::IdleNoVerdict | EpisodeKind::ChildDone, None) => match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since.max(memory.screen_changed_at),
            _ => now,
        },
        (_, None) => match &memory.episode {
            Some(episode) if episode.kind == kind => episode.since,
            _ => now,
        },
    };
    let notified =
        matches!(&memory.episode, Some(episode) if episode.kind == kind && episode.notified);
    let episode = Episode {
        kind,
        since,
        notified,
        turn,
        window,
    };
    let due = now - since >= episode_window(&episode, options);
    memory.episode = Some(episode);
    let age = now - since;
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
            recipient: observation
                .recipient
                .clone()
                .filter(|recipient| *recipient != options.coordinator),
            redirected: false,
            delivery: None,
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

/// Send the pending notices, one message per recipient, as the rate limit allows. Returns the
/// report row of the first batch, with any later batches of the same pass under `more`; on
/// failure the batch stays in flight and is retried next pass.
fn flush_notices(
    state: &mut WatchState,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Value {
    if options.check_only {
        return json!({"sent": false, "reason": "check only", "waiting": state.pending.len()});
    }
    let mut first = flush_batch(state, notifier, now, options);
    let mut more = Vec::new();
    let mut last = first.clone();
    while last["sent"] == true || last["redirected"] == true {
        last = flush_batch(state, notifier, now, options);
        if last["sent"] == true || last["reason"] != "nothing to report" {
            more.push(last.clone());
        }
    }
    if !more.is_empty() {
        first["more"] = json!(more);
    }
    first
}

/// The rate limit for `recipient` (`None`, the coordinator) still holds at `now`.
fn rate_limited(
    state: &WatchState,
    recipient: Option<&str>,
    now: f64,
    options: &WatchAgentsOptions,
) -> bool {
    let last = match recipient {
        None => state.last_notice_at,
        Some(recipient) => state.last_notice_by.get(recipient).copied(),
    };
    last.is_some_and(|last| now - last < options.min_notice_interval.as_secs_f64())
}

/// Send one batch: the one in flight, else every pending notice for the first recipient, in
/// the order notices became due, whose rate limit allows a message.
fn flush_batch(
    state: &mut WatchState,
    notifier: &mut dyn Notifier,
    now: f64,
    options: &WatchAgentsOptions,
) -> Value {
    let batched = |notice: &PendingNotice| notice.kind != EpisodeKind::ChildDone;
    if state.inflight.is_none() {
        if !state.pending.iter().any(batched) {
            return json!({"sent": false, "reason": "nothing to report"});
        }
        let mut recipients: Vec<Option<String>> = Vec::new();
        for notice in state.pending.iter().filter(|notice| batched(notice)) {
            if !recipients.contains(&notice.recipient) {
                recipients.push(notice.recipient.clone());
            }
        }
        let Some(recipient) = recipients
            .into_iter()
            .find(|recipient| !rate_limited(state, recipient.as_deref(), now, options))
        else {
            return json!({"sent": false, "reason": "rate limited", "waiting": state.pending.len()});
        };
        // The batch ID is derived from the notices themselves, so a watcher killed
        // between queueing a batch and persisting that fact re-derives the same ID
        // after a restart and the recipient's queue recognises it as sent.
        let keys: Vec<String> = state
            .pending
            .iter()
            .filter(|notice| batched(notice) && notice.recipient == recipient)
            .map(PendingNotice::key)
            .collect();
        let digest = match recipient.as_deref() {
            None => stable_hash(&keys.join("\n")),
            Some(recipient) => stable_hash(&format!("{recipient}\n{}", keys.join("\n"))),
        };
        state.inflight = Some(InflightBatch {
            message_id: format!("agent-watch-{digest:016x}"),
            keys,
            recipient,
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
        .map(|notice| notice.kind.line(&notice.agent, &notice.detail, options))
        .collect::<Vec<_>>();
    let text = format!(
        "[agentctl watch-agents] {} agent(s) need attention:\n{}\nInspect with `agentctl status NAME` and `agentctl read NAME`. The watcher never answers prompts itself.",
        lines.len(),
        lines.join("\n")
    );
    let recipient = batch.recipient.as_deref().unwrap_or(&options.coordinator);
    match notifier.notify(recipient, &text, &batch.message_id) {
        Ok(outcome) => {
            state
                .pending
                .retain(|notice| !batch.keys.contains(&notice.key()));
            state.inflight = None;
            match batch.recipient.as_deref() {
                None => state.last_notice_at = Some(now),
                Some(other) => {
                    state.last_notice_by.insert(other.to_owned(), now);
                }
            }
            let mut row = json!({"sent": true, "message_id": batch.message_id, "outcome": outcome, "notices": lines.len()});
            if let Some(other) = batch.recipient.as_deref() {
                row["recipient"] = json!(other);
            }
            row
        }
        Err(error) => {
            // A manager that could not take its batch (stopped, renamed, paused since) must not
            // hold every later notice behind it: its notices go to the coordinator instead, in a
            // batch of their own. One that did reach the manager's queue may then be read twice,
            // which is a lesser harm than a notice nobody reads.
            if let Some(manager) = batch.recipient.as_deref() {
                for notice in &mut state.pending {
                    if batch.keys.contains(&notice.key()) {
                        notice.recipient = None;
                        notice.redirected = true;
                    }
                }
                state.inflight = None;
                return json!({
                    "sent": false,
                    "message_id": batch.message_id,
                    "recipient": manager,
                    "reason": format!("{error}; its notices go to the coordinator instead"),
                    "redirected": true,
                    "waiting": state.pending.len(),
                });
            }
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
        let mut sent = notifier.notify(notice_target(options, &notice), &text, &message_id);
        // A manager that cannot take the finish (stopped, renamed, paused since) passes it to
        // the coordinator, now and from then on.
        if sent.is_err() && notice.recipient.is_some() {
            if let Some(pending) = state
                .pending
                .iter_mut()
                .find(|pending| pending.key() == key)
            {
                pending.recipient = None;
                pending.redirected = true;
            }
            sent = notifier.notify(&options.coordinator, &text, &message_id);
        }
        match sent {
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

/// Who is told about a notice: the agent's nearest reachable manager, as the pass decided it,
/// else the configured coordinator.
fn notice_target<'a>(options: &'a WatchAgentsOptions, notice: &'a PendingNotice) -> &'a str {
    notice.recipient.as_deref().unwrap_or(&options.coordinator)
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
    // An agent the pass ran out of time for keeps its memory as it was: its episodes neither
    // advance nor end, and the report says the pass was partial (#571 watch-agents-scale).
    let (unchecked, observations): (Vec<Observation>, Vec<Observation>) = observations
        .into_iter()
        .partition(|row| row.status == UNCHECKED);
    let unchecked: Vec<String> = unchecked.into_iter().map(|row| row.name).collect();
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
    // An agent named with --auto-deliver whose long turn holds queued input has the turn
    // interrupted and the input submitted, once per episode; the notice says what happened.
    if !options.check_only {
        for notice in &mut state.pending {
            if notice.kind == EpisodeKind::TurnQueued
                && notice.delivery.is_none()
                && options.auto_deliver.contains(&notice.agent)
            {
                let outcome = match notifier.interrupt_and_submit(&notice.agent) {
                    Ok(line) => format!("auto-delivered: {line}"),
                    Err(error) => format!("auto-deliver failed: {error}"),
                };
                notice.detail = format!("{}; {outcome}", notice.detail);
                notice.delivery = Some(outcome);
            }
        }
    }
    // Whom to tell is decided again each pass from the latest observation, so a notice still
    // waiting does not go to a manager that has since stopped, paused or lost the agent.
    let in_flight: Vec<String> = state
        .inflight
        .as_ref()
        .map(|batch| batch.keys.clone())
        .unwrap_or_default();
    for notice in &mut state.pending {
        if notice.redirected || in_flight.contains(&notice.key()) {
            continue;
        }
        if let Some(observation) = observations.iter().find(|row| row.name == notice.agent) {
            notice.recipient = observation
                .recipient
                .clone()
                .filter(|recipient| *recipient != options.coordinator);
        }
    }
    // A recipient's last send matters only while it can still rate-limit the next one.
    let window = options.min_notice_interval.as_secs_f64();
    state.last_notice_by.retain(|_, at| now - *at < window);
    // First, as a priority sender: the other notices wait for the coordinator's lock like any
    // ordinary sender, so behind a jam on the coordinator they would hold this one up.
    let delivery_waits = report_delivery_jams(probe, notifier, state, now, options);
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
            && unchecked.is_empty()
            && episodes.iter().all(|row| {
                // A finished child is news for its coordinator, not a fault in the team.
                row["kind"] == EpisodeKind::ChildDone.as_str()
                    || (row["reported"] != true && row["due"] != true)
            }),
        "probe_error": probe_error,
        "agents": seen.len(),
        "episodes": episodes,
        "scrolled": scrolled,
        "unchecked": unchecked,
        "notice": notice,
        "child_done": child_done,
        "waiting": state.pending.len(),
        "delivery_waits": delivery_waits,
    })
}

/// Report, once per jam, each agent whose senders have waited for its lifecycle lock longer
/// than `--delivery-wait-minutes`: every managed send holds that lock until its delivery is
/// confirmed, so a slow or busy agent makes every message to it wait in turn, and the senders
/// only look idle (#556 chat-delivery-latency). The notice goes to the coordinator as a priority
/// sender, because it may be about the coordinator's own lock. A jam lasts while any sender has
/// waited past the threshold, whichever sender that is, so fifteen senders queued behind one
/// another make one notice, not fifteen; it ends in the first pass that finds none. Returns
/// every agent with waiters, for the pass report.
fn report_delivery_jams(
    probe: &mut dyn Probe,
    notifier: &mut dyn Notifier,
    state: &mut WatchState,
    now: f64,
    options: &WatchAgentsOptions,
) -> Vec<Value> {
    if options.delivery_wait.is_zero() {
        state.jams_reported.clear();
        return Vec::new();
    }
    let waits = probe.delivery_waits();
    let threshold = options.delivery_wait.as_secs_f64();
    let mut rows = Vec::new();
    let mut still_jammed = BTreeMap::new();
    for (agent, report) in &waits {
        let longest = crate::lock_wait::longest_wait(report);
        let Some(sentence) = crate::lock_wait::describe(agent, report) else {
            continue;
        };
        let mut row = json!({"agent": agent, "longest_wait_seconds": longest, "detail": sentence});
        if longest >= threshold {
            if let Some(reported_at) = state.jams_reported.get(agent).copied() {
                still_jammed.insert(agent.clone(), reported_at);
                row["reported"] = json!(true);
            } else if options.check_only {
                row["due"] = json!(true);
            } else {
                let text = format!(
                    "[agentctl watch-agents] deliveries to {agent} are waiting: {sentence}. Every \
                     managed send holds that lock until its delivery is confirmed, so these \
                     messages are typed one at a time; a chat bridge's requests go first. \
                     `agentctl chat status --registry REGISTRY --bridge-state DIR` shows which \
                     chat requests are caught in it."
                );
                let message_id = format!("agent-watch-jam-{agent}-{}", now as u64);
                match notifier.notify_urgent(&options.coordinator, &text, &message_id) {
                    Ok(outcome) => {
                        still_jammed.insert(agent.clone(), now);
                        row["reported"] = json!(true);
                        row["outcome"] = json!(outcome);
                    }
                    Err(error) => row["error"] = json!(error.to_string()),
                }
            }
        }
        rows.push(row);
    }
    state.jams_reported = still_jammed;
    rows
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
        if let Some(names) = report["unchecked"]
            .as_array()
            .filter(|names| !names.is_empty())
        {
            let names: Vec<&str> = names.iter().filter_map(Value::as_str).collect();
            eprintln!(
                "agentctl watch-agents: the pass budget ran out before {} agent(s) were checked: {}",
                names.len(),
                names.join(", ")
            );
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

/// Agents checked at once in a pass. A check is mostly waiting on Herdr, and a loaded server
/// can take seconds to read one busy pane's scrollback.
const PASS_WORKERS: usize = 8;
/// The longest any one Herdr call may run in a pass: Herdr's usual control timeout.
pub const PASS_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest the pass's one `agentcloudctl list` may run (it unpacks itself first, so it is
/// given longer than a Herdr call), and never past the pass's budget.
const PASS_CLOUD_TIMEOUT: Duration = Duration::from_secs(30);
/// The status of an agent the pass ran out of time to check: it is reported as unchecked, and
/// its episodes neither advance nor end.
const UNCHECKED: &str = "unchecked";

/// The recent screen last read of an idle agent, with what it was read under: while the agent
/// stays idle in the same pane, at the bottom, with the same visible rows, nothing new can have
/// reached its screen, so the costly recent read is not repeated.
#[derive(Clone)]
struct KeptScreen {
    pane: String,
    status: String,
    visible: u64,
    recent: String,
}

/// One agent's check, made on a pool thread.
struct Checked {
    row: Value,
    observation: Option<Observation>,
    kept: Option<KeptScreen>,
    /// The screen came from the last pass, not from Herdr.
    reused: bool,
}

/// The production probe: one registry, read through Herdr. The coordinator itself,
/// paused agents, cloud sessions viewed through a terminal, and records that are not
/// running are not watched; a `launch_failed` record was already reported by `start`.
///
/// A pass checks only records that are running or starting and not paused (#571
/// watch-agents-scale): the rest are judged from the record alone. Up to [`PASS_WORKERS`]
/// agents are checked at once; Herdr's read-only answers are kept for the pass (see
/// [`crate::client::HerdrClient::snapshotting`]), and so are `/proc`'s process and lock
/// tables. A pass that reaches its budget reports the agents it had no time for as unchecked
/// instead of reporting nothing.
pub struct ManagedProbe<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
    coordinator: String,
    budget: Option<Duration>,
    verbose: bool,
    /// Keep idle agents' screens for the next pass (off for a single pass, which has none).
    keep_screens: bool,
    screens: BTreeMap<String, KeptScreen>,
}

impl<'a, A: ManagedApi + ?Sized> ManagedProbe<'a, A> {
    /// Watch `registry` through `client`, exempting `coordinator`.
    pub fn new(client: &'a A, registry: &Path, coordinator: &str) -> Self {
        Self {
            client,
            registry: registry.to_owned(),
            coordinator: coordinator.to_owned(),
            budget: None,
            verbose: false,
            keep_screens: true,
            screens: BTreeMap::new(),
        }
    }

    /// End each pass's checks after `budget` (`None`: no limit), and, when `verbose`, say on
    /// standard error what each check found and how long it took.
    #[must_use]
    pub fn with_budget(mut self, budget: Option<Duration>, verbose: bool) -> Self {
        self.budget = budget;
        self.verbose = verbose;
        self
    }

    /// Whether a pass keeps idle agents' screens for the next one; a watcher that runs one
    /// pass (`--once`) has no next pass, so it skips the extra visible read that keeping needs.
    #[must_use]
    pub fn keeping_screens(mut self, keep: bool) -> Self {
        self.keep_screens = keep;
        self
    }

    fn progress(&self, line: &str) {
        if self.verbose {
            eprintln!("agentctl watch-agents: {line}");
        }
    }

    /// Check one running agent: its status, and, for an agent the watcher judges, its screen,
    /// the work under its harness, and its progress metric. `None` once the pass is out of
    /// time: a check that straddled the deadline may have had calls cancelled, and a cancelled
    /// call must not read as a dead agent.
    fn check(
        &self,
        manager: &ManagedAgents<'_, A>,
        row: &Value,
        proc_view: &crate::work_state::PassProc,
        deadline: Option<Instant>,
    ) -> Option<Checked> {
        let expired = || deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if expired() {
            return None;
        }
        let name = row["name"].as_str().unwrap_or_default().to_owned();
        let row = manager.watch_status(&name, &mut crate::subagents::FleetCache::new());
        let watched = name != self.coordinator
            && row["paused"] != true
            && row["move_pending"] != true
            && (row["record_error"] == true
                || (matches!(row["lifecycle"].as_str(), Some("running" | "starting"))
                    && row["adapter"].as_str() != Some("agentcloud")));
        if !watched {
            return (!expired()).then_some(Checked {
                row,
                observation: None,
                kept: None,
                reused: false,
            });
        }
        let gone = row["agent_status"] == "unknown" && !row["probe_error"].is_null();
        let status = row["agent_status"].as_str().unwrap_or("unknown").to_owned();
        let (screen, kept, reused) = if gone {
            (None, None, false)
        } else {
            self.screen(manager, &name, &status)
        };
        let observation = self.observation(row.clone(), name, gone, screen, proc_view);
        (!expired()).then_some(Checked {
            row,
            observation: Some(observation),
            kept,
            reused,
        })
    }

    /// The agent's recent screen, the screen to keep for the next pass, and whether the screen
    /// was kept from the last pass rather than read.
    fn screen(
        &self,
        manager: &ManagedAgents<'_, A>,
        name: &str,
        status: &str,
    ) -> (Option<String>, Option<KeptScreen>, bool) {
        // Only an idle or finished agent's screen holds still long enough to be worth keeping.
        let visible = (self.keep_screens && matches!(status, "idle" | "done"))
            .then(|| manager.watch_visible(name, SCREEN_LINES).ok())
            .flatten()
            .filter(|(pane, _)| self.client.pane_at_bottom(pane))
            .map(|(pane, text)| (pane, stable_hash(&text)));
        if let (Some((pane, visible)), Some(kept)) = (visible.as_ref(), self.screens.get(name)) {
            if kept.pane == *pane && kept.status == status && kept.visible == *visible {
                return (Some(kept.recent.clone()), Some(kept.clone()), true);
            }
        }
        let recent = manager.watch_read(name, SCREEN_LINES).ok();
        let kept = match (visible, recent.as_ref()) {
            (Some((pane, visible)), Some(recent)) => Some(KeptScreen {
                pane,
                status: status.to_owned(),
                visible,
                recent: recent.clone(),
            }),
            _ => None,
        };
        (recent, kept, false)
    }

    fn observation(
        &self,
        row: Value,
        name: String,
        gone: bool,
        screen: Option<String>,
        proc_view: &crate::work_state::PassProc,
    ) -> Observation {
        let queue = row["queue"]
            .as_str()
            .map_or_else(|| self.registry.join(&name).join("queue"), PathBuf::from);
        let now = SystemClock.now();
        let started = screen.as_deref().and_then(|screen| {
            crate::turn_state::turn_age(&crate::turn_state::read_screen(screen), None)
                .map(|age| now - age)
        });
        let backlog = crate::turn_state::backlog(&queue, now, started);
        // The pinned harness process (a custom-pane harness such as Muse is pinned as its
        // custom process), checked against its start time so a reused PID is never read as
        // the agent.
        let identity = if row["harness_identity"].is_object() {
            &row["harness_identity"]
        } else {
            &row["custom_process_identity"]
        };
        let work = match (gone, identity["pid"].as_u64()) {
            (false, Some(pid)) => u32::try_from(pid).ok().and_then(|pid| {
                crate::work_state::sample(proc_view, pid, identity["starttime_ticks"].as_u64())
            }),
            _ => None,
        };
        let progress = match crate::progress_metric::load(&self.registry, &name) {
            Ok(Some(declaration)) => {
                let cwd = row["cwd"]
                    .as_str()
                    .map_or_else(|| self.registry.clone(), PathBuf::from);
                Some(crate::progress_metric::read(&declaration, &cwd))
            }
            Ok(None) => None,
            Err(error) => Some(crate::progress_metric::Reading {
                error: Some(error.to_string()),
                window_seconds: f64::INFINITY,
                source: crate::progress_metric::path(&self.registry, &name)
                    .display()
                    .to_string(),
                ..crate::progress_metric::Reading::default()
            }),
        };
        Observation {
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
            screen,
            has_delivered: queue_has_entries(&queue, "processed"),
            has_pending: ["pending", "inflight"].iter().any(|field| {
                row[field]
                    .as_array()
                    .is_some_and(|values| !values.is_empty())
            }),
            recipient: None,
            backlog,
            work,
            progress,
            name,
        }
    }
}

/// Run `job` on every item, at most `workers` at once, keeping the items' order. A thread the
/// system refuses (a process limit under load) leaves the remaining items to the threads that
/// started, or to this one.
fn bounded_map<T: Sync, R: Send>(
    items: &[T],
    workers: usize,
    job: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> =
        items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    let work = || loop {
        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let Some(item) = items.get(index) else {
            break;
        };
        let result = job(item);
        *results[index].lock().expect("result lock poisoned") = Some(result);
    };
    std::thread::scope(|scope| {
        for _ in 1..workers.clamp(1, items.len().max(1)) {
            if std::thread::Builder::new()
                .spawn_scoped(scope, work)
                .is_err()
            {
                break;
            }
        }
        work();
    });
    results
        .into_iter()
        .map(|slot| {
            slot.into_inner()
                .expect("result lock poisoned")
                .expect("every item ran")
        })
        .collect()
}

fn queue_has_entries(queue: &Path, child: &str) -> bool {
    fs::read_dir(queue.join(child)).is_ok_and(|mut entries| entries.next().is_some())
}

impl<A: ManagedApi + ?Sized> Probe for ManagedProbe<'_, A> {
    /// Only an agent with a waiter record can have waiters, so the record directories name
    /// every candidate, the coordinator included.
    fn delivery_waits(&mut self) -> Vec<(String, Value)> {
        let Ok(entries) = fs::read_dir(self.registry.join(".waiting")) else {
            return Vec::new();
        };
        let mut agents: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
            .collect();
        agents.sort();
        agents
            .into_iter()
            .filter_map(|agent| {
                let report = crate::lock_wait::contention(&self.registry, &agent);
                let waiting = report["waiters"]
                    .as_array()
                    .is_some_and(|list| !list.is_empty());
                waiting.then_some((agent, report))
            })
            .collect()
    }

    fn observe(&mut self) -> Result<Vec<Observation>> {
        let started = Instant::now();
        let deadline = self.budget.map(|budget| started + budget);
        self.client.begin_pass(deadline);
        let cloud_timeout = deadline.map_or(PASS_CLOUD_TIMEOUT, |deadline| {
            PASS_CLOUD_TIMEOUT.min(deadline.saturating_duration_since(started))
        });
        let manager = ManagedAgents::new(self.client, &self.registry)?
            .with_project_workspace_resolved()
            .with_cloud_list_timeout(cloud_timeout);
        let mut rows = manager.watch_rows()?;
        // Only a running or starting record that is not paused is checked live; for every
        // other record its row is already all the watcher reads.
        let live: Vec<usize> = (0..rows.len())
            .filter(|&index| {
                let row = &rows[index];
                row["record_error"] != true
                    && row["paused"] != true
                    && matches!(row["lifecycle"].as_str(), Some("running" | "starting"))
            })
            .collect();
        let (cloud, local): (Vec<usize>, Vec<usize>) = live
            .into_iter()
            .partition(|&index| rows[index]["adapter"].as_str() == Some("agentcloud"));
        self.progress(&format!(
            "{} records, {} to check through Herdr, {} agentcloud",
            rows.len(),
            local.len(),
            cloud.len()
        ));
        let proc_view = crate::work_state::PassProc::default();
        let total = local.len();
        let done = std::sync::atomic::AtomicUsize::new(0);
        let (checks, cloud_rows) = {
            let this = &*self;
            let manager = &manager;
            let rows = &rows;
            std::thread::scope(|scope| {
                // agentcloud sessions share one fleet listing, fetched only when a running
                // cloud record needs it, beside the Herdr checks.
                let cloud_thread = scope.spawn(|| {
                    let mut fleet = crate::subagents::FleetCache::new();
                    cloud
                        .iter()
                        .map(|&index| {
                            let name = rows[index]["name"].as_str().unwrap_or_default();
                            (index, manager.watch_status(name, &mut fleet))
                        })
                        .collect::<Vec<_>>()
                });
                let checks = bounded_map(&local, PASS_WORKERS, |&index| {
                    let begun = Instant::now();
                    let checked = this.check(manager, &rows[index], &proc_view, deadline);
                    let count = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    let name = rows[index]["name"].as_str().unwrap_or("?");
                    match &checked {
                        Some(checked) => this.progress(&format!(
                            "[{count}/{total}] {name}: {} in {:.1} s{}",
                            checked.row["agent_status"]
                                .as_str()
                                .unwrap_or("not watched"),
                            begun.elapsed().as_secs_f64(),
                            if checked.reused {
                                ", screen unchanged"
                            } else {
                                ""
                            },
                        )),
                        None => this.progress(&format!(
                            "[{count}/{total}] {name}: not checked, the pass budget ran out"
                        )),
                    }
                    (index, checked)
                });
                (checks, cloud_thread.join().unwrap_or_default())
            })
        };
        for (index, row) in cloud_rows {
            rows[index] = row;
        }
        let mut observations = Vec::new();
        let mut unchecked = Vec::new();
        let mut screens = BTreeMap::new();
        let mut reused = 0;
        for (index, checked) in checks {
            match checked {
                Some(checked) => {
                    reused += usize::from(checked.reused);
                    if let Some(observation) = checked.observation {
                        if let Some(kept) = checked.kept {
                            screens.insert(observation.name.clone(), kept);
                        }
                        observations.push(observation);
                    }
                    rows[index] = checked.row;
                }
                None => unchecked.push(index),
            }
        }
        self.screens = screens;
        // A record this build cannot read is watched as unreachable rather than silently
        // dropped: the coordinator should hear about it.
        for row in &rows {
            // A record that became unreadable while its check ran was observed by that check.
            if row["record_error"] == true
                && row["name"] != self.coordinator.as_str()
                && !observations
                    .iter()
                    .any(|seen| row["name"] == seen.name.as_str())
            {
                let name = row["name"].as_str().unwrap_or_default().to_owned();
                observations.push(self.observation(row.clone(), name, true, None, &proc_view));
            }
        }
        self.progress(&format!(
            "checked {} agents in {:.1} s ({} watched, {} of their screens unchanged since the last pass, {} not checked)",
            total - unchecked.len(),
            started.elapsed().as_secs_f64(),
            observations.len(),
            reused,
            unchecked.len()
        ));
        // When every watched agent is unreachable at once the terminal server, not
        // every harness, is gone; `agentctl watch` reports and repairs that.
        // A partial pass cannot say that: the agents it had no time for may be fine.
        let all_gone = unchecked.is_empty()
            && !observations.is_empty()
            && observations
                .iter()
                .all(|observation| observation.status == "gone");
        if all_gone {
            return Err(fail(
                "no watched agent could be probed; the terminal server may be down",
            ));
        }
        // Notices about an agent go to the nearest agent up its managed_by chain that can take
        // one now; a manager that is paused, unreachable or gone passes them further up.
        let (mut nodes, _, registry) = manager.control_view()?;
        for node in &mut nodes {
            if let Some(row) = rows.iter().find(|row| row["name"] == node.name.as_str()) {
                node.reachable = node.reachable
                    && row["paused"] != true
                    && !(row["agent_status"] == "unknown" && !row["probe_error"].is_null());
            }
        }
        for observation in &mut observations {
            observation.recipient = controller_of(
                &nodes,
                &registry,
                Some(&self.coordinator),
                &observation.name,
            )
            .filter(|recipient| *recipient != self.coordinator);
        }
        // An agent the pass had no time for is named, so the report says the pass was partial.
        for index in unchecked {
            let name = rows[index]["name"].as_str().unwrap_or_default().to_owned();
            if name == self.coordinator {
                continue;
            }
            observations.push(Observation {
                name,
                status: UNCHECKED.to_owned(),
                screen: None,
                has_delivered: false,
                has_pending: false,
                detail: format!(
                    "not checked: the pass's {} s budget ran out",
                    self.budget.map_or(0, |budget| budget.as_secs())
                ),
                recipient: None,
                backlog: crate::turn_state::Backlog::default(),
                work: None,
                progress: None,
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
        self.queue_notice(
            ManagedAgents::new(self.client, &self.registry)?,
            coordinator,
            text,
            message_id,
        )
    }

    fn notify_urgent(&mut self, coordinator: &str, text: &str, message_id: &str) -> Result<String> {
        self.queue_notice(
            ManagedAgents::new(self.client, &self.registry)?.with_lock_priority(),
            coordinator,
            text,
            message_id,
        )
    }

    fn interrupt_and_submit(&mut self, agent: &str) -> Result<String> {
        let report =
            ManagedAgents::new(self.client, &self.registry)?.interrupt_and_submit(agent)?;
        Ok(report["detail"]
            .as_str()
            .unwrap_or("interrupted")
            .to_owned())
    }
}

impl<A: ManagedApi + ?Sized> ManagedNotifier<'_, A> {
    fn queue_notice(
        &self,
        manager: ManagedAgents<'_, A>,
        coordinator: &str,
        text: &str,
        message_id: &str,
    ) -> Result<String> {
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
                // Someone cancelled the notice, or it expired: it is not sent again.
                Ok(Some(QueueMessageState::Cancelled)) => return Ok("cancelled".to_owned()),
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
#[path = "agent_watch_scale_tests.rs"]
mod scale_tests;

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn options() -> WatchAgentsOptions {
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
            delivery_wait: Duration::from_secs(600),
            turn: Duration::from_secs(7200),
            auto_deliver: Vec::new(),
            loop_repeats: 3,
            loop_hold: Duration::from_secs(600),
            step_history: crate::work_state::StepHistory::default(),
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
            recipient: None,
            backlog: crate::turn_state::Backlog::default(),
            work: None,
            progress: None,
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

    /// A probe whose only finding is a scripted wait for the coordinator's lifecycle lock.
    struct JamProbe(Vec<f64>);

    impl Probe for JamProbe {
        fn observe(&mut self) -> Result<Vec<Observation>> {
            Ok(Vec::new())
        }
        fn delivery_waits(&mut self) -> Vec<(String, Value)> {
            let waited = if self.0.is_empty() {
                0.0
            } else {
                self.0.remove(0)
            };
            if waited == 0.0 {
                return Vec::new();
            }
            vec![(
                "coordinator".to_owned(),
                json!({
                    "holder": {"pid": 11, "command": "agentctl send coordinator --file note"},
                    "waiters": [{"pid": 22, "priority": true, "waiting_seconds": waited,
                                 "command": "agentctl chat run"}],
                }),
            )]
        }
    }

    #[test]
    fn a_long_wait_for_an_agents_lock_is_reported_once_per_jam_as_urgent() {
        let options = options();
        let mut state = WatchState::default();
        let mut notifier = RecordingNotifier::default();
        // 5 min (under the 10 min threshold), 11 min, 12 min, then another sender at the head
        // at 10.5 min (the same jam), cleared, then a new jam at 10 min.
        let mut probe = JamProbe(vec![300.0, 660.0, 720.0, 630.0, 0.0, 600.0]);
        let mut pass = |now: f64, state: &mut WatchState, notifier: &mut RecordingNotifier| {
            watch_agents_once(&mut probe, notifier, state, now, &options)
        };
        let report = pass(1_000.0, &mut state, &mut notifier);
        assert_eq!(report["delivery_waits"][0]["agent"], "coordinator");
        assert!(notifier.sent.is_empty());
        let report = pass(1_360.0, &mut state, &mut notifier);
        assert_eq!(report["delivery_waits"][0]["reported"], true);
        assert_eq!(notifier.sent.len(), 1);
        let (to, text, id) = &notifier.sent[0];
        assert_eq!(to, "coordinator");
        assert!(
            text.contains("deliveries to coordinator are waiting"),
            "{text}"
        );
        assert!(text.contains("the longest for 11 min (PID 22: agentctl chat run, priority)"));
        assert!(text.contains("held by PID 11: agentctl send coordinator --file note"));
        assert_eq!(id, "agent-watch-jam-coordinator-1360");
        assert_eq!(notifier.urgent, vec![id.clone()]);
        pass(1_420.0, &mut state, &mut notifier);
        pass(1_480.0, &mut state, &mut notifier);
        assert_eq!(notifier.sent.len(), 1, "the same jam is reported once");
        pass(1_540.0, &mut state, &mut notifier);
        assert!(state.jams_reported.is_empty());
        pass(1_600.0, &mut state, &mut notifier);
        assert_eq!(notifier.sent.len(), 2, "a new jam is reported again");
    }

    #[derive(Default)]
    struct RecordingNotifier {
        sent: Vec<(String, String, String)>,
        attempts: Vec<String>,
        fail_attempts: usize,
        urgent: Vec<String>,
        interrupted: Vec<String>,
    }

    impl Notifier for RecordingNotifier {
        fn interrupt_and_submit(&mut self, agent: &str) -> Result<String> {
            self.interrupted.push(agent.to_owned());
            Ok("interrupted the turn after 46 h 54 m and submitted the 4 queued input(s) it put back in the input box".to_owned())
        }

        fn notify_urgent(
            &mut self,
            coordinator: &str,
            text: &str,
            message_id: &str,
        ) -> Result<String> {
            self.urgent.push(message_id.to_owned());
            self.notify(coordinator, text, message_id)
        }

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
    fn notices_go_to_each_agents_manager_in_one_batch_per_recipient() {
        let mut options = options();
        options.min_notice_interval = Duration::from_secs(600);
        let managed = |name: &str, recipient: Option<&str>| Observation {
            recipient: recipient.map(str::to_owned),
            ..observation(name, "idle", Some(PROMPT_SCREEN))
        };
        let round = || {
            vec![
                managed("worker-a", Some("lieutenant")),
                managed("worker-b", Some("lieutenant")),
                managed("lieutenant", None),
                // A recipient that names the coordinator is the coordinator's batch.
                managed("loner", Some("coordinator")),
            ]
        };
        let mut probe = ScriptedProbe(vec![round(), round(), round()]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.sent.len(), 2, "{due}");
        let to: BTreeMap<&str, &str> = notifier
            .sent
            .iter()
            .map(|(recipient, text, _)| (recipient.as_str(), text.as_str()))
            .collect();
        assert!(to["lieutenant"].contains("worker-a") && to["lieutenant"].contains("worker-b"));
        assert!(!to["lieutenant"].contains("loner"));
        assert!(to["coordinator"].contains("- lieutenant:") && to["coordinator"].contains("loner"));
        assert_ne!(notifier.sent[0].2, notifier.sent[1].2, "distinct batch IDs");
        assert_eq!(due["notice"]["more"][0]["sent"], true, "{due}");
        assert_eq!(state.last_notice_by.get("lieutenant"), Some(&300.0));
        assert_eq!(state.last_notice_at, Some(300.0));
        // A batch for one recipient never carries another's notices, even after a restart.
        let saved: WatchState =
            serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
        assert_eq!(saved.last_notice_by, state.last_notice_by);
    }

    #[test]
    fn a_manager_that_cannot_take_its_batch_does_not_hold_the_notices() {
        let options = options();
        let managed = Observation {
            recipient: Some("lieutenant".to_owned()),
            ..observation("worker", "idle", Some(PROMPT_SCREEN))
        };
        let mut probe = ScriptedProbe(vec![
            vec![managed.clone()],
            vec![managed.clone()],
            vec![managed],
        ]);
        let mut notifier = RecordingNotifier {
            fail_attempts: 1,
            ..RecordingNotifier::default()
        };
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let due = pass(&mut probe, &mut notifier, &mut state, 300.0, &options);
        assert_eq!(notifier.attempts.len(), 2, "{due}");
        assert_eq!(notifier.sent.len(), 1, "{due}");
        assert_eq!(
            notifier.sent[0].0, "coordinator",
            "the coordinator heard instead"
        );
        assert!(state.inflight.is_none() && state.pending.is_empty());
        assert_eq!(due["notice"]["redirected"], true, "{due}");
        // The redirected notice stays with the coordinator; nothing is resent to anyone.
        pass(&mut probe, &mut notifier, &mut state, 400.0, &options);
        assert_eq!(notifier.sent.len(), 1);
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
    fn a_finish_goes_to_the_childs_manager_and_to_the_coordinator_if_it_cannot_take_it() {
        let options = options();
        let finished = |name: &str| Observation {
            recipient: Some("lieutenant".to_owned()),
            ..observation(name, "idle", Some(DONE_SCREEN))
        };
        let mut probe = ScriptedProbe(vec![vec![finished("worker")], vec![finished("other")]]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let done = pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert_eq!(done["child_done"][0]["sent"], true, "{done}");
        assert_eq!(
            notifier.sent[0].0, "lieutenant",
            "the manager hears of its own child"
        );
        notifier.fail_attempts = 1;
        let redirected = pass(&mut probe, &mut notifier, &mut state, 10.0, &options);
        assert_eq!(redirected["child_done"][0]["sent"], true, "{redirected}");
        assert_eq!(notifier.sent[1].0, "coordinator");
        assert!(state.pending.is_empty());
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
            let found = classify(
                &observation("worker", "idle", Some(screen)),
                &options,
                &AgentMemory::default(),
                0.0,
            );
            assert_eq!(
                found.map(|finding| finding.kind),
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

    // ---- #570 agent-progress-monitoring: what "working" means -------------------------------

    use crate::work_state::fake::FakeProc;
    use crate::work_state::{sample, WorkSample};

    const CODEX_LONG: &str = include_str!("../testdata/harness-turns/codex-long-turn-queued.txt");
    const CODEX_AFTER: &str = include_str!("../testdata/harness-turns/codex-after-interrupt.txt");

    /// The one batched notice sent so far, as text.
    fn only_notice(notifier: &RecordingNotifier) -> &str {
        assert_eq!(notifier.sent.len(), 1, "{:?}", notifier.sent);
        &notifier.sent[0].1
    }

    fn working(name: &str, screen: &str, work: Option<WorkSample>) -> Observation {
        Observation {
            work,
            ..observation(name, "working", Some(screen))
        }
    }

    #[test]
    fn a_46_hour_turn_with_queued_input_is_reported_with_its_age_and_queue() {
        let options = options();
        let mut probe = ScriptedProbe(vec![
            vec![working("lane-a", CODEX_AFTER, None)],
            vec![working("lane-a", CODEX_LONG, None)],
            vec![working("lane-a", CODEX_LONG, None)],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        // A ten-minute turn with nothing queued is healthy.
        let young = pass(&mut probe, &mut notifier, &mut state, 1_000.0, &options);
        assert_ne!(young["episodes"][0]["kind"], "turn-queued", "{young}");
        let report = pass(&mut probe, &mut notifier, &mut state, 1_060.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "turn-queued", "{report}");
        assert!(only_notice(&notifier).contains(
            "- lane-a: inside one turn for 46 h 54 m; 4 inputs queued in Codex's queued follow-up inputs, submitted only when the turn ends"
        ));
        // The timer moves on; it is the same episode, reported once.
        pass(&mut probe, &mut notifier, &mut state, 1_120.0, &options);
        assert_eq!(notifier.sent.len(), 1);
        assert!(notifier.interrupted.is_empty(), "report only by default");
    }

    #[test]
    fn auto_deliver_interrupts_a_named_agents_long_turn_and_says_so() {
        let mut options = options();
        options.auto_deliver = vec!["lane-a".to_owned()];
        let mut probe = ScriptedProbe(vec![
            vec![
                working("lane-a", CODEX_LONG, None),
                working("lane-b", CODEX_LONG, None),
            ],
            vec![
                working("lane-a", CODEX_LONG, None),
                working("lane-b", CODEX_LONG, None),
            ],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert_eq!(notifier.interrupted, ["lane-a"], "only the named agent");
        let text = only_notice(&notifier);
        assert!(
            text.contains("- lane-a: inside one turn for 46 h 54 m;"),
            "{text}"
        );
        assert!(
            text.contains("; auto-delivered: interrupted the turn after 46 h 54 m"),
            "{text}"
        );
        assert!(!text.contains("- lane-b: inside one turn for 46 h 54 m; 4 inputs queued in Codex's queued follow-up inputs, submitted only when the turn ends; auto"));
        pass(&mut probe, &mut notifier, &mut state, 60.0, &options);
        assert_eq!(notifier.interrupted, ["lane-a"], "once per episode");
    }

    #[test]
    fn an_auto_delivery_is_reported_even_after_the_turn_it_ended() {
        let mut options = options();
        options.auto_deliver = vec!["lane-a".to_owned()];
        options.min_notice_interval = Duration::from_secs(600);
        let mut probe = ScriptedProbe(vec![
            vec![working("lane-a", CODEX_LONG, None)],
            vec![working("lane-a", CODEX_AFTER, None)],
            vec![working("lane-a", CODEX_AFTER, None)],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState {
            last_notice_at: Some(0.0),
            ..WatchState::default()
        };
        pass(&mut probe, &mut notifier, &mut state, 60.0, &options);
        assert_eq!(notifier.interrupted, ["lane-a"]);
        assert!(notifier.sent.is_empty(), "held by the rate limit");
        // The interrupt ended the turn; the notice saying so is not withdrawn with it.
        pass(&mut probe, &mut notifier, &mut state, 120.0, &options);
        assert_eq!(state.pending.len(), 1);
        pass(&mut probe, &mut notifier, &mut state, 660.0, &options);
        assert!(only_notice(&notifier).contains("auto-delivered: interrupted the turn"));
    }

    #[test]
    fn hourly_activity_is_not_progress_when_the_declared_metric_stands_still() {
        let options = options();
        let reading = |value: &str| crate::progress_metric::Reading {
            value: Some(value.to_owned()),
            error: None,
            window_seconds: 6.0 * 3600.0,
            source: "/work/status.md".to_owned(),
        };
        let lane = |hour: u32, value: &str| Observation {
            progress: Some(reading(value)),
            // A new status line every hour: the screen keeps changing.
            ..observation(
                "lane-a",
                "working",
                Some(&format!("• Ran echo heartbeat {hour}\n")),
            )
        };
        let mut probe = ScriptedProbe(
            (0..8)
                .map(|hour| vec![lane(hour, "crashes recorded: 0")])
                .chain([vec![lane(8, "crashes recorded: 1")]])
                .collect(),
        );
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        for hour in 0..6 {
            pass(
                &mut probe,
                &mut notifier,
                &mut state,
                f64::from(hour) * 3600.0,
                &options,
            );
        }
        assert!(notifier.sent.is_empty(), "five hours is inside the window");
        let report = pass(
            &mut probe,
            &mut notifier,
            &mut state,
            6.0 * 3600.0,
            &options,
        );
        assert_eq!(report["episodes"][0]["kind"], "no-progress", "{report}");
        assert!(only_notice(&notifier).contains(
            "- lane-a: no progress for 6 h 0 m: /work/status.md unchanged while it works (\"crashes recorded: 0\")"
        ));
        pass(
            &mut probe,
            &mut notifier,
            &mut state,
            7.0 * 3600.0,
            &options,
        );
        let moved = pass(
            &mut probe,
            &mut notifier,
            &mut state,
            8.0 * 3600.0,
            &options,
        );
        assert_ne!(
            moved["episodes"][0]["kind"], "no-progress",
            "the metric moved: {moved}"
        );
    }

    /// A harness (PID 100) running `cargo build` (PID 201, under shell 200) that sleeps on a
    /// file lock held by `cargo test` (PID 900).
    fn lock_waiter() -> FakeProc {
        let mut proc = FakeProc::default();
        proc.spawn(100, 1, 'S', "claude")
            .spawn(200, 100, 'S', "/bin/bash -c cargo build --release")
            .spawn(201, 200, 'S', "cargo build --release")
            .sleep_in(200, "do_wait")
            .sleep_in(201, "locks_lock_inode_wait")
            .open(201, 3, "/work/target/release/.cargo-lock", Some("00:2e:77"))
            .spawn(900, 1, 'S', "cargo test --workspace");
        proc.locks = "7: FLOCK  ADVISORY  WRITE 900 00:2e:77 0 EOF\n7: -> FLOCK  ADVISORY  WRITE 201 00:2e:77 0 EOF\n".to_owned();
        proc
    }

    const STILL_SCREEN: &str = "● Bash(cargo build --release)\n  ⎿  Running…\n\n✻ Baking… (3m 2s · ↓ 1.2k tokens · esc to interrupt)\n";

    #[test]
    fn a_tool_blocked_on_a_lock_names_the_holder_and_a_progressing_tool_is_healthy() {
        let options = options();
        let blocked = sample(&lock_waiter(), 100, None);
        let mut burning = lock_waiter();
        burning.processes.iter_mut().for_each(|row| {
            if row.pid == 201 {
                row.state = 'R';
            }
        });
        let mut later = burning.clone();
        later.burn(201, 900);
        let mut probe = ScriptedProbe(vec![
            vec![working("lane-a", STILL_SCREEN, blocked.clone())],
            vec![working(
                "lane-a",
                &STILL_SCREEN.replace("3m 2s", "34m 40s"),
                blocked,
            )],
            vec![working("busy", STILL_SCREEN, sample(&burning, 100, None))],
            vec![working("busy", STILL_SCREEN, sample(&later, 100, None))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        // Only the turn timer moved in 31 minutes: that is not output.
        let report = pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "tool-blocked", "{report}");
        assert_eq!(
            only_notice(&notifier).lines().nth(1).unwrap(),
            "- lane-a: tool blocked 31 min: `cargo build --release` (PID 201) waits on a file lock on /work/target/release/.cargo-lock held by `cargo test --workspace` (PID 900)"
        );
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let healthy = pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert!(
            !healthy["episodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["agent"] == "busy"),
            "a tool using CPU is progress even when the screen is still: {healthy}"
        );
    }

    #[test]
    fn a_gate_step_blocked_on_a_lock_is_named_while_its_siblings_run() {
        let options = options();
        let gate = |sibling_ticks: u64| {
            let mut proc = FakeProc::default();
            proc.spawn(100, 1, 'S', "codex")
                .spawn(200, 100, 'S', "bash -lc python3 scripts/validate.py")
                .spawn(201, 200, 'S', "dagrun run --dag validation.dag.yaml")
                .open(201, 9, "/tmp/gate/journal.jsonl", None)
                .spawn(300, 201, 'S', "cargo test -p agentctl")
                .sleep_in(300, "locks_lock_inode_wait")
                .open(
                    300,
                    4,
                    "/work/rs/target/debug/.cargo-lock",
                    Some("00:2e:91"),
                )
                .spawn(301, 201, 'R', "pytest py/tests")
                .burn(301, sibling_ticks)
                .spawn(900, 1, 'S', "cargo build --release");
            proc.locks = "3: FLOCK  ADVISORY  WRITE 900 00:2e:91 0 EOF\n3: -> FLOCK  ADVISORY  WRITE 300 00:2e:91 0 EOF\n".to_owned();
            proc.files.insert(
                "/tmp/gate/journal.jsonl".to_owned(),
                [
                    r#"{"ts":"100.0","event":"step_start","step":"rust-agentctl","pid":"300","cmd":"cargo test -p agentctl"}"#,
                    r#"{"ts":"100.0","event":"step_start","step":"py-unit","pid":"301","cmd":"pytest py/tests"}"#,
                ]
                .join("\n"),
            );
            sample(&proc, 100, None)
        };
        let mut probe = ScriptedProbe(vec![
            vec![working(
                "lane-a",
                "• Working (1m 0s • esc to interrupt)\n",
                gate(10),
            )],
            vec![working(
                "lane-a",
                "• Working (41m 0s • esc to interrupt)\n",
                gate(9000),
            )],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 100.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 2_500.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "tool-blocked", "{report}");
        assert_eq!(
            only_notice(&notifier).lines().nth(1).unwrap(),
            "- lane-a: gate step rust-agentctl blocked 40 min: `cargo test -p agentctl` (PID 300) waits on a file lock on /work/rs/target/debug/.cargo-lock held by `cargo build --release` (PID 900)"
        );
    }

    #[test]
    fn a_gate_step_past_its_recorded_p95_is_reported() {
        let mut options = options();
        let mut csv = "step,elapsed_s\n".to_owned();
        for seconds in [1500, 1600, 1700, 1800, 1860] {
            csv.push_str(&format!("rust-agentctl,{seconds}\n"));
        }
        options.step_history.add_csv(&csv);
        let mut proc = FakeProc::default();
        proc.spawn(100, 1, 'S', "codex")
            .spawn(200, 100, 'S', "bash -lc dagrun run")
            .open(200, 9, "/tmp/gate/journal.jsonl", None)
            .spawn(300, 200, 'R', "cargo test -p agentctl");
        proc.files.insert(
            "/tmp/gate/journal.jsonl".to_owned(),
            r#"{"ts":"1000.0","event":"step_start","step":"rust-agentctl","pid":"300","cmd":"cargo test"}"#.to_owned(),
        );
        let mut probe = ScriptedProbe(vec![vec![working(
            "lane-a",
            "• Working (50m 0s • esc to interrupt)\n",
            sample(&proc, 100, None),
        )]]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        let report = pass(&mut probe, &mut notifier, &mut state, 4_000.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "over-expected", "{report}");
        assert!(only_notice(&notifier).contains(
            "- lane-a: gate step rust-agentctl running 50 min, past its p95 of 31 min over 5 recorded runs"
        ));
    }

    #[test]
    fn a_silent_stream_with_no_tool_is_reported_and_moving_tokens_are_not() {
        let options = options();
        let mut proc = FakeProc::default();
        proc.spawn(100, 1, 'S', "claude")
            .spawn(150, 100, 'S', "node /opt/mcp/server.js");
        let idle_harness = sample(&proc, 100, None);
        assert_eq!(
            idle_harness.as_ref().unwrap().items,
            vec![],
            "an MCP server is not a tool"
        );
        let thinking = |timer: &str, tokens: &str| {
            format!("● Looking into it.\n\n✻ Pondering… ({timer} · ↓ {tokens} tokens · esc to interrupt)\n")
        };
        let mut probe = ScriptedProbe(vec![
            vec![working(
                "silent",
                &thinking("1m 0s", "2.1k"),
                idle_harness.clone(),
            )],
            vec![working(
                "silent",
                &thinking("32m 0s", "2.1k"),
                idle_harness.clone(),
            )],
            vec![working(
                "streaming",
                &thinking("1m 0s", "2.1k"),
                idle_harness.clone(),
            )],
            vec![working(
                "streaming",
                &thinking("32m 0s", "9.8k"),
                idle_harness,
            )],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "stream-silent", "{report}");
        assert_eq!(
            only_notice(&notifier).lines().nth(1).unwrap(),
            "- silent: model stream silent 31 min: the transcript and the token counter have not moved and no tool is running"
        );
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let streaming = pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert!(!streaming["episodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["agent"] == "streaming"));
    }

    #[test]
    fn loops_and_retry_banners_are_reported_once_they_hold() {
        let options = options();
        let repeated = "• Ran cargo test -p kiln\n  └ error: 1 failed\n• Ran cargo test -p kiln\n  └ error: 1 failed\n• Ran cargo test -p kiln\n  └ error: 1 failed\n\n• Working (5m 0s • esc to interrupt)\n";
        let retrying = "● Thinking about it\n  ⎿  API Error (overloaded) · Retrying in 8 seconds… (attempt 4/10)\n\n✻ Pondering… (2m 0s · esc to interrupt)\n";
        let errors = "● Bash(make a)\n  ⎿  Error: connection refused\n● Edit(config.toml)\n● Bash(make b)\n  ⎿  Error: connection refused\n● Bash(make c)\n  ⎿  Error: connection refused\n";
        assert_eq!(
            repeated_activity(repeated, 3).unwrap(),
            "the same command ran 3 times in a row: cargo test -p kiln"
        );
        assert!(repeated_activity(retrying, 3)
            .unwrap()
            .starts_with("retry banner: ⎿  API Error (overloaded) · Retrying in 8 seconds"));
        assert_eq!(
            repeated_activity(errors, 3).unwrap(),
            "the same error in 3 attempts: Error: connection refused"
        );
        // One compiler run that prints one error for three places is one attempt.
        let one_run = "● Bash(cargo build)\n  ⎿  error[E0308]: mismatched types\n     error[E0308]: mismatched types\n     error[E0308]: mismatched types\n";
        assert_eq!(repeated_activity(one_run, 3), None);
        assert_eq!(repeated_activity("• Ran a\n• Ran b\n• Ran a\n", 3), None);
        let fix_and_test = "• Ran cargo test\n• Edited src/lib.rs (+2 -1)\n• Ran cargo test\n• Edited src/lib.rs (+1 -1)\n• Ran cargo test\n";
        assert_eq!(
            repeated_activity(fix_and_test, 3),
            None,
            "edits between runs are progress"
        );
        assert_eq!(
            repeated_activity("  └ reconnecting to the database (attempt 2/5)\n", 3),
            None,
            "tool output is not a harness banner"
        );
        assert!(repeated_activity("⚠ Reconnecting... 2/5\n", 3).is_some());
        let mut probe = ScriptedProbe(vec![
            vec![working("lane-a", repeated, None)],
            vec![working("lane-a", repeated, None)],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        assert!(
            notifier.sent.is_empty(),
            "a loop is given --loop-minutes to clear"
        );
        pass(&mut probe, &mut notifier, &mut state, 600.0, &options);
        assert!(only_notice(&notifier)
            .contains("- lane-a: the same command ran 3 times in a row: cargo test -p kiln"));
    }

    #[test]
    fn waiting_on_a_dead_or_idle_background_shell_is_reported_and_a_running_one_is_not() {
        let options = options();
        let footer = "● Started the gate in the background.\n\n❯\u{a0}\n\n  ⏵⏵ auto mode on · 1 shell · ↓ to manage\n";
        let idle = |work: Option<WorkSample>| Observation {
            work,
            ..observation("lane-a", "idle", Some(footer))
        };
        let mut gone = FakeProc::default();
        gone.spawn(100, 1, 'S', "claude");
        let mut stuck = FakeProc::default();
        stuck
            .spawn(100, 1, 'S', "claude")
            .spawn(200, 100, 'S', "/bin/bash -c python3 scripts/validate.py")
            .sleep_in(200, "pipe_read");
        let mut running = stuck.clone();
        running.processes[1].state = 'R';
        let mut running_later = running.clone();
        running_later.burn(200, 50);
        let mut probe = ScriptedProbe(vec![
            vec![idle(sample(&gone, 100, None))],
            vec![idle(sample(&gone, 100, None))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let report = pass(&mut probe, &mut notifier, &mut state, 900.0, &options);
        assert_eq!(report["episodes"][0]["kind"], "background-wait", "{report}");
        assert_eq!(report["episodes"][0]["due"], true, "{report}");
        assert!(only_notice(&notifier).contains(
            "- lane-a: status idle waiting on background work the screen counts (1 shell), but no shell runs under its harness"
        ));

        let mut probe = ScriptedProbe(vec![
            vec![idle(sample(&stuck, 100, None))],
            vec![idle(sample(&stuck, 100, None))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert!(only_notice(&notifier).contains(
            "- lane-a: status idle waiting on background work (1 shell) that is idle: tool blocked 31 min: `/bin/bash -c python3 scripts/validate.py` (PID 200) waits on a pipe"
        ));

        let mut probe = ScriptedProbe(vec![
            vec![idle(sample(&running, 100, None))],
            vec![idle(sample(&running_later, 100, None))],
        ]);
        let mut notifier = RecordingNotifier::default();
        let mut state = WatchState::default();
        pass(&mut probe, &mut notifier, &mut state, 0.0, &options);
        let healthy = pass(&mut probe, &mut notifier, &mut state, 1_860.0, &options);
        assert_eq!(healthy["episodes"], json!([]), "{healthy}");
    }
}
