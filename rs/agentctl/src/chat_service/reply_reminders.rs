//! When `chat run` reminds the coordinator of a delivered chat request it has not
//! answered (#292 reply-in-thinking-block).
//!
//! A check runs at each scan of the request records (every `DeliveryTiming::retry_interval`).
//! It finds the requests that may be reminded (see `chat_runtime::ReminderCandidates`) and, while
//! there are any, follows the coordinator's idle time: Herdr must report the pane `idle` or
//! `done`, none of the loop's own evidence may show a running turn, and Claude Code itself, asked
//! through `claude agents --json` exactly as `agentctl inbox watch` asks
//! (`crate::inbox::claude_sessions_by_pane`), must say the session in the pane is `idle`. Herdr's
//! word alone is not enough: its screen rules can report a working Claude Code pane as idle. An
//! idle period starts at the first check that finds all three and ends at the first that does
//! not. A request is due once the coordinator has been idle for the configured time since both
//! the start of that period and its prompt was typed. After a run of `claude agents --json` that
//! fails, lists no session in the pane, or is slow, Claude Code is not asked again for one idle
//! time, except right before a reminder.
//!
//! Before reminding, the check reads the pane and captures it as a recovery scan does, so a reply
//! on screen that no capture stored yet counts, and it reminds no one if that capture may have
//! typed or left a prompt for the coordinator. Herdr and Claude Code are then asked once more.
//! Last, right before the reminder is composed, it reads the transcript of the Claude Code session
//! that answer named (`<projects>/<session>.jsonl`, the file `agentctl inbox watch` reads) and
//! leaves out each request whose reply ID heads a complete reply block in the session's ordinary
//! text: that reply was printed, though no read of the pane has shown it yet, as when the pane's
//! view is scrolled away from its end, and a reminder saying no reply arrived would only make the
//! coordinator send it a second time once the pane does show it (#260 reminder-capture-race). A
//! transcript that cannot be found or read leaves the reminder as it was, and is logged.
//! The reminder is recorded before it is queued and is typed only through the queue's normal
//! checks, never into a working pane nor behind another prompt; one the queue does not type is
//! taken back out of it. That holds off the next reminder until the coordinator has been seen
//! working, by Herdr's status or a screen, and idle again, or, if it is not seen working, until a
//! retry delay has passed: twice the idle time after the first such reminder in a row, doubling
//! with each after it, and at most `MAX_REPLY_REMINDER_RETRY`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::agent::DrainOptions;
use crate::chat_runtime::{
    self, BridgeState, PendingReminder, ReminderCandidate, ReminderOutcome, ReplyReminderRecord,
    SNAPSHOT_LINES,
};
use crate::client::AgentPaneInfo;
use crate::subagents::{ManagedAgents, ManagedApi};

use super::{
    capture_recovery_snapshot, report_receipt_errors, service_log, CancellableDelivery,
    ChatServiceError, CycleReport, PassControl, RouteCache, SnapshotInput, StopRuntime, StopState,
    TurnEvidence, TypingNotes, TypingWatch,
};

/// The idle time after which `chat run` reminds a Claude Code coordinator of a delivered request
/// it has not answered, unless `--reply-reminder-idle` says otherwise.
pub const DEFAULT_REPLY_REMINDER_IDLE: Duration = Duration::from_secs(60);
/// The shortest `--reply-reminder-idle` other than 0, which turns reminders off.
pub const MIN_REPLY_REMINDER_IDLE: Duration = Duration::from_secs(10);
/// The longest `--reply-reminder-idle`. A request is reminded only while its prompt was typed at
/// most `chat_runtime::MAX_REPLY_REMINDER_AGE` (24 hours) ago, and only once the coordinator has
/// been idle for the idle time after any work it did on it, so the idle time and that work must
/// together stay within that age. Half of it leaves the other half for the work.
pub const MAX_REPLY_REMINDER_IDLE: Duration = Duration::from_secs(12 * 60 * 60);
const _: () = assert!(
    MAX_REPLY_REMINDER_IDLE.as_secs() * 2 <= chat_runtime::MAX_REPLY_REMINDER_AGE.as_secs()
);
/// The longest a reminder that was taken back out of the queue, or never queued, holds off the
/// next while the coordinator is not seen working.
pub const MAX_REPLY_REMINDER_RETRY: Duration = Duration::from_secs(60 * 60);
/// How long one `claude agents --json` may run before it is killed and the check gives up.
const CLAUDE_QUERY_TIMEOUT: Duration = Duration::from_secs(10);
/// A run of `claude agents --json` that takes longer than this is slow: its answer counts, but
/// Claude Code is not asked again for one idle time, except right before a reminder, so a slow
/// host does not spend most of each scan waiting for it.
const CLAUDE_SLOW_ANSWER: Duration = Duration::from_secs(3);
/// Most bytes at the end of a Claude Code transcript read for reply blocks before a reminder, as
/// many as `agentctl inbox watch` reads for an agent's last message.
const TRANSCRIPT_TAIL_BYTES: u64 = 8 * 1024 * 1024;

/// How `chat run` reminds a Claude Code coordinator of the requests it has not answered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplyReminderSettings {
    /// How long the coordinator must have been idle, after a request's prompt was typed, before
    /// it is reminded of that request.
    pub idle: Duration,
    /// The Claude Code executable whose `agents --json` tells whether the coordinator is idle.
    pub claude_bin: PathBuf,
}

impl Default for ReplyReminderSettings {
    fn default() -> Self {
        Self {
            idle: DEFAULT_REPLY_REMINDER_IDLE,
            claude_bin: PathBuf::from("claude"),
        }
    }
}

/// Parse `--reply-reminder-idle`: a whole number of seconds (`s`, or no unit), minutes (`m`) or
/// hours (`h`); `0` turns reminders off, and anything else must be at least
/// `MIN_REPLY_REMINDER_IDLE` and at most `MAX_REPLY_REMINDER_IDLE`.
///
/// # Errors
///
/// A value of another form or outside those bounds, with the reason.
pub fn parse_idle(value: &str) -> Result<Option<Duration>, String> {
    let value = value.trim();
    let (number, unit) = match value.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((index, _)) => value.split_at(index),
        None => (value, "s"),
    };
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        _ => return Err("expected a whole number with unit s, m or h, such as 60s".to_owned()),
    };
    let count: u64 = number
        .parse()
        .map_err(|_| "expected a whole number with unit s, m or h, such as 60s".to_owned())?;
    if count == 0 {
        return Ok(None);
    }
    let idle = Duration::from_secs(count.saturating_mul(multiplier));
    if idle < MIN_REPLY_REMINDER_IDLE || idle > MAX_REPLY_REMINDER_IDLE {
        return Err(format!(
            "the idle time must be 0, which turns reply reminders off, or at least {}s and at \
most {}h",
            MIN_REPLY_REMINDER_IDLE.as_secs(),
            MAX_REPLY_REMINDER_IDLE.as_secs() / 3_600
        ));
    }
    Ok(Some(idle))
}

