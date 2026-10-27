//! `agentctl tail NAME`: a read-only follower of a headless agent's event log.
//!
//! It takes no lock and writes nothing, so any number of copies can run, and closing one never
//! touches the agent. The default rendering is one line per event (prose wrapped), so a Herdr tab
//! running `agentctl tail NAME --follow` is a view of the agent with nothing folded or redrawn.
//! It is unrelated to `agentctl read --output tail`, which reads an interactive agent's pane.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;
use unicode_width::UnicodeWidthChar;

use super::events::{self, Event, Follower};

/// What `tail` prints and whether it keeps following.
#[derive(Clone, Debug)]
pub struct TailOptions {
    /// Keep printing new events until interrupted.
    pub follow: bool,
    /// Print only events after this `seq`.
    pub since: Option<u64>,
    /// Print only the last this many (matching) events before following.
    pub last: Option<usize>,
    /// Print events verbatim as JSON lines instead of rendering them.
    pub json: bool,
    /// Print only these event types; a trailing `.` matches a prefix (`tool.`).
    pub types: Vec<String>,
    /// Columns to wrap prose at.
    pub width: usize,
    /// Print the event with this `seq` and the rest of its turn, then exit.
    pub at: Option<u64>,
    /// How often `--follow` looks for new events.
    pub poll: Duration,
}

impl Default for TailOptions {
    fn default() -> Self {
        Self {
            follow: false,
            since: None,
            last: None,
            json: false,
            types: Vec::new(),
            width: 100,
            at: None,
            poll: Duration::from_millis(250),
        }
    }
}

fn type_matches(types: &[String], kind: &str) -> bool {
    types.is_empty()
        || types.iter().any(|wanted| {
            if wanted.ends_with('.') {
                kind.starts_with(wanted.as_str())
            } else {
                kind == wanted
            }
        })
}

