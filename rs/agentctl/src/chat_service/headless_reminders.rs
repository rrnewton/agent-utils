//! Reply reminders for a headless coordinator (#2331 chat-headless-reply-reminders).
//!
//! The pane-based check in `reply_reminders` judges a TUI coordinator's idle time from Herdr and
//! Claude Code. A headless coordinator has neither, but its event log says exactly when the turn
//! that took a request ended: the request's queue item `chat-<key>` starts (alone, batched with
//! others, or mid-turn), and the first `turn.completed` after that start, or the harness exiting,
//! ends it. A request still holding no reply [`HEADLESS_REMINDER_GRACE`] after its turn ended
//! is due a reminder: the model either forgot the reply block or wrote a reply nowhere the bridge
//! reads (2026-11-26: a coordinator logged "REPLIED (CHAT_REPLY_N)" in a status line and never
//! printed the block; nobody noticed for 45 minutes). The grace lets the transcript reader
//! capture a block printed at the very end of the turn first.
//!
//! The reminder itself is recorded and composed exactly as for a TUI coordinator (once per
//! request, recorded before it is queued, in `reply-reminders.json`). It is queued at the turn
//! boundary through the headless queue, whose wrapper delivers it when the agent is idle, so a
//! queued reminder counts as typed.
//!
//! A mid-turn notice (#2386 headless-reply-in-thinking) comes earlier, while the turn still
//! runs: a request whose input started, with no reply after [`DEFAULT_NOTICE_TOOLS`] tool calls
//! spread over at least [`DEFAULT_NOTICE_AFTER`], is noticed once, so a reply written only in
//! thinking is caught minutes before the turn ends. Both thresholds are read from the event
//! log's own timestamps, never the wall clock, so host load cannot bring one forward. Notices
//! are recorded in `reply-notices.json` (see `chat_runtime::reply_notices`); the turn-end
//! reminder stays the backstop.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::agent::{AgentError, DrainOptions};
use crate::chat_runtime::{PendingNotice, PendingReminder, ReminderCandidate, ReminderOutcome};
use crate::headless::events::{self, Event};
use crate::subagents::{ManagedAgents, ManagedApi};

/// How long after the turn that took a request ended a headless coordinator is reminded of it,
/// when it holds no reply by then.
pub(crate) const HEADLESS_REMINDER_GRACE: Duration = Duration::from_secs(20);
/// Most bytes at the end of the event log read for turn ends.
const LOG_READ_BYTES: u64 = 4 * 1024 * 1024;
/// Least time between two reads of the event log while requests wait for their turn to end.
const LOG_READ_INTERVAL: Duration = Duration::from_secs(10);
/// How long a reminder the headless queue refused holds off the next.
const RETRY_AFTER_REFUSAL: Duration = Duration::from_secs(60);
/// How many tool calls after a request's input started, with no reply to it, bring a mid-turn
/// notice (#2386), by default.
pub(crate) const DEFAULT_NOTICE_TOOLS: usize = 3;
/// How long after a request's input started its latest counted tool call must come, by the event
/// log's timestamps, before a mid-turn notice, by default.
pub(crate) const DEFAULT_NOTICE_AFTER: Duration = Duration::from_secs(2 * 60);
/// The shortest and longest `--reply-notice-after`.
pub(crate) const MIN_NOTICE_AFTER: Duration = Duration::from_secs(30);
pub(crate) const MAX_NOTICE_AFTER: Duration = Duration::from_secs(60 * 60);
/// The most `--reply-notice-tools`.
pub(crate) const MAX_NOTICE_TOOLS: usize = 100;

/// The queue item of chat request `key`, as the bridge names it.
fn queue_id(key: &str) -> String {
    format!("chat-{key}")
}

/// How long before a request's recorded delivery its input may have started and still count:
/// the bridge records a headless delivery when it sees the model start the input, which can be
/// late after a bridge restart. Older starts of the same queue item are ignored.
const START_SLACK_MILLIS: u64 = 10 * 60 * 1000;