/// The display name of the harness Herdr reports in the pane `info` describes, such as
/// `Muse` for a Muse pane: the name reminder text uses for the coordinator's harness,
/// never a hardcoded one.
pub(crate) fn harness_display_name(info: &AgentPaneInfo) -> String {
    match info.agent.as_deref() {
        Some(kind) => crate::harness::adapter(kind).map_or_else(
            || kind.to_owned(),
            |adapter| adapter.display_name().to_owned(),
        ),
        None => "an unrecognized harness".to_owned(),
    }
}

/// What the coordinator's harness says about the session in its pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeState {
    /// The session waits at its prompt.
    Idle,
    /// Any other state the harness reports, such as `busy`.
    Busy(String),
    /// Herdr does not report the pane as running a harness whose idleness reminders can ask
    /// about (a Claude Code pane is asked through its own CLI; a known harness's pane is
    /// read from its Herdr status).
    NotClaude,
    /// Claude Code lists no session in the pane.
    NoSession,
}

/// Where a check learns whether Claude Code in the coordinator's pane is idle.
pub(crate) trait ClaudeIdleSource {
    /// The state of the Claude Code session in the pane `info` describes, or why Claude Code
    /// could not be asked. Asking gives up as soon as `cancelled` says so.
    fn state(
        &self,
        info: &AgentPaneInfo,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ClaudeState, String>;

    /// The reply IDs among `reply_ids` that head a complete reply block in the transcript of the
    /// Claude Code session that the last [`Self::state`] found in the pane `info` describes: see
    /// [`printed_reply_ids`]. Empty for a source that keeps no transcript.
    fn printed_replies(
        &self,
        info: &AgentPaneInfo,
        reply_ids: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String> {
        let _ = (info, reply_ids);
        Ok(BTreeSet::new())
    }
}

/// The Claude Code session the last question found in a pane.
#[derive(Clone, Debug)]
struct FoundSession {
    pane_id: String,
    session_id: String,
    pid: String,
}

/// `claude agents --json`, joined to Herdr panes as `agentctl inbox watch` joins it.
struct ClaudeAgents {
    claude: PathBuf,
    proc_root: PathBuf,
    // The session the last answer named for the pane it was asked about, whose transcript
    // `printed_replies` reads.
    found: Mutex<Option<FoundSession>>,
}

impl ClaudeAgents {
    fn new(claude: PathBuf, proc_root: PathBuf) -> Self {
        Self {
            claude,
            proc_root,
            found: Mutex::new(None),
        }
    }
}

/// Where the Claude Code process `pid` keeps its transcripts: `projects` in its
/// `CLAUDE_CONFIG_DIR`, or else in `.claude` in its home directory, or else in `.claude` in this
/// process's home directory, as `agentctl inbox watch` assumes.
fn claude_projects(proc_root: &Path, pid: &str) -> PathBuf {
    if let Some(directory) = crate::inbox::environment_value(proc_root, pid, "CLAUDE_CONFIG_DIR")
        .filter(|directory| !directory.is_empty())
    {
        return PathBuf::from(directory).join("projects");
    }
    crate::inbox::environment_value(proc_root, pid, "HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .unwrap_or_default()
        .join(".claude")
        .join("projects")
}

/// The reply IDs among `reply_ids` that head a complete reply block in the ordinary text of the
/// assistant messages in the last `TRANSCRIPT_TAIL_BYTES` of the Claude Code transcript at `path`:
/// an opening marker line `<CHAT_REPLY_ID>` and, after at least one other nonblank line, the
/// closing line `</CHAT_REPLY_ID>`, each alone on its line apart from surrounding blanks and
/// outside code fences, in one message. Thinking and tool calls are not ordinary text. A line cut
/// by the byte limit is skipped, since it does not parse.
pub(crate) fn printed_reply_ids(
    path: &Path,
    reply_ids: &BTreeSet<String>,
) -> io::Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    if reply_ids.is_empty() {
        return Ok(found);
    }
    let mut file = fs::File::open(path)?;
    let length = file.metadata()?.len();
    let from = length.saturating_sub(TRANSCRIPT_TAIL_BYTES);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(length - from).read_to_end(&mut bytes)?;
    for line in bytes.split(|byte| *byte == b'\n') {
        // Most lines name no marker, and are not parsed.
        if !std::str::from_utf8(line).is_ok_and(|line| line.contains("CHAT_REPLY_")) {
            continue;
        }
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
        for text in content
            .iter()
            .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        {
            complete_blocks(text, reply_ids, &mut found);
        }
    }
    Ok(found)
}

/// Add to `found` each of `reply_ids` that heads a complete reply block in `text`, as
/// [`printed_reply_ids`] describes.
fn complete_blocks(text: &str, reply_ids: &BTreeSet<String>, found: &mut BTreeSet<String>) {
    let mut fence: Option<(char, usize)> = None;
    // The reply ID of the block open at this line, and whether it has a nonblank line yet.
    let mut open: Option<(&str, bool)> = None;
    for line in text.lines() {
        let line = line.trim();
        let run = |character: char| line.chars().take_while(|value| *value == character).count();
        if let Some((character, length)) = fence {
            if run(character) >= length && line.trim_start_matches(character).is_empty() {
                fence = None;
            }
            continue;
        }
        if let Some(character) = line
            .chars()
            .next()
            .filter(|value| matches!(value, '`' | '~'))
        {
            let length = run(character);
            let info = &line[length..];
            if length >= 3 && !(character == '`' && info.contains('`')) {
                fence = Some((character, length));
                open = None;
                continue;
            }
        }
        if let Some(identifier) = line
            .strip_prefix("<CHAT_REPLY_")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            open = reply_ids
                .get(identifier)
                .map(|identifier| (identifier.as_str(), false));
        } else if let Some(identifier) = line
            .strip_prefix("</CHAT_REPLY_")
            .and_then(|rest| rest.strip_suffix('>'))
        {
            if let Some((opened, true)) = open {
                if opened == identifier {
                    found.insert(opened.to_owned());
                }
            }
            open = None;
        } else if !line.is_empty() {
            if let Some((_, body)) = open.as_mut() {
                *body = true;
            }
        }
    }
}

impl ClaudeIdleSource for ClaudeAgents {
    fn state(
        &self,
        info: &AgentPaneInfo,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ClaudeState, String> {
        if info.agent.as_deref() != Some("claude") {
            // Another known harness (Codex, Muse) has no `agents --json` to ask: its pane's
            // Herdr status, which the caller has already required to read idle or done and
            // to show no running turn, is its answer. An unknown kind cannot be asked.
            return Ok(
                match info.agent.as_deref().and_then(crate::harness::adapter) {
                    Some(_) if matches!(info.status.as_str(), "idle" | "done") => {
                        ClaudeState::Idle
                    }
                    Some(_) => ClaudeState::Busy(info.status.clone()),
                    None => ClaudeState::NotClaude,
                },
            );
        }
        let sessions = crate::inbox::claude_sessions_by_pane(
            &self.claude,
            &self.proc_root,
            CLAUDE_QUERY_TIMEOUT,
            cancelled,
        )
        .map_err(|error| error.to_string())?;
        let session = sessions.get(&info.pane_id);
        *self
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = session.and_then(|session| {
            Some(FoundSession {
                pane_id: info.pane_id.clone(),
                session_id: session.session_id()?.to_owned(),
                pid: session.pid()?,
            })
        });
        Ok(match session.and_then(|session| session.status()) {
            Some("idle") => ClaudeState::Idle,
            Some(other) => ClaudeState::Busy(other.to_owned()),
            None => ClaudeState::NoSession,
        })
    }

    fn printed_replies(
        &self,
        info: &AgentPaneInfo,
        reply_ids: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, String> {
        if info.agent.as_deref() != Some("claude") {
            // Only a Claude Code transcript is read for printed replies; another harness's
            // pane is captured by the check itself, so there is nothing more to leave out.
            return Ok(BTreeSet::new());
        }
        let found = self
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .filter(|found| found.pane_id == info.pane_id)
            .ok_or_else(|| {
                format!(
                    "`{} agents --json` named no session ID and process for the pane {}",
                    self.claude.display(),
                    info.pane_id
                )
            })?;
        let projects = claude_projects(&self.proc_root, &found.pid);
        let path =
            crate::inbox::transcript_of_session(&projects, &found.session_id).ok_or_else(|| {
                format!(
                    "no transcript {}.jsonl is in any project under {}",
                    found.session_id,
                    projects.display()
                )
            })?;
        printed_reply_ids(&path, reply_ids)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))
    }
}

/// The idle period a check has followed: when the first check of an unbroken run that found the
/// coordinator idle saw it so.
#[derive(Debug, Default)]
struct IdlePeriod {
    since_millis: Option<u64>,
}

impl IdlePeriod {
    /// The coordinator was found not idle, or could not be found idle: the period ends.
    fn end(&mut self) {
        *self = Self::default();
    }

