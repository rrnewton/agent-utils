//! What `agentctl watch-agents` knows about a headless agent (`#589 headless-watch`).
//!
//! A headless agent has no screen to read: its status comes from the wrapper (through the
//! record's status row) and from the tail of its event log. This module reduces the newest
//! events to a [`View`]: whether a turn is open and since when, when the log last moved, the
//! tool running now, and the last prose of the last finished turn, where a verdict
//! (`DONE:` / `BLOCKED:`) is read. The watcher's episode logic does the rest.

use std::path::Path;

use serde::Serialize;

use super::events::{self, Event};

/// Bytes read from the end of the log each pass: enough for the last turns of a busy agent
/// without reading a whole segment.
pub const RECENT_BYTES: u64 = 512 * 1024;

/// A headless agent as the event log shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct View {
    /// `seq` of the newest event, 0 when the log is empty.
    pub last_seq: u64,
    /// When the newest event was written, in Unix seconds.
    pub last_event_at: Option<f64>,
    /// The newest turn number.
    pub turn: u64,
    /// When the open turn began, in Unix seconds; `None` when no turn is open.
    pub busy_since: Option<f64>,
    /// When the last turn ended, in Unix seconds.
    pub turn_completed_at: Option<f64>,
    /// How the last turn ended (`completed`, `interrupted`, `error`).
    pub last_outcome: Option<String>,
    /// The prose of the last finished turn, last message last.
    pub last_text: Vec<String>,
    /// The tool call running now: tool, summary, and when it started (Unix seconds).
    pub open_tool: Option<(String, String, f64)>,
    /// The wrapper is restarting a harness that exited (`status: unreachable`).
    pub restarting: bool,
    /// The newest lifecycle event (`lifecycle.takeover`, `lifecycle.stopped`, ...).
    pub lifecycle: Option<String>,
}

fn seconds(ts: u64) -> f64 {
    ts as f64 / 1000.0
}

/// The view the events (oldest first) give.
#[must_use]
pub fn view(events: &[Event]) -> View {
    let mut view = View::default();
    let mut text: Vec<String> = Vec::new();
    for event in events {
        view.last_seq = event.seq;
        view.last_event_at = Some(seconds(event.ts));
        view.turn = view.turn.max(event.turn);
        match event.kind.as_str() {
            "turn.started" => {
                view.busy_since = Some(seconds(event.ts));
                view.open_tool = None;
                text.clear();
            }
            "turn.completed" => {
                view.busy_since = None;
                view.open_tool = None;
                view.turn_completed_at = Some(seconds(event.ts));
                view.last_outcome = event.data["outcome"].as_str().map(str::to_owned);
                view.last_text = std::mem::take(&mut text);
            }
            "assistant.text" => {
                if let Some(prose) = event.data["text"].as_str() {
                    text.push(prose.to_owned());
                }
            }
            "tool.started" => {
                view.open_tool = Some((
                    event.data["tool"].as_str().unwrap_or("").to_owned(),
                    event.data["summary"].as_str().unwrap_or("").to_owned(),
                    seconds(event.ts),
                ));
            }
            "tool.completed" => view.open_tool = None,
            "status" => {
                view.restarting = event.data["status"] == "unreachable";
            }
            kind if kind.starts_with("lifecycle.") => {
                view.lifecycle = Some(kind.to_owned());
                // A harness that exited ends any open turn; the wrapper restarts it or stops.
                if matches!(
                    kind,
                    "lifecycle.harness_exited" | "lifecycle.stopped" | "lifecycle.started"
                ) {
                    view.busy_since = None;
                    view.open_tool = None;
                }
            }
            _ => {}
        }
    }
    view
}

/// The view of the log in `dir`, from its newest [`RECENT_BYTES`].
pub fn read(dir: &Path) -> std::io::Result<View> {
    Ok(view(&events::read_recent(dir, RECENT_BYTES)?))
}

/// The verdict of the last finished turn: the first line of its last message that has one,
/// searching from the last message back, that starts with one of `markers`.
#[must_use]
pub fn verdict(view: &View, markers: &[String]) -> Option<String> {
    view.last_text.iter().rev().find_map(|message| {
        message.lines().find_map(|line| {
            let line = line.trim_start().trim_start_matches(['*', '_', '#', ' ']);
            markers
                .iter()
                .any(|marker| line.starts_with(marker.as_str()))
                .then(|| line.trim_end().chars().take(200).collect())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::events::EventWriter;
    use serde_json::json;

    #[test]
    fn the_view_follows_turns_tools_and_lifecycle() {
        let dir = std::env::temp_dir().join(format!(
            "agentctl-hwatch-{}-{}",
            std::process::id(),
            events::now_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("events");
        let mut writer = EventWriter::open(&log, "a").unwrap();
        writer.begin_turn();
        writer
            .append("turn.started", None, json!({"cause": "input"}))
            .unwrap();
        writer
            .append(
                "assistant.text",
                None,
                json!({"text": "BLOCKED: need a decision"}),
            )
            .unwrap();
        writer
            .append("turn.completed", None, json!({"outcome": "completed"}))
            .unwrap();
        writer.begin_turn();
        writer
            .append("turn.started", None, json!({"cause": "input"}))
            .unwrap();
        writer
            .append(
                "tool.started",
                None,
                json!({"tool": "Bash", "summary": "sleep 9"}),
            )
            .unwrap();
        let busy = read(&log).unwrap();
        assert_eq!(busy.turn, 2);
        assert!(busy.busy_since.is_some());
        assert_eq!(
            busy.open_tool.as_ref().map(|tool| tool.1.as_str()),
            Some("sleep 9")
        );
        let markers = vec!["DONE:".to_owned(), "BLOCKED:".to_owned()];
        assert_eq!(
            verdict(&busy, &markers).as_deref(),
            Some("BLOCKED: need a decision")
        );
        writer
            .append("tool.completed", None, json!({"ok": true}))
            .unwrap();
        writer
            .append("assistant.text", None, json!({"text": "Still going."}))
            .unwrap();
        writer
            .append("turn.completed", None, json!({"outcome": "interrupted"}))
            .unwrap();
        let idle = read(&log).unwrap();
        assert_eq!(idle.busy_since, None);
        assert_eq!(idle.open_tool, None);
        assert_eq!(idle.last_outcome.as_deref(), Some("interrupted"));
        assert_eq!(
            verdict(&idle, &markers),
            None,
            "an older turn's verdict does not count"
        );
        assert_eq!(idle.last_seq, 8);
        // A harness that exits inside a turn closes it.
        writer.begin_turn();
        writer.append("turn.started", None, json!({})).unwrap();
        writer
            .append("lifecycle.harness_exited", None, json!({"code": 1}))
            .unwrap();
        assert_eq!(read(&log).unwrap().busy_since, None);
        // The recent window skips a line it cuts and still sees the newest events.
        let recent = events::read_recent(&log, 300).unwrap();
        assert!(
            !recent.is_empty() && recent.last().unwrap().seq == 10,
            "{recent:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