/// When the turn that took each chat request in `since` ended, from the event log `events`
/// (oldest first). `since` maps each request key to the earliest time its input may have started.
/// The answer is the time of the first `turn.completed` or `lifecycle.harness_exited` after the
/// request's latest start: a later start of the same input, as after the harness dropped it
/// before the model saw it (`input.cancelled` with `before_start`), supersedes an earlier one. A
/// request whose turn has not ended, or that has not started, is absent.
pub(crate) fn turn_ends(events: &[Event], since: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    let wanted: BTreeMap<String, (&String, u64)> = since
        .iter()
        .map(|(key, at)| (queue_id(key), (key, *at)))
        .collect();
    let mut open: BTreeSet<&String> = BTreeSet::new();
    let mut ends = BTreeMap::new();
    for event in events {
        let item = event.data["queue_id"]
            .as_str()
            .and_then(|id| wanted.get(id))
            .filter(|(_, at)| event.ts >= *at)
            .map(|(key, _)| *key);
        match event.kind.as_str() {
            "input.started" => {
                if let Some(key) = item {
                    ends.remove(key);
                    open.insert(key);
                }
            }
            "input.cancelled" if event.data["before_start"] == true => {
                if let Some(key) = item {
                    open.remove(key);
                }
            }
            "turn.completed" | "lifecycle.harness_exited" => {
                for key in std::mem::take(&mut open) {
                    ends.insert(key.clone(), event.ts);
                }
            }
            _ => {}
        }
    }
    ends
}

/// How far the still-running turn that took a chat request has gone since the request's input
/// started (#2386): the start's timestamp, the tool calls since, and the latest one's timestamp.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Progress {
    pub(crate) started: u64,
    pub(crate) tool_calls: usize,
    pub(crate) last_tool: Option<u64>,
}

/// The progress of each chat request in `since` whose turn is still running, from the event log
/// `events` (oldest first), by the rules of [`turn_ends`]: counted from the request's latest
/// start, and absent once a `turn.completed` or harness exit follows that start, or when the
/// request has not started.
pub(crate) fn progress(
    events: &[Event],
    since: &BTreeMap<String, u64>,
) -> BTreeMap<String, Progress> {
    let wanted: BTreeMap<String, (&String, u64)> = since
        .iter()
        .map(|(key, at)| (queue_id(key), (key, *at)))
        .collect();
    let mut open: BTreeMap<&String, Progress> = BTreeMap::new();
    for event in events {
        let item = event.data["queue_id"]
            .as_str()
            .and_then(|id| wanted.get(id))
            .filter(|(_, at)| event.ts >= *at)
            .map(|(key, _)| *key);
        match event.kind.as_str() {
            "input.started" => {
                if let Some(key) = item {
                    open.insert(
                        key,
                        Progress {
                            started: event.ts,
                            ..Progress::default()
                        },
                    );
                }
            }
            "input.cancelled" if event.data["before_start"] == true => {
                if let Some(key) = item {
                    open.remove(key);
                }
            }
            "tool.started" => {
                for progress in open.values_mut() {
                    progress.tool_calls += 1;
                    progress.last_tool = Some(event.ts);
                }
            }
            "turn.completed" | "lifecycle.harness_exited" => open.clear(),
            _ => {}
        }
    }
    open.into_iter()
        .map(|(key, progress)| (key.clone(), progress))
        .collect()
}

/// The candidates among `eligible` due a mid-turn notice: still in their turn after at least
/// `tools` tool calls, the latest at least `after` past the input's start by the log's clock.
/// The tool-call count the notice states is the largest among them.
pub(crate) fn notice_due(
    eligible: &[ReminderCandidate],
    progress: &BTreeMap<String, Progress>,
    tools: usize,
    after: Duration,
) -> (Vec<ReminderCandidate>, usize) {
    let after = u64::try_from(after.as_millis()).unwrap_or(u64::MAX);
    let mut most = 0;
    let due = eligible
        .iter()
        .filter(|candidate| {
            progress.get(&candidate.key).is_some_and(|progress| {
                let due = tools > 0
                    && progress.tool_calls >= tools
                    && progress
                        .last_tool
                        .is_some_and(|last| last.saturating_sub(progress.started) >= after);
                if due {
                    most = most.max(progress.tool_calls);
                }
                due
            })
        })
        .cloned()
        .collect();
    (due, most)
}