    /// The coordinator was found idle at `now_millis`: when its idle period began.
    fn idle(&mut self, now_millis: u64) -> u64 {
        *self.since_millis.get_or_insert(now_millis)
    }
}

/// What holds off the next reminder after one was taken back out of the queue, or never queued.
#[derive(Debug)]
struct Hold {
    /// `TurnEvidence::seen_working` when the reminder was taken back: a later value says the
    /// coordinator has been seen working since.
    seen_working: u64,
    /// When the hold ends even though the coordinator was not seen working.
    until: Instant,
}

/// Which of a check's questions to Claude Code is being asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ask {
    /// The check's first question, which waits after a failed, sessionless or slow answer.
    First,
    /// The question right before a reminder, which is always asked.
    Again,
}

/// How long a reminder taken back out of the queue, or never queued, holds off the next while
/// the coordinator is not seen working, when it is the `withdrawals`th such reminder in a row:
/// `first` for the first, doubling with each after it, and at most `max`.
pub(crate) fn retry_delay(first: Duration, withdrawals: u32, max: Duration) -> Duration {
    let factor = 1_u32
        .checked_shl(withdrawals.saturating_sub(1).min(31))
        .unwrap_or(u32::MAX);
    first.saturating_mul(factor).min(max)
}

/// The candidates of `eligible` due a reminder at `now_millis`, when the coordinator has been
/// idle since `idle_since_millis`: idle for `idle` since both that moment and when the request's
/// prompt was typed. A prompt typed during the idle period started a turn that ended before the
/// next check, so the coordinator has been idle only since some time after it was typed.
pub(crate) fn due_requests(
    eligible: &[ReminderCandidate],
    idle_since_millis: u64,
    now_millis: u64,
    idle: Duration,
) -> Vec<ReminderCandidate> {
    let idle = u64::try_from(idle.as_millis()).unwrap_or(u64::MAX);
    eligible
        .iter()
        .filter(|candidate| {
            now_millis.saturating_sub(idle_since_millis.max(candidate.delivered_at_millis)) >= idle
        })
        .cloned()
        .collect()
}

/// What one check did, for the owner loop: its passes' report, and the pane it read, if any.
#[derive(Default)]
pub(super) struct ReminderCheck {
    pub(super) report: CycleReport,
    pub(super) snapshot: Option<String>,
}

/// Where settling a recorded reminder left the record.
enum Settled {
    /// No reminder is left waiting in the queue: the record as it now stands.
    Record(ReplyReminderRecord),
    /// A reminder is still in the queue, which could not give it back.
    Waiting,
    /// The record could not be read or written.
    Unknown,
}