/// Local wall-clock `HH:MM:SS` of a Unix time in milliseconds.
fn clock(ts: u64) -> String {
    let seconds = (ts / 1000) as libc::time_t;
    // SAFETY: localtime_r writes only into the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&seconds, &mut tm) };
    if converted.is_null() {
        return "--:--:--".to_owned();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}

/// `text` wrapped to `width` display columns, every line after the first indented by `indent`.
fn wrap(text: &str, width: usize, first: &str, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut prefix = first;
    for paragraph in text.split('\n') {
        let mut line = prefix.to_owned();
        let mut columns = display_width(&line);
        let mut empty = true;
        for word in paragraph.split(' ').filter(|word| !word.is_empty()) {
            let word_width = display_width(word);
            if !empty && columns + 1 + word_width > width {
                lines.push(std::mem::replace(&mut line, indent.to_owned()));
                columns = display_width(indent);
                empty = true;
            }
            if !empty {
                line.push(' ');
                columns += 1;
            }
            line.push_str(word);
            columns += word_width;
            empty = false;
        }
        lines.push(line.trim_end().to_owned());
        prefix = indent;
    }
    lines
}

fn display_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

fn rule(label: &str, width: usize) -> String {
    let used = display_width(label) + 4;
    let fill = width.saturating_sub(used).max(3);
    format!("── {label} {}", "─".repeat(fill))
}

fn seconds(milliseconds: Option<u64>) -> String {
    match milliseconds {
        Some(ms) if ms < 10_000 => format!("{:.1} s", ms as f64 / 1000.0),
        Some(ms) => format!("{} s", ms / 1000),
        None => "?".to_owned(),
    }
}

fn text_of<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The human rendering of one event: zero or more lines.
#[must_use]
pub fn render(event: &Event, width: usize) -> Vec<String> {
    let data = &event.data;
    let time = clock(event.ts);
    let head = format!("{time} ");
    let pad = " ".repeat(head.len());
    match event.kind.as_str() {
        "lifecycle.started" => vec![format!(
            "{head}══ wrapper started · generation {} · {} · session {} ══",
            event.generation,
            [text_of(data, "harness"), text_of(data, "harness_version")]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" "),
            event.session.as_deref().unwrap_or("?"),
        )],
        "lifecycle.harness_spawned" => vec![format!(
            "{head}   harness started (pid {}){}",
            data["pid"],
            if data["resume"] == true {
                ", resuming the session"
            } else {
                ", new session"
            }
        )],
        "interrupt.requested" => vec![format!("{head}!! interrupt requested")],
        "lifecycle.stopped" => vec![format!(
            "{head}══ wrapper stopped: {} ══",
            text_of(data, "cause")
        )],
        "lifecycle.harness" => vec![format!(
            "{head}   harness ready: {} {}, model {}, permission mode {}",
            text_of(data, "harness"),
            text_of(data, "version"),
            text_of(data, "model"),
            text_of(data, "permission_mode"),
        )],
        "lifecycle.harness_exited" => vec![format!(
            "{head}!! harness exited ({}); {}",
            data.get("code")
                .map_or_else(|| "signal".to_owned(), Value::to_string),
            text_of(data, "next"),
        )],
        "lifecycle.takeover" => vec![format!(
            "{head}══ taken over by an interactive terminal; this log has no events until it is released ══"
        )],
        "lifecycle.released" => vec![format!("{head}══ released back to headless ══")],
        "status" => vec![format!("{head}   · {}", text_of(data, "status"))],
        "input.queued" => {
            let label = format!(
                "{head}← {} ({}, {}) [{}]: ",
                text_of(data, "from"),
                text_of(data, "deliver"),
                text_of(data, "priority"),
                text_of(data, "queue_id"),
            );
            wrap(text_of(data, "excerpt"), width, &label, &format!("{pad}  "))
        }
        "input.started" => vec![format!(
            "{head}  ↳ started [{}]{}",
            text_of(data, "queue_id"),
            if data["mid_turn"] == true {
                " mid-turn"
            } else {
                ""
            }
        )],
        "input.completed" => Vec::new(),
        "input.cancelled" => vec![format!(
            "{head}  ↳ cancelled [{}] {}",
            text_of(data, "queue_id"),
            if data["before_start"] == true {
                "before the model saw it; returned to the queue"
            } else {
                "after the model saw it"
            }
        )],
        "turn.started" => vec![format!(
            "{head}{}",
            rule(
                &format!("turn {} ({})", event.turn, text_of(data, "cause")),
                width.saturating_sub(head.len())
            )
        )],
        "turn.completed" => {
            let usage = &data["usage"];
            let tokens = format!(
                "in {} out {} cache r {} w {}",
                usage["input_tokens"].as_u64().unwrap_or(0),
                usage["output_tokens"].as_u64().unwrap_or(0),
                usage["cache_read_input_tokens"].as_u64().unwrap_or(0),
                usage["cache_creation_input_tokens"].as_u64().unwrap_or(0),
            );
            let cost = data["cost_usd"]
                .as_f64()
                .filter(|cost| *cost >= 0.00005)
                .map_or_else(String::new, |cost| format!(" · ${cost:.4}"));
            vec![format!(
                "{head}{}",
                rule(
                    &format!(
                        "turn {} {} · {} · {tokens}{cost}",
                        event.turn,
                        text_of(data, "outcome"),
                        seconds(data["duration_ms"].as_u64()),
                    ),
                    width.saturating_sub(head.len())
                )
            )]
        }
        "assistant.text" => wrap(
            text_of(data, "text"),
            width,
            &format!("{head}● "),
            &format!("{pad}  "),
        ),
        "tool.started" => vec![format!(
            "{head}$ {}  {}",
            text_of(data, "tool"),
            text_of(data, "summary")
        )],
        "tool.completed" => vec![format!(
            "{head}  {} {}{}",
            if data["ok"] == false { "✗" } else { "✓" },
            seconds(data["duration_ms"].as_u64()),
            data["output_bytes"]
                .as_u64()
                .map_or_else(String::new, |bytes| format!(" · {bytes} B")),
        )],
        "permission.denied" => vec![format!(
            "{head}⛔ denied {}  {}",
            text_of(data, "tool"),
            text_of(data, "summary")
        )],
        "compact.started" => vec![format!("{head}⋯ compacting")],
        "compact.completed" => vec![format!(
            "{head}⋯ compacted ({}): {} → {} tokens",
            text_of(data, "trigger"),
            data["pre_tokens"].as_u64().unwrap_or(0),
            data["post_tokens"].as_u64().unwrap_or(0),
        )],
        "error" => vec![format!(
            "{head}!! {}: {}",
            text_of(data, "class"),
            text_of(data, "message")
        )],
        other => vec![format!(
            "{head}· {other} {}",
            events::excerpt(&data.to_string(), 160)
        )],
    }
}

/// Prints events with the gap lines between them.
struct Printer<'a> {
    options: &'a TailOptions,
    out: &'a mut dyn Write,
    last_seq: Option<u64>,
}

