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

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::agent::{AgentError, DrainOptions};
use crate::chat_runtime::{PendingReminder, ReminderCandidate, ReminderOutcome};
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

/// What a headless check keeps between scans.
#[derive(Debug)]
pub(crate) struct HeadlessState {
    /// How long after its turn ended a request without a reply is due.
    pub(crate) grace: Duration,
    /// Least time between two reads of the event log for the same requests.
    interval: Duration,
    /// When the event log was last read, and the turn ends found then.
    read: Option<(Instant, BTreeSet<String>, BTreeMap<String, u64>)>,
    /// Until when no reminder is queued, after the queue refused one.
    retry_at: Option<Instant>,
}

impl Default for HeadlessState {
    fn default() -> Self {
        Self {
            grace: HEADLESS_REMINDER_GRACE,
            interval: LOG_READ_INTERVAL,
            read: None,
            retry_at: None,
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

    /// Nothing is left to remind: forget the cached turn ends and any hold.
    pub(crate) fn forget(&mut self) {
        self.read = None;
        self.retry_at = None;
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
        if let Some((at, covered, ends)) = &self.read {
            if at.elapsed() < self.interval && keys.is_subset(covered) {
                return Ok(ends.clone());
            }
        }
        let directory = agent_queue::registry::directory(registry, agent_name)
            .map_err(|error| error.to_string())?;
        let log = events::read_recent(&events::directory(&directory), LOG_READ_BYTES)
            .map_err(|error| format!("the event log of {agent_name:?} cannot be read: {error}"))?;
        let ends = turn_ends(&log, since);
        self.read = Some((Instant::now(), keys, ends.clone()));
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
        state.read = Some((
            Instant::now(),
            keys(&["a", "b"]).into_keys().collect(),
            BTreeMap::from([("a".to_owned(), 5)]),
        ));
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