/// Parse `--reply-notice-after`: a whole number of seconds (`s`, or no unit), minutes (`m`) or
/// hours (`h`), at least [`MIN_NOTICE_AFTER`] and at most [`MAX_NOTICE_AFTER`].
pub(crate) fn parse_notice_after(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    let (number, unit) = match value.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((index, _)) => value.split_at(index),
        None => (value, "s"),
    };
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        _ => return Err("expected a whole number with unit s, m or h, such as 2m".to_owned()),
    };
    let count: u64 = number
        .parse()
        .map_err(|_| "expected a whole number with unit s, m or h, such as 2m".to_owned())?;
    let after = Duration::from_secs(count.saturating_mul(multiplier));
    if after < MIN_NOTICE_AFTER || after > MAX_NOTICE_AFTER {
        return Err(format!(
            "the notice time must be at least {}s and at most {}m",
            MIN_NOTICE_AFTER.as_secs(),
            MAX_NOTICE_AFTER.as_secs() / 60
        ));
    }
    Ok(after)
}

/// Parse `--reply-notice-tools`: a whole number of tool calls, at most [`MAX_NOTICE_TOOLS`]; `0`
/// turns mid-turn notices off.
pub(crate) fn parse_notice_tools(value: &str) -> Result<usize, String> {
    let tools: usize = value
        .trim()
        .parse()
        .map_err(|_| "expected a whole number of tool calls, such as 3, or 0 for off".to_owned())?;
    if tools > MAX_NOTICE_TOOLS {
        return Err(format!(
            "the tool-call count must be 0, which turns mid-turn notices off, or at most \
{MAX_NOTICE_TOOLS}"
        ));
    }
    Ok(tools)
}