impl Printer<'_> {
    /// Print `event` (if its type is selected), first noting any hole before it.
    fn event(&mut self, event: &Event) -> io::Result<()> {
        if let Some(previous) = self.last_seq {
            if event.seq > previous + 1 {
                self.gap(previous + 1, event.seq - 1)?;
            }
        }
        self.last_seq = Some(self.last_seq.map_or(event.seq, |seq| seq.max(event.seq)));
        if !type_matches(&self.options.types, &event.kind) {
            return Ok(());
        }
        if self.options.json {
            serde_json::to_writer(&mut *self.out, event)?;
            writeln!(self.out)?;
        } else {
            for line in render(event, self.options.width) {
                writeln!(self.out, "{line}")?;
            }
        }
        self.out.flush()
    }

    fn gap(&mut self, first: u64, last: u64) -> io::Result<()> {
        if self.options.json {
            writeln!(
                self.out,
                "{}",
                serde_json::json!({"gap": {"first_seq": first, "last_seq": last}})
            )
        } else if first == last {
            writeln!(
                self.out,
                "… event {first} is missing (pruned or unreadable)"
            )
        } else {
            writeln!(
                self.out,
                "… events {first}-{last} are missing (pruned or unreadable)"
            )
        }
    }
}

/// Run `tail` over the log in `dir`, writing to `out`. With `follow`, poll until `stop` returns
/// true. Returns `false` when `--at` named an event that does not exist.
pub fn run(
    dir: &Path,
    options: &TailOptions,
    out: &mut dyn Write,
    stop: &dyn Fn() -> bool,
) -> io::Result<bool> {
    let mut follower = Follower::new(dir);
    let initial = follower.poll()?;
    if let Some(target) = options.at {
        let Some(found) = initial.iter().find(|event| event.seq == target) else {
            return Ok(false);
        };
        for event in initial.iter().filter(|event| event.turn == found.turn) {
            let marker = if event.seq == target { "▶ " } else { "  " };
            if options.json {
                serde_json::to_writer(&mut *out, event)?;
                writeln!(out)?;
            } else {
                for line in render(event, options.width) {
                    writeln!(out, "{marker}{line}")?;
                }
            }
        }
        out.flush()?;
        return Ok(true);
    }
    let mut selected: Vec<&Event> = initial
        .iter()
        .filter(|event| options.since.is_none_or(|since| event.seq > since))
        .collect();
    let mut printer = Printer {
        options,
        out,
        last_seq: None,
    };
    if let Some(since) = options.since {
        printer.last_seq = Some(since);
    }
    if let Some(last) = options.last {
        let matching: Vec<usize> = selected
            .iter()
            .enumerate()
            .filter(|(_, event)| type_matches(&options.types, &event.kind))
            .map(|(index, _)| index)
            .collect();
        if matching.len() > last {
            let start = matching[matching.len() - last];
            selected.drain(..start);
            printer.last_seq = selected.first().map(|event| event.seq.saturating_sub(1));
        }
    }
    for event in selected {
        printer.event(event)?;
    }
    while options.follow && !stop() {
        std::thread::sleep(options.poll);
        for event in follower.poll()? {
            if options.since.is_none_or(|since| event.seq > since) {
                printer.event(&event)?;
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::headless::events::EventWriter;
    use serde_json::json;
    use std::fs;

    fn sample(dir: &Path) {
        let mut writer = EventWriter::open(dir, "coordinator").unwrap();
        writer.set_session(Some("s-1".to_owned()));
        writer
            .append(
                "lifecycle.started",
                None,
                json!({"harness": "claude", "harness_version": "2.1.310"}),
            )
            .unwrap();
        writer
            .append(
                "input.queued",
                Some("q1"),
                json!({"queue_id": "q1", "from": "owner", "deliver": "now", "priority": "urgent", "excerpt": "status please"}),
            )
            .unwrap();
        writer.begin_turn();
        writer
            .append("turn.started", Some("q1"), json!({"cause": "input"}))
            .unwrap();
        writer
            .append(
                "tool.started",
                Some("q1"),
                json!({"tool": "Bash", "summary": "git fetch origin", "tool_use_id": "t1"}),
            )
            .unwrap();
        writer
            .append(
                "tool.completed",
                Some("q1"),
                json!({"tool": "Bash", "tool_use_id": "t1", "ok": true, "duration_ms": 1200, "output_bytes": 34}),
            )
            .unwrap();
        writer
            .append("assistant.text", Some("q1"), json!({"text": "All green."}))
            .unwrap();
        writer
            .append(
                "turn.completed",
                Some("q1"),
                json!({"outcome": "completed", "duration_ms": 3400, "cost_usd": 0.0123,
                       "usage": {"input_tokens": 2, "output_tokens": 50}}),
            )
            .unwrap();
    }

    fn scratch(label: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-tail-{label}-{}-{}",
            std::process::id(),
            events::now_millis()
        ));
        fs::create_dir_all(&path).unwrap();
        path.join("events")
    }

    fn output(dir: &Path, options: &TailOptions) -> (bool, String) {
        let mut buffer = Vec::new();
        let found = run(dir, options, &mut buffer, &|| true).unwrap();
        (found, String::from_utf8(buffer).unwrap())
    }

    #[test]
    fn renders_one_line_per_event_with_rules_and_tools() {
        let dir = scratch("render");
        sample(&dir);
        let (_, text) = output(&dir, &TailOptions::default());
        assert!(
            text.contains("wrapper started · generation 1 · claude 2.1.310 · session s-1"),
            "{text}"
        );
        assert!(
            text.contains("← owner (now, urgent) [q1]: status please"),
            "{text}"
        );
        assert!(text.contains("── turn 1 (input) ─"), "{text}");
        assert!(text.contains("$ Bash  git fetch origin"), "{text}");
        assert!(text.contains("✓ 1.2 s · 34 B"), "{text}");
        assert!(text.contains("● All green."), "{text}");
        assert!(
            text.contains("turn 1 completed · 3.4 s · in 2 out 50 cache r 0 w 0 · $0.0123"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 7, "{text}");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn json_types_last_and_since_select_events() {
        let dir = scratch("select");
        sample(&dir);
        let options = TailOptions {
            json: true,
            types: vec!["tool.".to_owned()],
            ..TailOptions::default()
        };
        let (_, text) = output(&dir, &options);
        let kinds: Vec<String> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["type"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(kinds, vec!["tool.started", "tool.completed"]);
        let options = TailOptions {
            json: true,
            last: Some(2),
            ..TailOptions::default()
        };
        let (_, text) = output(&dir, &options);
        let seqs: Vec<u64> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["seq"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        assert_eq!(seqs, vec![6, 7]);
        let options = TailOptions {
            json: true,
            since: Some(5),
            ..TailOptions::default()
        };
        let (_, text) = output(&dir, &options);
        assert_eq!(text.lines().count(), 2, "{text}");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn holes_and_pruned_ranges_print_gap_lines() {
        let dir = scratch("gap");
        sample(&dir);
        // Remove the middle event's line to make a hole at seq 4.
        let path = dir.join("1.jsonl");
        let text = fs::read_to_string(&path).unwrap();
        let kept: Vec<&str> = text
            .lines()
            .filter(|line| !line.contains("\"seq\":4,"))
            .collect();
        fs::write(&path, format!("{}\n", kept.join("\n"))).unwrap();
        let (_, text) = output(&dir, &TailOptions::default());
        assert!(text.contains("… event 4 is missing"), "{text}");
        let (_, text) = output(
            &dir,
            &TailOptions {
                since: Some(0),
                ..TailOptions::default()
            },
        );
        assert!(!text.contains("events 1-"), "{text}");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn at_prints_the_turn_of_one_event_and_reports_a_missing_one() {
        let dir = scratch("at");
        sample(&dir);
        let (found, text) = output(
            &dir,
            &TailOptions {
                at: Some(4),
                ..TailOptions::default()
            },
        );
        assert!(found);
        assert!(text.contains("▶ "), "{text}");
        assert!(text.contains("$ Bash"), "{text}");
        assert!(!text.contains("wrapper started"), "{text}");
        let (found, _) = output(
            &dir,
            &TailOptions {
                at: Some(99),
                ..TailOptions::default()
            },
        );
        assert!(!found);
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn wrap_breaks_long_prose_at_the_width() {
        let lines = wrap("one two three four five six", 12, "● ", "  ");
        assert!(lines.len() > 1, "{lines:?}");
        assert!(
            lines.iter().all(|line| display_width(line) <= 12),
            "{lines:?}"
        );
        assert!(lines[0].starts_with("● one"), "{lines:?}");
    }
}