/// The reply reminders of one `chat run`.
pub(crate) struct ReplyReminders {
    idle: Duration,
    source: Box<dyn ClaudeIdleSource>,
    period: IdlePeriod,
    // What holds off the next reminder, after one was taken back out of the queue.
    hold: Option<Hold>,
    // How many reminders in a row were taken back out of the queue, or never queued, with no
    // reminder typed and the coordinator not seen working in between.
    withdrawals: u32,
    // The hold after the first such reminder in a row, which doubles with each after it, and its
    // bound: twice the idle time and `MAX_REPLY_REMINDER_RETRY`, except in tests.
    retry_first: Duration,
    retry_max: Duration,
    // Until when Claude Code is not asked a check's first question, after a failed, sessionless
    // or slow answer.
    claude_wait: Option<Instant>,
    // The problem each step last logged, by step, so each is logged once until it changes or
    // the step works again.
    problems: BTreeMap<&'static str, String>,
    // The last reason logged for waiting without asking Claude Code, such as a pane that does
    // not run Claude Code.
    waiting: Option<String>,
    // The reminder last logged as waiting in the queue, and why.
    stuck: Option<(String, String)>,
    // The requests whose reply the coordinator's transcript showed, which are never reminded: a
    // transcript only grows, so it is not read again for them.
    printed: BTreeSet<String>,
    #[cfg(test)]
    pub(super) said: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ReplyReminders {
    /// Reminders after `settings.idle`, asking `settings.claude_bin` whether the coordinator is
    /// idle.
    pub(super) fn new(settings: &ReplyReminderSettings) -> Self {
        Self::with_source(
            settings.idle,
            Box::new(ClaudeAgents::new(
                settings.claude_bin.clone(),
                PathBuf::from("/proc"),
            )),
        )
    }

    pub(crate) fn with_source(idle: Duration, source: Box<dyn ClaudeIdleSource>) -> Self {
        Self {
            idle,
            source,
            period: IdlePeriod::default(),
            hold: None,
            withdrawals: 0,
            retry_first: idle.saturating_mul(2),
            retry_max: MAX_REPLY_REMINDER_RETRY,
            claude_wait: None,
            problems: BTreeMap::new(),
            waiting: None,
            stuck: None,
            printed: BTreeSet::new(),
            #[cfg(test)]
            said: std::sync::Arc::default(),
        }
    }

    /// These reminders, holding off the next reminder after one taken back out of the queue for
    /// `first`, doubling with each in a row, and at most `max`, instead of the defaults.
    #[cfg(test)]
    pub(super) fn with_retry(mut self, first: Duration, max: Duration) -> Self {
        self.retry_first = first;
        self.retry_max = max;
        self
    }

    /// What the service logs at startup about these reminders.
    pub(super) fn describe(settings: &ReplyReminderSettings) -> String {
        format!(
            "agentctl: reply reminders: a coordinator idle for {}s after a request's \
prompt was typed, with no reply to it, is reminded once of that request; the harness in \
its pane tells whether it is idle (`{} agents --json` for a Claude Code pane, the pane's \
status for the others)",
            settings.idle.as_secs(),
            settings.claude_bin.display()
        )
    }

    fn say(&mut self, line: impl fmt::Display) {
        let line = line.to_string();
        service_log(&line);
        #[cfg(test)]
        self.said.lock().expect("reminder log").push(line);
    }

    fn failed(&mut self, step: &'static str, problem: impl fmt::Display) {
        let problem = problem.to_string();
        if self.problems.get(step) != Some(&problem) {
            self.say(format_args!(
                "agentctl: reply reminders: {step} failed; the next check tries again: {problem}"
            ));
            self.problems.insert(step, problem);
        }
    }

    fn worked(&mut self, step: &'static str) {
        if self.problems.remove(step).is_some() {
            self.say(format_args!(
                "agentctl: reply reminders: {step} works again"
            ));
        }
    }

    fn wait_because(&mut self, reason: Option<String>) {
        if reason != self.waiting {
            if let Some(reason) = &reason {
                self.say(format_args!("agentctl: reply reminders wait: {reason}"));
            }
            self.waiting = reason;
        }
    }

    /// Log what became of `reminder` for each request it named, by the reply ID `record` gives
    /// it. A reminder still waiting in the queue is logged once for each reason.
    fn report_outcome(
        &mut self,
        reminder: &PendingReminder,
        outcome: &ReminderOutcome,
        record: &ReplyReminderRecord,
    ) {
        if let ReminderOutcome::Waiting(why) = outcome {
            let logged = (reminder.message_id.clone(), why.clone());
            if self.stuck.as_ref() == Some(&logged) {
                return;
            }
            self.stuck = Some(logged);
        } else {
            self.stuck = None;
        }
        let described = outcome.describe();
        for key in &reminder.keys {
            let reply_id = record
                .reminded
                .iter()
                .find(|request| &request.key == key)
                .map_or("?", |request| request.reply_id.as_str());
            self.say(format_args!(
                "agentctl: the reply reminder for request {key} (reply ID {reply_id}) {described}"
            ));
        }
    }

    /// Settle a reminder recorded as waiting in the queue, as at startup, before any prompt is
    /// typed: one still in the queue's inbox is taken back out, so that no drain types it after
    /// its request may have been answered. Creates `reply-reminders.json`, with reminders starting
    /// now, when it does not exist. Returns whether a reminder is left waiting in the queue,
    /// which could not give it back; the owner loop then calls this again before its later
    /// passes until none is. A reminder taken back out here holds off nothing: at startup this
    /// run has seen no idle period yet.
    pub(super) fn settle<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
    ) -> bool {
        matches!(
            self.settle_recorded(state, manager, stop, None),
            Settled::Waiting
        )
    }