/// Queue mid-turn `notice` for headless agent `agent_name` now, while the agent works, and say
/// whether the queue took it: queued, or set aside as possibly submitted, counts as delivered;
/// anything else queued nothing.
pub(crate) fn deliver_notice<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    runtime: &dyn crate::agent::AgentRuntime,
    agent_name: &str,
    notice: &PendingNotice,
    options: DrainOptions,
) -> Result<(), String> {
    let options = DrainOptions {
        deliver_while_working: true,
        ..options
    };
    match manager.send_identified_with_runtime(
        agent_name,
        &notice.prompt,
        options,
        &notice.message_id,
        runtime,
    ) {
        Ok(_) | Err(AgentError::Pending(_) | AgentError::PossiblySubmitted(_)) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The earliest start time [`turn_ends`] accepts for each of `eligible`.
pub(crate) fn start_bounds(eligible: &[ReminderCandidate]) -> BTreeMap<String, u64> {
    eligible
        .iter()
        .map(|candidate| {
            (
                candidate.key.clone(),
                candidate
                    .delivered_at_millis
                    .saturating_sub(START_SLACK_MILLIS),
            )
        })
        .collect()
}

/// The candidates among `eligible` whose turn ended at least `grace` before `now_millis`.
pub(crate) fn due(
    eligible: &[ReminderCandidate],
    ends: &BTreeMap<String, u64>,
    now_millis: u64,
    grace: Duration,
) -> Vec<ReminderCandidate> {
    let grace = u64::try_from(grace.as_millis()).unwrap_or(u64::MAX);
    eligible
        .iter()
        .filter(|candidate| {
            ends.get(&candidate.key)
                .is_some_and(|ended| ended.saturating_add(grace) <= now_millis)
        })
        .cloned()
        .collect()
}

/// Queue `reminder` for headless agent `agent_name` at the turn boundary and say what became of
/// it. The wrapper delivers a queued input when the agent is idle, so queued is typed; an input
/// set aside as possibly submitted is uncertain; anything else queued nothing.
pub(crate) fn deliver<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    runtime: &dyn crate::agent::AgentRuntime,
    agent_name: &str,
    reminder: &PendingReminder,
    options: DrainOptions,
) -> ReminderOutcome {
    let options = DrainOptions {
        deliver_while_working: false,
        ..options
    };
    match manager.send_identified_with_runtime(
        agent_name,
        &reminder.prompt,
        options,
        &reminder.message_id,
        runtime,
    ) {
        Ok(_) | Err(AgentError::Pending(_)) => ReminderOutcome::Typed,
        Err(AgentError::PossiblySubmitted(why)) => ReminderOutcome::Uncertain(why.message),
        Err(error) => ReminderOutcome::Withdrawn(error.to_string()),
    }
}

/// One read of the event log: when, for which requests, and what it found.
#[derive(Debug)]
struct LogRead {
    at: Instant,
    covered: BTreeSet<String>,
    /// Turn ends by request (see [`turn_ends`]).
    ends: BTreeMap<String, u64>,
    /// Still-running progress by request (see [`progress`]).
    progress: BTreeMap<String, Progress>,
}

/// What a headless check keeps between scans.
#[derive(Debug)]
pub(crate) struct HeadlessState {
    /// How long after its turn ended a request without a reply is due.
    pub(crate) grace: Duration,
    /// Least time between two reads of the event log for the same requests.
    interval: Duration,
    /// The last read of the event log, if any.
    read: Option<LogRead>,
    /// Until when no reminder is queued, after the queue refused one.
    retry_at: Option<Instant>,
    /// How many tool calls with no reply bring a mid-turn notice; 0 turns notices off (#2386).
    pub(crate) notice_tools: usize,
    /// How far past the input's start, by the event log's clock, the latest of them must come.
    pub(crate) notice_after: Duration,
    /// Until when no notice is queued, after the queue refused one.
    notice_retry_at: Option<Instant>,
    /// How long a notice the queue refused holds off the next.
    notice_retry_after: Duration,
}

impl Default for HeadlessState {
    fn default() -> Self {
        Self {
            grace: HEADLESS_REMINDER_GRACE,
            interval: LOG_READ_INTERVAL,
            read: None,
            retry_at: None,
            notice_tools: DEFAULT_NOTICE_TOOLS,
            notice_after: DEFAULT_NOTICE_AFTER,
            notice_retry_at: None,
            notice_retry_after: RETRY_AFTER_REFUSAL,
        }
    }
}

impl HeadlessState {
    /// This state with `grace` and `interval` in place of the defaults, so a test never waits.
    #[cfg(test)]
    pub(crate) fn with_timing(grace: Duration, interval: Duration) -> Self {
        Self {
            grace,
            interval,
            ..Self::default()
        }
    }

    /// These notice thresholds in place of the defaults: `tools` tool calls (0 for off), the
    /// latest at least `after` past the input's start.
    pub(crate) fn with_notices(mut self, tools: usize, after: Duration) -> Self {
        self.notice_tools = tools;
        self.notice_after = after;
        self
    }

    /// Nothing is left to remind: forget the cached turn ends and any hold.
    pub(crate) fn forget(&mut self) {
        self.read = None;
        self.retry_at = None;
        self.notice_retry_at = None;
    }

    /// Whether a refusal still holds off the next notice.
    pub(crate) fn notice_held(&self) -> bool {
        self.notice_retry_at
            .is_some_and(|until| Instant::now() < until)
    }

    /// Hold off the next notice after the queue refused one.
    pub(crate) fn notice_refused(&mut self) {
        self.notice_retry_at = Some(Instant::now() + self.notice_retry_after);
    }

    /// This state with a refused notice holding off the next for `after`, so a test never waits
    /// the real minute.
    #[cfg(test)]
    pub(crate) fn with_notice_retry(mut self, after: Duration) -> Self {
        self.notice_retry_after = after;
        self
    }

    /// The still-running progress found by the last read of the event log (see [`progress`]).
    pub(crate) fn progress(&self) -> BTreeMap<String, Progress> {
        self.read
            .as_ref()
            .map(|read| read.progress.clone())
            .unwrap_or_default()
    }

    /// Whether a refusal still holds off the next reminder.
    pub(crate) fn held(&self) -> bool {
        self.retry_at.is_some_and(|until| Instant::now() < until)
    }

    /// Hold off the next reminder after the queue refused one.
    pub(crate) fn refused(&mut self) {
        self.retry_at = Some(Instant::now() + RETRY_AFTER_REFUSAL);
    }

    /// The turn ends of the requests in `since` (see [`turn_ends`]) from the event log of headless
    /// agent `agent_name` in `registry`, read again only when a request is new to the cache or
    /// [`LOG_READ_INTERVAL`] has passed. An unreadable log is an error, for the caller to log.
    pub(crate) fn turn_ends(
        &mut self,
        registry: &Path,
        agent_name: &str,
        since: &BTreeMap<String, u64>,
    ) -> Result<BTreeMap<String, u64>, String> {
        let keys: BTreeSet<String> = since.keys().cloned().collect();
        if let Some(read) = &self.read {
            if read.at.elapsed() < self.interval && keys.is_subset(&read.covered) {
                return Ok(read.ends.clone());
            }
        }
        let directory = agent_queue::registry::directory(registry, agent_name)
            .map_err(|error| error.to_string())?;
        let log = events::read_recent(&events::directory(&directory), LOG_READ_BYTES)
            .map_err(|error| format!("the event log of {agent_name:?} cannot be read: {error}"))?;
        let ends = turn_ends(&log, since);
        self.read = Some(LogRead {
            at: Instant::now(),
            covered: keys,
            ends: ends.clone(),
            progress: progress(&log, since),
        });
        Ok(ends)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn event(seq: u64, ts: u64, kind: &str, data: Value) -> Event {
        serde_json::from_value(json!({
            "v": 1, "seq": seq, "id": format!("evt-1-{seq}"), "ts": ts, "agent": "lead",
            "generation": 1, "turn": 1, "type": kind, "data": data,
        }))
        .unwrap()
    }

    fn keys(names: &[&str]) -> BTreeMap<String, u64> {
        names.iter().map(|name| ((*name).to_owned(), 0)).collect()
    }

    #[test]
    fn progress_counts_tool_calls_after_each_start_until_the_turn_ends() {
        // #2386: tool calls count from each request's own start, by the log's timestamps.
        let log = vec![
            event(1, 100, "tool.started", json!({})),
            event(
                2,
                1_000,
                "input.started",
                json!({"queue_id": "chat-a", "mid_turn": true}),
            ),
            event(3, 2_000, "tool.started", json!({})),
            event(
                4,
                60_000,
                "input.started",
                json!({"queue_id": "chat-b", "mid_turn": true}),
            ),
            event(5, 61_000, "tool.started", json!({})),
            event(6, 61_500, "assistant.text", json!({"text": "working"})),
            event(7, 121_000, "tool.started", json!({})),
        ];
        let found = progress(&log, &keys(&["a", "b", "c"]));
        assert_eq!(
            found["a"],
            Progress {
                started: 1_000,
                tool_calls: 3,
                last_tool: Some(121_000)
            }
        );
        assert_eq!(
            found["b"],
            Progress {
                started: 60_000,
                tool_calls: 2,
                last_tool: Some(121_000)
            }
        );
        assert!(!found.contains_key("c"), "never started");
        // A turn end, or a harness exit, closes them all.
        for end in ["turn.completed", "lifecycle.harness_exited"] {
            let mut ended = log.clone();
            ended.push(event(8, 130_000, end, json!({})));
            assert!(progress(&ended, &keys(&["a", "b"])).is_empty(), "{end}");
        }
        // An input dropped before the model saw it does not count.
        let mut dropped = log.clone();
        dropped.push(event(
            8,
            122_000,
            "input.cancelled",
            json!({"queue_id": "chat-b", "before_start": true}),
        ));
        assert_eq!(
            progress(&dropped, &keys(&["a", "b"]))
                .keys()
                .collect::<Vec<_>>(),
            ["a"]
        );
    }

    #[test]
    fn a_notice_is_due_after_enough_tool_calls_spread_over_enough_log_time() {
        let candidate = |key: &str| ReminderCandidate {
            key: key.to_owned(),
            reply_id: format!("id-{key}"),
            delivered_at_millis: 0,
        };
        let eligible = ["a", "b", "c", "d"].map(candidate);
        let at = |started: u64, tool_calls: usize, last_tool: Option<u64>| Progress {
            started,
            tool_calls,
            last_tool,
        };
        let progress = BTreeMap::from([
            ("a".to_owned(), at(1_000, 3, Some(121_000))),
            ("b".to_owned(), at(1_000, 2, Some(900_000))),
            ("c".to_owned(), at(1_000, 9, Some(120_999))),
            ("d".to_owned(), at(1_000, 0, None)),
        ]);
        let after = Duration::from_secs(120);
        let (due, most) = notice_due(&eligible, &progress, 3, after);
        assert_eq!(
            due.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            ["a"],
            "b has too few tool calls, c's last is 1 ms early by the log, d has none"
        );
        assert_eq!(most, 3);
        assert!(
            notice_due(&eligible, &progress, 0, after).0.is_empty(),
            "0 is off"
        );
        let (due, most) = notice_due(&eligible, &progress, 2, after);
        assert_eq!(
            due.iter().map(|c| c.key.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(most, 3);
    }

    #[test]
    fn the_notice_settings_parse_within_their_bounds() {
        assert_eq!(parse_notice_tools("3"), Ok(3));
        assert_eq!(parse_notice_tools("0"), Ok(0));
        assert_eq!(parse_notice_tools("100"), Ok(100));
        for bad in ["101", "x", "-1", ""] {
            assert!(parse_notice_tools(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_notice_after("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_notice_after("30"), Ok(Duration::from_secs(30)));
        assert_eq!(parse_notice_after("1h"), Ok(Duration::from_secs(3_600)));
        for bad in ["29s", "61m", "2h", "2d", "soon", "0"] {
            assert!(parse_notice_after(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_turn_end_is_the_first_completion_after_the_input_started() {
        let log = vec![
            event(1, 100, "input.started", json!({"queue_id": "chat-alone"})),
            event(2, 150, "assistant.text", json!({"text": "working"})),
            event(3, 200, "turn.completed", json!({"outcome": "completed"})),
            // Batched with another input at the start of a turn.
            event(4, 300, "input.started", json!({"queue_id": "tm-other"})),
            event(
                5,
                300,
                "input.started",
                json!({"queue_id": "chat-batched", "batched": true}),
            ),
            // Started mid-turn, between tool calls.
            event(
                6,
                350,
                "input.started",
                json!({"queue_id": "chat-midturn", "mid_turn": true}),
            ),
            event(7, 400, "turn.completed", json!({"outcome": "completed"})),
            // A later turn does not move an end already found.
            event(8, 500, "input.started", json!({"queue_id": "tm-next"})),
            event(9, 600, "turn.completed", json!({"outcome": "completed"})),
        ];
        let ends = turn_ends(&log, &keys(&["alone", "batched", "midturn", "absent"]));
        assert_eq!(ends.get("alone"), Some(&200));
        assert_eq!(ends.get("batched"), Some(&400));
        assert_eq!(ends.get("midturn"), Some(&400));
        assert_eq!(ends.get("absent"), None);
    }

    #[test]
    fn an_open_turn_a_dropped_input_and_a_harness_exit_are_judged_correctly() {
        let log = vec![
            event(1, 100, "input.started", json!({"queue_id": "chat-dropped"})),
            event(
                2,
                110,
                "input.cancelled",
                json!({"queue_id": "chat-dropped", "before_start": true}),
            ),
            event(3, 120, "turn.completed", json!({"outcome": "interrupted"})),
            event(4, 200, "input.started", json!({"queue_id": "chat-exited"})),
            event(5, 250, "lifecycle.harness_exited", json!({"code": 1})),
            event(6, 300, "input.started", json!({"queue_id": "chat-open"})),
            event(7, 320, "assistant.text", json!({"text": "still going"})),
        ];
        let ends = turn_ends(&log, &keys(&["dropped", "exited", "open"]));
        assert_eq!(
            ends.get("dropped"),
            None,
            "back in the queue, not answered yet"
        );
        assert_eq!(ends.get("exited"), Some(&250));
        assert_eq!(ends.get("open"), None, "its turn is still running");
        // An input cancelled after the model saw it still ended with its turn.
        let log = vec![
            event(1, 100, "input.started", json!({"queue_id": "chat-seen"})),
            event(
                2,
                110,
                "input.cancelled",
                json!({"queue_id": "chat-seen", "before_start": false}),
            ),
            event(3, 120, "turn.completed", json!({"outcome": "interrupted"})),
        ];
        assert_eq!(turn_ends(&log, &keys(&["seen"])).get("seen"), Some(&120));
    }

    #[test]
    fn the_latest_start_counts_and_starts_before_the_bound_are_ignored() {
        let log = vec![
            // An older run of the same queue item, long before this delivery.
            event(1, 100, "input.started", json!({"queue_id": "chat-again"})),
            event(2, 200, "turn.completed", json!({"outcome": "completed"})),
            // Delivered again: started, its turn still running.
            event(
                3,
                10_000,
                "input.started",
                json!({"queue_id": "chat-again"}),
            ),
            event(4, 10_100, "assistant.text", json!({"text": "noted"})),
        ];
        let mut since = BTreeMap::from([("again".to_owned(), 0)]);
        assert_eq!(
            turn_ends(&log, &since).get("again"),
            None,
            "the latest start is open"
        );
        since.insert("again".to_owned(), 5_000);
        assert_eq!(turn_ends(&log, &since).get("again"), None);
        let mut log = log;
        log.push(event(
            5,
            10_200,
            "turn.completed",
            json!({"outcome": "completed"}),
        ));
        assert_eq!(turn_ends(&log, &since).get("again"), Some(&10_200));
        let bounds = start_bounds(&[ReminderCandidate {
            key: "again".to_owned(),
            reply_id: "001".to_owned(),
            delivered_at_millis: 20 * 60 * 1000,
        }]);
        assert_eq!(bounds.get("again"), Some(&(10 * 60 * 1000)));
    }

    #[test]
    fn a_request_is_due_only_after_the_grace() {
        let candidate = |key: &str| ReminderCandidate {
            key: key.to_owned(),
            reply_id: format!("{key}-id"),
            delivered_at_millis: 1,
        };
        let eligible = vec![candidate("ended"), candidate("recent"), candidate("open")];
        let ends = BTreeMap::from([("ended".to_owned(), 1_000), ("recent".to_owned(), 15_000)]);
        let due = due(&eligible, &ends, 30_000, HEADLESS_REMINDER_GRACE);
        assert_eq!(
            due.iter()
                .map(|candidate| candidate.key.as_str())
                .collect::<Vec<_>>(),
            vec!["ended"]
        );
    }

    #[test]
    fn the_log_is_read_again_only_for_new_requests_or_after_the_interval() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-headless-reminders-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut state = HeadlessState::default();
        // No log yet: no turn has ended.
        assert!(state
            .turn_ends(&registry, "lead", &keys(&["a"]))
            .unwrap_or_default()
            .is_empty());
        // A cached read covers the same keys within the interval without touching the disk.
        state.read = Some(LogRead {
            at: Instant::now(),
            covered: keys(&["a", "b"]).into_keys().collect(),
            ends: BTreeMap::from([("a".to_owned(), 5)]),
            progress: BTreeMap::new(),
        });
        assert_eq!(
            state.turn_ends(&registry, "lead", &keys(&["a"])).unwrap(),
            BTreeMap::from([("a".to_owned(), 5)])
        );
        // A key the cache does not cover reads the log again, which holds nothing here.
        assert!(state
            .turn_ends(&registry, "lead", &keys(&["c"]))
            .unwrap_or_default()
            .is_empty());
        assert!(!state.held());
        state.refused();
        assert!(state.held());
        state.forget();
        assert!(!state.held() && state.read.is_none());
    }
}
