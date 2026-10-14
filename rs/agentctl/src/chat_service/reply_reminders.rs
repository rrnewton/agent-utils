//! When `chat run` reminds a Claude Code coordinator of a delivered chat request it has not
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
//! the start of that period and its prompt was typed.
//!
//! Before reminding, the check reads the pane and captures it as a recovery scan does, so a reply
//! on screen that no capture stored yet counts, and it reminds no one if that capture may have
//! typed or left a prompt for the coordinator. Herdr and Claude Code are then asked once more.
//! The reminder is recorded before it is queued and is typed only through the queue's normal
//! checks, never into a working pane; one the queue does not type is taken back out of it, and
//! no other is composed until the coordinator has been busy and idle again.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

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
/// The longest `--reply-reminder-idle`: a request delivered longer ago than this is never
/// reminded anyway.
pub const MAX_REPLY_REMINDER_IDLE: Duration = chat_runtime::MAX_REPLY_REMINDER_AGE;
/// How long one `claude agents --json` may run before it is killed and the check gives up.
const CLAUDE_QUERY_TIMEOUT: Duration = Duration::from_secs(10);

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

/// What Claude Code says about the session in the coordinator's pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeState {
    /// The session waits at its prompt.
    Idle,
    /// Any other state Claude Code reports, such as `busy`.
    Busy(String),
    /// Herdr does not report the pane as running Claude Code.
    NotClaude,
    /// Claude Code lists no session in the pane.
    NoSession,
}

/// Where a check learns whether Claude Code in the coordinator's pane is idle.
pub(crate) trait ClaudeIdleSource {
    /// The state of the Claude Code session in the pane `info` describes, or why Claude Code
    /// could not be asked.
    fn state(&self, info: &AgentPaneInfo) -> Result<ClaudeState, String>;
}

/// `claude agents --json`, joined to Herdr panes as `agentctl inbox watch` joins it.
struct ClaudeAgents {
    claude: PathBuf,
    proc_root: PathBuf,
}

impl ClaudeIdleSource for ClaudeAgents {
    fn state(&self, info: &AgentPaneInfo) -> Result<ClaudeState, String> {
        if info.agent.as_deref() != Some("claude") {
            return Ok(ClaudeState::NotClaude);
        }
        let sessions = crate::inbox::claude_sessions_by_pane(
            &self.claude,
            &self.proc_root,
            CLAUDE_QUERY_TIMEOUT,
        )
        .map_err(|error| error.to_string())?;
        Ok(
            match sessions
                .get(&info.pane_id)
                .and_then(|session| session.status())
            {
                Some("idle") => ClaudeState::Idle,
                Some(other) => ClaudeState::Busy(other.to_owned()),
                None => ClaudeState::NoSession,
            },
        )
    }
}