    /// [`Self::settle`], within a check when `seen_working` is the turn evidence's count, so that
    /// a reminder taken back out holds off the next.
    fn settle_recorded<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
        seen_working: Option<u64>,
    ) -> Settled {
        let now = chat_runtime::unix_millis();
        let record = match state.update_reply_reminders(now, None, |_| (false, ())) {
            Ok((record, ())) => {
                self.worked("the reminder record");
                record
            }
            Err(error) => {
                self.failed("the reminder record", error);
                return Settled::Unknown;
            }
        };
        let Some(reminder) = record.pending.clone() else {
            return Settled::Record(record);
        };
        let notes = TypingNotes::default();
        let delivery = TypingWatch {
            delivery: &CancellableDelivery {
                manager,
                runtime: StopRuntime::new(stop),
            },
            notes: &notes,
            state,
            receipts: stop.ack_queue.as_ref(),
        };
        let outcome = chat_runtime::settle_reminder(state, &delivery, &reminder);
        self.record(state, &reminder, &outcome, &record, now, None, seen_working)
    }

    /// Record and log what became of `reminder`, and, within a check, when `seen_working` is the
    /// turn evidence's count, hold off the next reminder after one taken back out of the queue.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        state: &BridgeState,
        reminder: &PendingReminder,
        outcome: &ReminderOutcome,
        before: &ReplyReminderRecord,
        now: u64,
        retained: Option<&std::collections::BTreeSet<String>>,
        seen_working: Option<u64>,
    ) -> Settled {
        self.report_outcome(reminder, outcome, before);
        match outcome {
            ReminderOutcome::Withdrawn(_) => {
                if let Some(seen_working) = seen_working {
                    self.hold_off(seen_working);
                }
            }
            ReminderOutcome::Waiting(_) => {}
            ReminderOutcome::Typed
            | ReminderOutcome::AlreadyTyped
            | ReminderOutcome::Uncertain(_) => self.withdrawals = 0,
        }
        let waiting = matches!(outcome, ReminderOutcome::Waiting(_));
        match chat_runtime::record_reminder_outcome(state, reminder, outcome, now, retained) {
            Ok(record) => {
                self.worked("the reminder record");
                if waiting {
                    Settled::Waiting
                } else {
                    Settled::Record(record)
                }
            }
            Err(error) => {
                self.failed("the reminder record", error);
                if waiting {
                    Settled::Waiting
                } else {
                    Settled::Unknown
                }
            }
        }
    }

    /// A reminder was taken back out of the queue, or never queued, when the turn evidence had
    /// counted `seen_working`: hold off the next until the coordinator has been seen working, or
    /// for the retry delay.
    fn hold_off(&mut self, seen_working: u64) {
        self.withdrawals = self.withdrawals.saturating_add(1);
        let delay = retry_delay(self.retry_first, self.withdrawals, self.retry_max);
        self.hold = Some(Hold {
            seen_working,
            until: Instant::now() + delay,
        });
        self.say(format_args!(
            "agentctl: reply reminders: no other reminder is sent until the coordinator has been \
seen working and is idle again, or for {}s if it is not seen working",
            delay.as_secs_f64().ceil()
        ));
    }

    /// Whether a reminder taken back out of the queue still holds off the next. The hold ends
    /// once the coordinator has been seen working since, which also ends the idle period, so the
    /// next reminder waits for a whole idle period after that work; or, if it has not been seen
    /// working, once the retry delay has passed, and the idle period goes on.
    fn held(&mut self, turn: &TurnEvidence) -> bool {
        let Some(hold) = &self.hold else {
            return false;
        };
        if turn.seen_working() != hold.seen_working {
            self.hold = None;
            self.withdrawals = 0;
            self.period.end();
            false
        } else if Instant::now() < hold.until {
            true
        } else {
            self.hold = None;
            false
        }
    }

    /// One check, as the module documentation describes. `prompts_waiting` says that the last
    /// scan found request prompts not typed yet: the coordinator is about to get work, and a
    /// reminder would be typed after their prompts, so none is composed. Only a failure that
    /// would stop any other pass of the loop, such as an unusable bridge state during the read
    /// before a reminder, or a Herdr call a stop cancelled, is an error; every other problem is
    /// logged and the next check tries again.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn check<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
        delivery: DrainOptions,
        routes: &mut RouteCache,
        control: &mut PassControl<'_>,
        turn: &mut TurnEvidence,
        prompts_waiting: bool,
    ) -> Result<ReminderCheck, ChatServiceError> {
        let mut check = ReminderCheck::default();
        if control.stopped() {
            return Ok(check);
        }
        let runtime = StopRuntime::new(stop);
        let agent_name = &state.config().agent_name;
        // A reminder an earlier check could not settle is settled before another is composed.
        let Settled::Record(record) =
            self.settle_recorded(state, manager, stop, Some(turn.seen_working()))
        else {
            return Ok(check);
        };
        let now = chat_runtime::unix_millis();
        let candidates = match state.reply_reminder_candidates() {
            Ok(candidates) => {
                self.worked("the request records");
                candidates
            }
            Err(error) => {
                self.failed("the request records", error);
                return Ok(check);
            }
        };
        // Only a request that can still be reminded needs to stay known as printed.
        self.printed.retain(|key| candidates.retained.contains(key));
        let eligible = candidates
            .eligible(&record, now)
            .into_iter()
            .filter(|candidate| !self.printed.contains(&candidate.key))
            .collect::<Vec<_>>();
        if eligible.is_empty() {
            // Nothing is left to remind, so nothing is held off either.
            self.period.end();
            self.hold = None;
            self.withdrawals = 0;
            return Ok(check);
        }
        if prompts_waiting {
            return Ok(check);
        }
        let Some(info) = self.lookup(manager, agent_name, &runtime, stop, turn)? else {
            return Ok(check);
        };
        // While a reminder taken back out of the queue holds off the next, Herdr's status still
        // ends the idle period, but Claude Code is not asked.
        if self.held(turn) {
            self.herdr_idle(&info, turn);
            return Ok(check);
        }
        if !self.idle(&info, turn, stop, Ask::First) {
            return Ok(check);
        }
        let idle_since = self.period.idle(now);
        if due_requests(&eligible, idle_since, now, self.idle).is_empty() {
            return Ok(check);
        }

        // Read the pane and capture it as a recovery scan does, so a reply on screen that no
        // capture has stored yet counts.
        let snapshot = match manager.read_capture_with_runtime(agent_name, SNAPSHOT_LINES, &runtime)
        {
            Ok(snapshot) => {
                self.worked("the read of the coordinator's pane");
                snapshot
            }
            Err(error) if stop.is_stopped() => return Err(error.into()),
            Err(error) => {
                self.failed("the read of the coordinator's pane", error);
                return Ok(check);
            }
        };
        turn.read(&snapshot);
        let report = capture_recovery_snapshot(
            state,
            manager,
            delivery,
            routes,
            SnapshotInput {
                text: &snapshot,
                truncated: false,
                revision: None,
            },
            control,
        )?;
        let left_work =
            report.prompt_typed || report.recovery_requested || !report.deferred_keys.is_empty();
        check.report = report;
        check.snapshot = Some(snapshot);
        // That capture may have typed a prompt, or left one for the coordinator, and Claude
        // Code reports a session busy only some time after text is typed: no reminder follows.
        if left_work || turn.working() || control.stopped() {
            return Ok(check);
        }
        let now = chat_runtime::unix_millis();
        let candidates = match state.reply_reminder_candidates() {
            Ok(candidates) => candidates,
            Err(error) => {
                self.failed("the request records", error);
                return Ok(check);
            }
        };
        let due = due_requests(
            &candidates.eligible(&record, now),
            idle_since,
            now,
            self.idle,
        );
        if due.is_empty() {
            return Ok(check);
        }
        // The coordinator may have started a turn of its own during the read, as when the owner
        // types into its pane.
        let Some(info) = self.lookup(manager, agent_name, &runtime, stop, turn)? else {
            return Ok(check);
        };
        if !self.idle(&info, turn, stop, Ask::Again) || self.period.since_millis != Some(idle_since)
        {
            return Ok(check);
        }
        // Right before the reminder is composed, after the answer that named the session, the
        // session's transcript shows whether the coordinator printed a reply that no read of the
        // pane has shown yet.
        let due = self.leave_out_printed(&info, due);
        if due.is_empty() {
            return Ok(check);
        }

        let reminder = chat_runtime::compose_reminder(state, &due, now);
        let recorded = state.update_reply_reminders(now, Some(&candidates.retained), |record| {
            for request in &due {
                if !record.reminded(&request.key) {
                    record.reminded.push(chat_runtime::RemindedRequest {
                        key: request.key.clone(),
                        reply_id: request.reply_id.clone(),
                        at_millis: now,
                    });
                }
            }
            record.pending = Some(reminder.clone());
            (true, ())
        });
        let record = match recorded {
            Ok((record, ())) => {
                self.worked("the reminder record");
                record
            }
            Err(error) => {
                self.failed("the reminder record", error);
                return Ok(check);
            }
        };
        let notes = TypingNotes::default();
        let outcome = chat_runtime::deliver_reminder(
            state,
            &TypingWatch {
                delivery: &CancellableDelivery {
                    manager,
                    runtime: StopRuntime::new(stop),
                },
                notes: &notes,
                state,
                receipts: stop.ack_queue.as_ref(),
            },
            &reminder,
            delivery,
        );
        self.record(
            state,
            &reminder,
            &outcome,
            &record,
            chat_runtime::unix_millis(),
            Some(&candidates.retained),
            Some(turn.seen_working()),
        );
        // Only a reminder that may have reached the pane counts as the start of a turn: one
        // refused before it was queued, or taken back out untyped, started none.
        check.report.prompt_typed |= outcome.may_have_typed();
        report_receipt_errors(&notes, &mut check.report);
        Ok(check)
    }

    /// `due` without each request whose reply ID heads a complete reply block in the transcript of
    /// the Claude Code session in the pane `info` describes, each of which is logged and never
    /// reminded. A transcript that cannot be found or read leaves `due` as it is, and is logged.
    fn leave_out_printed(
        &mut self,
        info: &AgentPaneInfo,
        due: Vec<ReminderCandidate>,
    ) -> Vec<ReminderCandidate> {
        let due = due
            .into_iter()
            .filter(|candidate| !self.printed.contains(&candidate.key))
            .collect::<Vec<_>>();
        if due.is_empty() {
            return due;
        }
        let reply_ids = due
            .iter()
            .map(|candidate| candidate.reply_id.clone())
            .collect::<BTreeSet<_>>();
        let printed = match self.source.printed_replies(info, &reply_ids) {
            Ok(printed) => {
                self.worked("the read of the coordinator's transcript");
                printed
            }
            Err(error) => {
                self.failed("the read of the coordinator's transcript", error);
                return due;
            }
        };
        let (left_out, kept): (Vec<_>, Vec<_>) = due
            .into_iter()
            .partition(|candidate| printed.contains(&candidate.reply_id));
        for candidate in left_out {
            self.say(format_args!(
                "agentctl: reply reminders: request {} (reply ID {}) is not reminded: the \
coordinator's {} transcript holds a complete reply block under that ID that no read of \
its pane has captured yet, as when the pane's view is scrolled away from its end; the bridge \
sends that reply once the pane shows it",
                candidate.key,
                candidate.reply_id,
                harness_display_name(info)
            ));
            self.printed.insert(candidate.key);
        }
        kept
    }

    /// Look the coordinator's pane up in Herdr, and note its status in `turn`. `None` when the
    /// lookup failed, which is logged.
    fn lookup<A: ManagedApi + ?Sized>(
        &mut self,
        manager: &ManagedAgents<'_, A>,
        agent_name: &str,
        runtime: &StopRuntime<'_>,
        stop: &StopState,
        turn: &mut TurnEvidence,
    ) -> Result<Option<AgentPaneInfo>, ChatServiceError> {
        match manager.pane_info_with_runtime(agent_name, runtime) {
            Ok(info) => {
                self.worked("the status lookup of the coordinator's pane");
                turn.agent_status(&info.status, info.agent.as_deref());
                Ok(Some(info))
            }
            Err(error) if stop.is_stopped() => Err(error.into()),
            Err(error) => {
                self.failed("the status lookup of the coordinator's pane", error);
                self.period.end();
                Ok(None)
            }
        }
    }

    /// Whether Herdr reports the pane `info` describes `idle` or `done` and the loop's own
    /// evidence shows no running turn. Ends the idle period when not.
    fn herdr_idle(&mut self, info: &AgentPaneInfo, turn: &TurnEvidence) -> bool {
        if matches!(info.status.as_str(), "idle" | "done") && !turn.working() {
            true
        } else {
            self.period.end();
            false
        }
    }

    /// Whether the coordinator in the pane `info` describes is idle: Herdr reports it `idle` or
    /// `done`, the loop's own evidence shows no running turn, and Claude Code says its session
    /// is `idle`. Claude Code is asked only when the other two hold. Ends the idle period when
    /// it is not, or cannot be found, idle. After a run of `claude agents --json` that failed,
    /// listed no session in the pane, or was slow, the check's first question waits for one
    /// idle time: until then this is `false` and leaves the idle period as it is.
    fn idle(
        &mut self,
        info: &AgentPaneInfo,
        turn: &TurnEvidence,
        stop: &StopState,
        ask: Ask,
    ) -> bool {
        if !self.herdr_idle(info, turn) {
            return false;
        }
        if ask == Ask::First && self.claude_wait.is_some_and(|until| Instant::now() < until) {
            return false;
        }
        let asked = Instant::now();
        let answer = self.source.state(info, &|| stop.is_stopped());
        let slow = asked.elapsed() > CLAUDE_SLOW_ANSWER;
        let wait = Instant::now() + self.idle;
        self.claude_wait = slow.then_some(wait);
        match answer {
            Ok(ClaudeState::Idle) => {
                self.worked("asking the coordinator's harness whether it is idle");
                self.wait_because(None);
                true
            }
            Ok(ClaudeState::Busy(_)) => {
                self.worked("asking the coordinator's harness whether it is idle");
                self.wait_because(None);
                self.period.end();
                false
            }
            Ok(ClaudeState::NotClaude) => {
                self.wait_because(Some(format!(
                    "the coordinator's pane runs {}, whose idleness reply reminders cannot \
                     ask about, so it is not reminded",
                    harness_display_name(info)
                )));
                self.period.end();
                false
            }
            Ok(ClaudeState::NoSession) => {
                self.worked("asking the coordinator's harness whether it is idle");
                self.wait_because(Some(format!(
                    "{} lists no session in the coordinator's pane {}",
                    harness_display_name(info),
                    info.pane_id
                )));
                self.claude_wait = Some(wait);
                self.period.end();
                false
            }
            Err(error) => {
                self.failed(
                    "asking the coordinator's harness whether it is idle",
                    error,
                );
                self.claude_wait = Some(wait);
                self.period.end();
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_idle_time_is_a_duration_of_ten_seconds_to_twelve_hours_or_zero_for_off() {
        for (value, idle) in [
            ("60s", Some(60)),
            ("60", Some(60)),
            ("1m", Some(60)),
            ("10s", Some(10)),
            ("12h", Some(43_200)),
            ("720m", Some(43_200)),
            ("0", None),
            ("0m", None),
        ] {
            assert_eq!(
                parse_idle(value),
                Ok(idle.map(Duration::from_secs)),
                "{value}"
            );
        }
        for value in ["9s", "13h", "43201", "24h", "1d", "-1", "", "1.5m", "m"] {
            assert!(parse_idle(value).is_err(), "{value}");
        }
        assert_eq!(DEFAULT_REPLY_REMINDER_IDLE, Duration::from_secs(60));
    }

    fn candidate(key: char, delivered_at_millis: u64) -> ReminderCandidate {
        ReminderCandidate {
            key: key.to_string().repeat(64),
            reply_id: "001".to_owned(),
            delivered_at_millis,
        }
    }

    #[test]
    fn a_request_is_due_once_the_coordinator_has_been_idle_long_enough_since_its_prompt() {
        let idle = Duration::from_secs(60);
        let early = candidate('a', 1_000);
        let late = candidate('b', 50_000);
        let eligible = [early.clone(), late.clone()];
        // Idle since 10 s: the early request is due at 70 s; the late one, typed during the idle
        // period, only 60 s after it was typed.
        assert!(due_requests(&eligible, 10_000, 69_999, idle).is_empty());
        assert_eq!(
            due_requests(&eligible, 10_000, 70_000, idle),
            std::slice::from_ref(&early)
        );
        assert_eq!(
            due_requests(&eligible, 10_000, 109_999, idle),
            std::slice::from_ref(&early)
        );
        assert_eq!(due_requests(&eligible, 10_000, 110_000, idle), eligible);
    }

    #[test]
    fn an_idle_period_ends_at_the_first_check_that_finds_the_coordinator_not_idle() {
        let mut period = IdlePeriod::default();
        assert_eq!(period.idle(5), 5);
        assert_eq!(
            period.idle(9),
            5,
            "the period began at the first idle check"
        );
        period.end();
        assert_eq!(period.idle(12), 12);
    }

    #[test]
    fn the_retry_delay_doubles_with_each_reminder_taken_back_in_a_row_up_to_its_bound() {
        let idle = Duration::from_secs(60);
        let first = idle * 2;
        let delays = (1..=8)
            .map(|withdrawals| {
                retry_delay(first, withdrawals, MAX_REPLY_REMINDER_RETRY).as_secs() / 60
            })
            .collect::<Vec<_>>();
        assert_eq!(delays, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(
            retry_delay(first, u32::MAX, MAX_REPLY_REMINDER_RETRY),
            MAX_REPLY_REMINDER_RETRY
        );
        assert_eq!(MAX_REPLY_REMINDER_RETRY, Duration::from_secs(3_600));
        // A long idle time is held off for at most the bound.
        assert_eq!(
            retry_delay(MAX_REPLY_REMINDER_IDLE * 2, 1, MAX_REPLY_REMINDER_RETRY),
            MAX_REPLY_REMINDER_RETRY
        );
    }

    #[test]
    fn a_hold_ends_once_the_coordinator_is_seen_working_or_its_delay_has_passed() {
        let mut reminders = ReplyReminders::with_source(
            Duration::from_secs(10),
            Box::new(Answers(std::sync::Mutex::new(Vec::new()))),
        );
        let mut turn = TurnEvidence::default();
        turn.status("idle");
        assert!(!reminders.held(&turn), "nothing holds at first");
        reminders.hold_off(turn.seen_working());
        assert_eq!(reminders.withdrawals, 1);
        assert!(reminders.held(&turn));
        // Neither a prompt the loop asked to be typed nor a screen that shows no running turn is
        // a sign of work.
        turn.report(&CycleReport {
            prompt_typed: true,
            ..CycleReport::default()
        });
        turn.read("visible output");
        turn.status("idle");
        assert!(reminders.held(&turn));
        // Herdr reporting the pane working is: the hold, the run of withdrawals and the idle
        // period all end.
        reminders.period.idle(1);
        turn.status("working");
        assert!(!reminders.held(&turn));
        assert_eq!(reminders.withdrawals, 0);
        assert_eq!(reminders.period.since_millis, None);
        assert!(reminders.hold.is_none());

        // A screen that shows a running turn is a sign of work too.
        reminders.hold_off(turn.seen_working());
        assert!(reminders.held(&turn));
        turn.read("✻ Thinking… (esc to interrupt)\n");
        assert!(!reminders.held(&turn));

        // Not seen working, a hold lasts its delay, after which the idle period goes on.
        let mut reminders = ReplyReminders::with_source(
            Duration::from_secs(10),
            Box::new(Answers(std::sync::Mutex::new(Vec::new()))),
        )
        .with_retry(Duration::from_millis(50), Duration::from_secs(1));
        let turn = TurnEvidence::default();
        reminders.period.idle(7);
        reminders.hold_off(turn.seen_working());
        assert!(reminders.held(&turn));
        std::thread::sleep(Duration::from_millis(60));
        assert!(!reminders.held(&turn));
        assert_eq!(reminders.period.since_millis, Some(7));
        assert_eq!(reminders.withdrawals, 1, "the next in a row waits longer");
        reminders.hold_off(turn.seen_working());
        assert!(reminders
            .hold
            .as_ref()
            .is_some_and(|hold| hold.until.saturating_duration_since(Instant::now())
                > Duration::from_millis(60)));
    }

    /// A stand-in for Claude Code that gives its answers in turn, the last first, and is idle
    /// once none is left.
    struct Answers(std::sync::Mutex<Vec<Result<ClaudeState, String>>>);

    impl ClaudeIdleSource for Answers {
        fn state(
            &self,
            _info: &AgentPaneInfo,
            _cancelled: &dyn Fn() -> bool,
        ) -> Result<ClaudeState, String> {
            self.0
                .lock()
                .expect("answers")
                .pop()
                .unwrap_or(Ok(ClaudeState::Idle))
        }
    }

    /// A stand-in `claude` that prints `listing`, and a process table in which process 101 runs
    /// in pane `w:coordinator` and process 202 in pane `w:other`.
    fn claude_fixture(
        name: &str,
        listing: &str,
    ) -> (std::path::PathBuf, Box<dyn ClaudeIdleSource>) {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-reply-reminder-claude-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("fixture directory");
        let claude = directory.join("claude");
        fs::write(&claude, format!("#!/bin/sh\ncat <<'END'\n{listing}\nEND\n"))
            .expect("write claude");
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).expect("claude mode");
        let proc_root = directory.join("proc");
        for (pid, pane) in [("101", "w:coordinator"), ("202", "w:other")] {
            fs::create_dir_all(proc_root.join(pid)).expect("process");
            fs::write(
                proc_root.join(pid).join("environ"),
                format!("HOME=/home/x\0HERDR_PANE_ID={pane}\0"),
            )
            .expect("environ");
        }
        let source = Box::new(ClaudeAgents::new(claude, proc_root));
        (directory, source)
    }

    fn pane(agent: &str, pane_id: &str) -> AgentPaneInfo {
        AgentPaneInfo {
            pane_id: pane_id.to_owned(),
            workspace_id: "w".to_owned(),
            cwd: "/".to_owned(),
            agent: Some(agent.to_owned()),
            status: "idle".to_owned(),
            session_agent: None,
            session_value: None,
            scroll: None,
            terminal_id: None,
            tab_id: None,
        }
    }

    /// One transcript line: an assistant message whose content is `content`.
    fn assistant(content: serde_json::Value) -> String {
        serde_json::json!({"type": "assistant", "message": {"content": content}}).to_string()
    }

    #[test]
    fn a_transcript_shows_a_reply_only_for_a_complete_block_in_ordinary_text() {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-reply-reminder-transcript-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).expect("fixture directory");
        let path = directory.join("s.jsonl");
        let text = |text: &str| serde_json::json!([{"type": "text", "text": text}]);
        let lines = [
            // Complete, with the bullet-free indentation a model sometimes adds.
            assistant(text(
                "Done.\n  <CHAT_REPLY_5002>\nThe answer.\n  </CHAT_REPLY_5002>  \n",
            )),
            // Only in thinking, or only in a tool call: not printed.
            assistant(serde_json::json!([{"type": "thinking", "thinking":
                "<CHAT_REPLY_5003>\nthought\n</CHAT_REPLY_5003>"}])),
            assistant(serde_json::json!([{"type": "tool_use", "input": {"command":
                "<CHAT_REPLY_5004>\nx\n</CHAT_REPLY_5004>"}}])),
            // Inside a code fence, sharing a line with other text, unclosed, or empty.
            assistant(text(
                "```text\n<CHAT_REPLY_5005>\nquoted\n</CHAT_REPLY_5005>\n```",
            )),
            assistant(text(
                "Reply under <CHAT_REPLY_5006> and </CHAT_REPLY_5006> later.",
            )),
            assistant(text("<CHAT_REPLY_5007>\nstarted")),
            assistant(text("<CHAT_REPLY_5008>\n\n</CHAT_REPLY_5008>")),
            // A user prompt that quotes a block is not the coordinator's output.
            serde_json::json!({"type": "user", "message": {"content":
                "<CHAT_REPLY_5009>\nquoted\n</CHAT_REPLY_5009>"}})
            .to_string(),
            // A closing line under another ID ends nothing, and a fence closed before the block
            // does not hide it.
            assistant(text("<CHAT_REPLY_5010>\nbody\n</CHAT_REPLY_5011>")),
            assistant(text(
                "```\ncode\n```\n<CHAT_REPLY_5012>\nbody\n</CHAT_REPLY_5012>",
            )),
            "{not json".to_owned(),
        ];
        fs::write(&path, lines.join("\n") + "\n").expect("transcript");
        let asked = (5002..=5012)
            .map(|id| id.to_string())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            printed_reply_ids(&path, &asked).expect("read"),
            BTreeSet::from(["5002".to_owned(), "5012".to_owned()])
        );
        // Only the IDs asked about are reported.
        assert_eq!(
            printed_reply_ids(&path, &BTreeSet::from(["5003".to_owned()])).expect("read"),
            BTreeSet::new()
        );
        // Only the end of a long transcript is read.
        let mut long = assistant(text("<CHAT_REPLY_5002>\nold\n</CHAT_REPLY_5002>")) + "\n";
        let filler = assistant(text(&"x".repeat(1_000))) + "\n";
        let fillers = usize::try_from(TRANSCRIPT_TAIL_BYTES).expect("bytes") / filler.len() + 1;
        long.push_str(&filler.repeat(fillers));
        fs::write(&path, long).expect("long transcript");
        assert_eq!(
            printed_reply_ids(&path, &asked).expect("read"),
            BTreeSet::new()
        );
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn the_transcript_read_is_the_one_of_the_session_claude_code_last_named_in_the_pane() {
        let (directory, _) = claude_fixture("transcript", "[]");
        let claude = directory.join("claude");
        let listing = r#"[{"pid":101,"sessionId":"s-1","status":"idle"}]"#;
        fs::write(&claude, format!("#!/bin/sh\ncat <<'END'\n{listing}\nEND\n"))
            .expect("write claude");
        let config = directory.join("config");
        let project = config.join("projects").join("-work-project");
        fs::create_dir_all(&project).expect("project");
        fs::write(
            directory.join("proc").join("101").join("environ"),
            format!(
                "HOME=/home/x\0CLAUDE_CONFIG_DIR={}\0HERDR_PANE_ID=w:coordinator\0",
                config.display()
            ),
        )
        .expect("environ");
        let source = ClaudeAgents::new(claude, directory.join("proc"));
        let ids = BTreeSet::from(["5002".to_owned()]);
        let coordinator = pane("claude", "w:coordinator");
        let error = source
            .printed_replies(&coordinator, &ids)
            .expect_err("no session was named yet");
        assert!(error.contains("named no session ID"), "{error}");
        assert_eq!(source.state(&coordinator, &|| false), Ok(ClaudeState::Idle));
        let error = source
            .printed_replies(&coordinator, &ids)
            .expect_err("the transcript does not exist yet");
        assert!(error.contains("no transcript s-1.jsonl"), "{error}");
        fs::write(
            project.join("s-1.jsonl"),
            assistant(serde_json::json!([{"type": "text",
                "text": "<CHAT_REPLY_5002>\nanswer\n</CHAT_REPLY_5002>"}]))
                + "\n",
        )
        .expect("transcript");
        assert_eq!(source.printed_replies(&coordinator, &ids), Ok(ids.clone()));
        // Another pane's question replaces the session it reads.
        assert_eq!(
            source.state(&pane("claude", "w:other"), &|| false),
            Ok(ClaudeState::NoSession)
        );
        assert!(source.printed_replies(&coordinator, &ids).is_err());
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn claude_code_tells_whether_the_session_in_the_coordinator_pane_is_idle() {
        let (directory, source) = claude_fixture(
            "states",
            r#"[{"pid":101,"sessionId":"s","status":"idle"},{"pid":"202","status":"busy"}]"#,
        );
        assert_eq!(
            source.state(&pane("claude", "w:coordinator"), &|| false),
            Ok(ClaudeState::Idle)
        );
        assert_eq!(
            source.state(&pane("claude", "w:other"), &|| false),
            Ok(ClaudeState::Busy("busy".to_owned()))
        );
        assert_eq!(
            source.state(&pane("claude", "w:elsewhere"), &|| false),
            Ok(ClaudeState::NoSession)
        );
        assert_eq!(
            source.state(&pane("codex", "w:coordinator"), &|| false),
            Ok(ClaudeState::Idle),
            "a Codex pane's own status answers instead of Claude Code's CLI"
        );
        assert_eq!(
            source.state(&pane("muse", "w:coordinator"), &|| false),
            Ok(ClaudeState::Idle),
            "a Muse pane's own status answers instead of Claude Code's CLI"
        );
        let mut working = pane("muse", "w:coordinator");
        working.status = "working".to_owned();
        assert_eq!(
            source.state(&working, &|| false),
            Ok(ClaudeState::Busy("working".to_owned())),
            "a working Muse pane is not idle"
        );
        assert_eq!(
            source.state(&pane("something-else", "w:coordinator"), &|| false),
            Ok(ClaudeState::NotClaude),
            "a harness reminders cannot ask about is never reminded"
        );
        assert_eq!(
            harness_display_name(&pane("muse", "w:coordinator")),
            "Muse",
            "reminder text names the detected harness"
        );
        assert_eq!(
            harness_display_name(&pane("claude", "w:coordinator")),
            "Claude Code"
        );
        fs::remove_dir_all(directory).expect("cleanup");

        // Any state other than `idle`, such as a background shell, counts as busy.
        let (directory, source) =
            claude_fixture("shell", r#"[{"pid":101,"sessionId":"s","status":"shell"}]"#);
        assert_eq!(
            source.state(&pane("claude", "w:coordinator"), &|| false),
            Ok(ClaudeState::Busy("shell".to_owned()))
        );
        fs::remove_dir_all(directory).expect("cleanup");

        let (directory, source) = claude_fixture("garbled", "not json");
        let error = source
            .state(&pane("claude", "w:coordinator"), &|| false)
            .expect_err("an unreadable answer is an error");
        assert!(error.contains("cannot parse"), "{error}");
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