/// The idle period a check has followed: when the first check of an unbroken run that found the
/// coordinator idle saw it so, and whether a reminder was taken back out of the queue during it.
#[derive(Debug, Default)]
struct IdlePeriod {
    since_millis: Option<u64>,
    withdrawn: bool,
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

/// The reply reminders of one `chat run`.
pub(crate) struct ReplyReminders {
    idle: Duration,
    source: Box<dyn ClaudeIdleSource>,
    period: IdlePeriod,
    // The problem each step last logged, by step, so each is logged once until it changes or
    // the step works again.
    problems: BTreeMap<&'static str, String>,
    // The last reason logged for waiting without asking Claude Code, such as a pane that does
    // not run Claude Code.
    waiting: Option<String>,
    // The reminder last logged as waiting in the queue, and why.
    stuck: Option<(String, String)>,
    #[cfg(test)]
    pub(super) said: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ReplyReminders {
    /// Reminders after `settings.idle`, asking `settings.claude_bin` whether the coordinator is
    /// idle.
    pub(super) fn new(settings: &ReplyReminderSettings) -> Self {
        Self::with_source(
            settings.idle,
            Box::new(ClaudeAgents {
                claude: settings.claude_bin.clone(),
                proc_root: PathBuf::from("/proc"),
            }),
        )
    }

    pub(crate) fn with_source(idle: Duration, source: Box<dyn ClaudeIdleSource>) -> Self {
        Self {
            idle,
            source,
            period: IdlePeriod::default(),
            problems: BTreeMap::new(),
            waiting: None,
            stuck: None,
            #[cfg(test)]
            said: std::sync::Arc::default(),
        }
    }

    /// What the service logs at startup about these reminders.
    pub(super) fn describe(settings: &ReplyReminderSettings) -> String {
        format!(
            "agentctl: reply reminders: a Claude Code coordinator idle for {}s after a request's \
prompt was typed, with no reply to it, is reminded once of that request; `{} agents --json` \
tells whether it is idle",
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
    /// now, when it does not exist. Returns whether no reminder is left waiting. A reminder taken
    /// back out during a check holds off the next one until the coordinator has been busy and
    /// idle again; one an earlier run left does not, since this run has seen no idle period yet.
    pub(super) fn settle<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
    ) -> bool {
        let settled = self.settle_recorded(state, manager, stop).is_some();
        self.period.withdrawn = false;
        settled
    }

    /// [`Self::settle`] within a check: the record once no reminder is left waiting, or `None`.
    fn settle_recorded<A: ManagedApi + ?Sized>(
        &mut self,
        state: &BridgeState,
        manager: &ManagedAgents<'_, A>,
        stop: &StopState,
    ) -> Option<ReplyReminderRecord> {
        let now = chat_runtime::unix_millis();
        let record = match state.update_reply_reminders(now, None, |_| (false, ())) {
            Ok((record, ())) => {
                self.worked("the reminder record");
                record
            }
            Err(error) => {
                self.failed("the reminder record", error);
                return None;
            }
        };
        let Some(reminder) = record.pending.clone() else {
            return Some(record);
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
        self.record(state, &reminder, &outcome, &record, now, None)
    }

    /// Record and log what became of `reminder`: the record once the reminder has left it, or
    /// `None` while it waits in the queue or the record cannot be written.
    fn record(
        &mut self,
        state: &BridgeState,
        reminder: &PendingReminder,
        outcome: &ReminderOutcome,
        before: &ReplyReminderRecord,
        now: u64,
        retained: Option<&std::collections::BTreeSet<String>>,
    ) -> Option<ReplyReminderRecord> {
        self.report_outcome(reminder, outcome, before);
        if matches!(outcome, ReminderOutcome::Withdrawn(_)) {
            // No reminder is composed again until the coordinator has been busy and idle again.
            self.period.withdrawn = true;
        }
        match chat_runtime::record_reminder_outcome(state, reminder, outcome, now, retained) {
            Ok(record) => {
                self.worked("the reminder record");
                (!matches!(outcome, ReminderOutcome::Waiting(_))).then_some(record)
            }
            Err(error) => {
                self.failed("the reminder record", error);
                None
            }
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
        let Some(record) = self.settle_recorded(state, manager, stop) else {
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
        let eligible = candidates.eligible(&record, now);
        if eligible.is_empty() {
            self.period.end();
            return Ok(check);
        }
        if prompts_waiting {
            return Ok(check);
        }
        let Some(info) = self.lookup(manager, agent_name, &runtime, stop, turn)? else {
            return Ok(check);
        };
        // A reminder taken back out of the queue during this idle period waits for the next one,
        // which begins once Herdr or the pane has shown the coordinator working; Claude Code is
        // not asked again until then.
        if self.period.withdrawn && self.herdr_idle(&info, turn) {
            return Ok(check);
        }
        if !self.idle(&info, turn) {
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
        if !self.idle(&info, turn) || self.period.since_millis != Some(idle_since) {
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
        );
        check.report.prompt_typed |= notes.typed.get();
        report_receipt_errors(&notes, &mut check.report);
        Ok(check)
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
    /// it is not, or cannot be found, idle.
    fn idle(&mut self, info: &AgentPaneInfo, turn: &TurnEvidence) -> bool {
        if !self.herdr_idle(info, turn) {
            return false;
        }
        match self.source.state(info) {
            Ok(ClaudeState::Idle) => {
                self.worked("asking Claude Code whether the coordinator is idle");
                self.wait_because(None);
                true
            }
            Ok(ClaudeState::Busy(_)) => {
                self.worked("asking Claude Code whether the coordinator is idle");
                self.wait_because(None);
                self.period.end();
                false
            }
            Ok(ClaudeState::NotClaude) => {
                self.wait_because(Some(
                    "the coordinator's pane does not run Claude Code, so it is not reminded"
                        .to_owned(),
                ));
                self.period.end();
                false
            }
            Ok(ClaudeState::NoSession) => {
                self.worked("asking Claude Code whether the coordinator is idle");
                self.wait_because(Some(format!(
                    "Claude Code lists no session in the coordinator's pane {}",
                    info.pane_id
                )));
                self.period.end();
                false
            }
            Err(error) => {
                self.failed("asking Claude Code whether the coordinator is idle", error);
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
    fn the_idle_time_is_a_duration_of_at_least_ten_seconds_or_zero_for_off() {
        for (value, idle) in [
            ("60s", Some(60)),
            ("60", Some(60)),
            ("1m", Some(60)),
            ("10s", Some(10)),
            ("24h", Some(86_400)),
            ("0", None),
            ("0m", None),
        ] {
            assert_eq!(
                parse_idle(value),
                Ok(idle.map(Duration::from_secs)),
                "{value}"
            );
        }
        for value in ["9s", "25h", "1d", "-1", "", "1.5m", "m"] {
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
        period.withdrawn = true;
        period.end();
        assert!(
            !period.withdrawn,
            "a withdrawal holds only its own idle period"
        );
        assert_eq!(period.idle(12), 12);
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
        let source = Box::new(ClaudeAgents { claude, proc_root });
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

    #[test]
    fn claude_code_tells_whether_the_session_in_the_coordinator_pane_is_idle() {
        let (directory, source) = claude_fixture(
            "states",
            r#"[{"pid":101,"sessionId":"s","status":"idle"},{"pid":"202","status":"busy"}]"#,
        );
        assert_eq!(
            source.state(&pane("claude", "w:coordinator")),
            Ok(ClaudeState::Idle)
        );
        assert_eq!(
            source.state(&pane("claude", "w:other")),
            Ok(ClaudeState::Busy("busy".to_owned()))
        );
        assert_eq!(
            source.state(&pane("claude", "w:elsewhere")),
            Ok(ClaudeState::NoSession)
        );
        assert_eq!(
            source.state(&pane("codex", "w:coordinator")),
            Ok(ClaudeState::NotClaude),
            "a pane that does not run Claude Code is never reminded"
        );
        fs::remove_dir_all(directory).expect("cleanup");

        // Any state other than `idle`, such as a background shell, counts as busy.
        let (directory, source) =
            claude_fixture("shell", r#"[{"pid":101,"sessionId":"s","status":"shell"}]"#);
        assert_eq!(
            source.state(&pane("claude", "w:coordinator")),
            Ok(ClaudeState::Busy("shell".to_owned()))
        );
        fs::remove_dir_all(directory).expect("cleanup");

        let (directory, source) = claude_fixture("garbled", "not json");
        let error = source
            .state(&pane("claude", "w:coordinator"))
            .expect_err("an unreadable answer is an error");
        assert!(error.contains("cannot parse"), "{error}");
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
