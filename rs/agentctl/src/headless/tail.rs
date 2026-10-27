//! `agentctl tail NAME`: a read-only follower of a headless agent's event log.
//!
//! It takes no lock and writes nothing, so any number of copies can run, and closing one never
//! touches the agent. Each event renders as a header line, with the agent's prose, inputs and
//! replies drawn as Markdown blocks beneath it ([`super::render`]) and long tool output folded;
//! nothing is redrawn, so a Herdr tab running `agentctl tail NAME --follow` is a scrolling view of
//! the agent. On a terminal it is colored and follows the pane's width; `--plain` (or `NO_COLOR`)
//! prints the same layout without colors. It is unrelated to `agentctl read --output tail`, which
//! reads an interactive agent's pane.

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use super::events::{self, Event, Follower};
use super::render::{self, Frame, Line, Span, Style, Theme};

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
    /// Columns to wrap prose at (when `auto_width` finds no terminal).
    pub width: usize,
    /// Follow the terminal's width, read again before every event, so a resized pane rewraps.
    pub auto_width: bool,
    /// Colors and Markdown styling; false prints plain text with the Markdown markers kept.
    pub pretty: bool,
    /// Lines of a tool result preview shown before the rest is folded.
    pub tool_lines: usize,
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
            auto_width: false,
            pretty: false,
            tool_lines: DEFAULT_TOOL_LINES,
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

fn seconds(milliseconds: Option<u64>) -> String {
    match milliseconds {
        Some(ms) if ms < 10_000 => format!("{:.1} s", ms as f64 / 1000.0),
        Some(ms) => format!("{} s", ms / 1000),
        None => "?".to_owned(),
    }
}

/// A token count, short: `950`, `9999`, `48k`, `1.2M`.
fn tokens(count: u64) -> String {
    match count {
        0..10_000 => count.to_string(),
        10_000..1_000_000 => format!("{}k", (count + 500) / 1000),
        _ => format!("{:.1}M", count as f64 / 1_000_000.0),
    }
}

fn text_of<'a>(data: &'a Value, key: &str) -> &'a str {
    data.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The colors of each kind of event.
mod palette {
    use super::render::{Color, Style};

    pub const TIME: Style = Style::dim();
    pub const LIFECYCLE: Style = Style::fg(Color::Cyan);
    pub const STATUS: Style = Style::fg(Color::Gray);
    pub const INPUT: Style = Style::fg(Color::Cyan).bold();
    pub const INPUT_BAR: Style = Style::fg(Color::Cyan);
    pub const STARTED: Style = Style::fg(Color::Blue);
    pub const ASSISTANT: Style = Style::fg(Color::Green).bold();
    pub const TOOL: Style = Style::fg(Color::Yellow).bold();
    pub const TOOL_TEXT: Style = Style::fg(Color::Yellow);
    pub const OK: Style = Style::fg(Color::Green);
    pub const FAIL: Style = Style::fg(Color::BrightRed).bold();
    pub const OUTPUT: Style = Style::fg(Color::Gray);
    pub const REPLY: Style = Style::fg(Color::Magenta).bold();
    pub const REPLY_BAR: Style = Style::fg(Color::Magenta);
    pub const INTERRUPT: Style = Style::fg(Color::BrightYellow).bold();
    pub const ERROR: Style = Style::fg(Color::BrightRed).bold();
    pub const TURN: Style = Style::fg(Color::White).bold();
    pub const TURN_RULE: Style = Style::fg(Color::Blue).bold();
    pub const DETAIL: Style = Style::dim();
}

/// What rendering one event needs besides the event.
pub struct Context<'a> {
    /// Total columns.
    pub width: usize,
    /// Style text with colors and Markdown emphasis (false: plain text, markers kept).
    pub pretty: bool,
    /// Lines of a tool result preview to show before folding the rest.
    pub tool_lines: usize,
    /// Senders of inputs seen so far, by queue ID, for turn headers.
    pub senders: &'a HashMap<String, String>,
}

impl Context<'_> {
    fn span(&self, style: Style, text: &str) -> Span {
        Span::new(style, text)
    }

    fn head(&self, ts: u64) -> Span {
        self.span(palette::TIME, &format!("{} ", clock(ts)))
    }

    fn pad(&self) -> Span {
        self.span(Style::PLAIN, &" ".repeat(HEAD_COLUMNS))
    }

    /// One line: the time, then `spans`, wrapped with continuation lines under the text.
    fn line(&self, ts: u64, lead: &[Span], spans: &[Span]) -> Vec<Line> {
        let mut first = vec![self.head(ts)];
        first.extend_from_slice(lead);
        let hang = HEAD_COLUMNS + render::line_width(lead);
        let rest = vec![self.span(Style::PLAIN, &" ".repeat(hang))];
        render::wrap(spans, self.width, &first, &rest)
    }

    /// Markdown `text` under a header line, behind a colored bar.
    fn block(&self, text: &str, bar: Style) -> Vec<Line> {
        let prefix = vec![self.pad(), self.span(bar, "┃ ")];
        let frame = Frame {
            width: self.width,
            first: &prefix,
            rest: &prefix,
        };
        render::markdown(text, &frame, &Theme::default(), self.pretty)
    }

    /// A rule across the width: `── label ─────`.
    fn rule(&self, ts: u64, glyph: &str, label: Vec<Span>, style: Style) -> Line {
        let mut line = vec![self.head(ts), self.span(style, &format!("{glyph}{glyph} "))];
        // The label is cut, never wrapped, so that at least three rule glyphs follow it.
        let label = render::fit(&label, self.width.saturating_sub(HEAD_COLUMNS + 3 + 4));
        let used = HEAD_COLUMNS + 3 + render::line_width(&label) + 1;
        line.extend(label);
        let fill = self.width.saturating_sub(used).max(3);
        line.push(self.span(style, &format!(" {}", glyph.repeat(fill))));
        line
    }
}

/// Columns taken by the `HH:MM:SS ` timestamp.
const HEAD_COLUMNS: usize = 9;

/// The human rendering of one event as plain text lines, safe to print to a terminal.
#[must_use]
pub fn render(event: &Event, width: usize) -> Vec<String> {
    let senders = HashMap::new();
    let context = Context {
        width,
        pretty: false,
        tool_lines: DEFAULT_TOOL_LINES,
        senders: &senders,
    };
    render_lines(event, &context)
        .iter()
        .map(|line| render::emit(line, false))
        .collect()
}

/// Lines of a tool result shown before the rest is folded, by default.
pub const DEFAULT_TOOL_LINES: usize = 4;

/// The rendering of one event as styled lines.
#[must_use]
pub fn render_lines(event: &Event, cx: &Context<'_>) -> Vec<Line> {
    let data = &event.data;
    let ts = event.ts;
    let s = |style: Style, text: &str| Span::new(style, text);
    match event.kind.as_str() {
        "lifecycle.started" => {
            let harness = [text_of(data, "harness"), text_of(data, "harness_version")]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(" ");
            cx.line(
                ts,
                &[],
                &[s(
                    palette::LIFECYCLE.bold(),
                    &format!(
                        "══ wrapper started · generation {} · {harness} · session {} ══",
                        event.generation,
                        event.session.as_deref().unwrap_or("?"),
                    ),
                )],
            )
        }
        "lifecycle.harness_spawned" => cx.line(
            ts,
            &[],
            &[s(
                palette::LIFECYCLE,
                &format!(
                    "   harness started (pid {}){}",
                    data["pid"],
                    if data["resume"] == true {
                        ", resuming the session"
                    } else {
                        ", new session"
                    }
                ),
            )],
        ),
        "lifecycle.harness" => cx.line(
            ts,
            &[],
            &[s(
                palette::LIFECYCLE,
                &format!(
                    "   harness ready: {} {}, model {}, permission mode {}",
                    text_of(data, "harness"),
                    text_of(data, "version"),
                    text_of(data, "model"),
                    text_of(data, "permission_mode"),
                ),
            )],
        ),
        "lifecycle.harness_exited" => cx.line(
            ts,
            &[],
            &[s(
                palette::ERROR,
                &format!(
                    "!! harness exited ({}); {}",
                    data.get("code")
                        .map_or_else(|| "signal".to_owned(), Value::to_string),
                    text_of(data, "next"),
                ),
            )],
        ),
        "lifecycle.stopped" => cx.line(
            ts,
            &[],
            &[s(
                palette::LIFECYCLE.bold(),
                &format!("══ wrapper stopped: {} ══", text_of(data, "cause")),
            )],
        ),
        "lifecycle.takeover" => cx.line(
            ts,
            &[],
            &[s(
                palette::LIFECYCLE.bold(),
                &format!(
                    "══ taken over by an interactive terminal{}; this log has no events until it is handed back ══",
                    match text_of(data, "tab") {
                        "" => String::new(),
                        tab => format!(" (tab {tab})"),
                    }
                ),
            )],
        ),
        "lifecycle.released" => cx.line(
            ts,
            &[],
            &[s(
                palette::LIFECYCLE.bold(),
                &format!(
                    "══ interactive session ended{}; agentctl handback resumes it headless ══",
                    data["code"]
                        .as_i64()
                        .map_or_else(String::new, |code| format!(" (exit {code})"))
                ),
            )],
        ),
        "interrupt.requested" => cx.line(ts, &[], &[s(palette::INTERRUPT, "!! interrupt requested")]),
        "status" => cx.line(
            ts,
            &[],
            &[s(palette::STATUS, &format!("   · {}", text_of(data, "status")))],
        ),
        "input.queued" => {
            let mut header = vec![
                s(palette::INPUT, "← "),
                s(palette::INPUT, text_of(data, "from")),
                s(
                    palette::DETAIL,
                    &format!(
                        "  {} · {}  [{}]",
                        text_of(data, "deliver"),
                        text_of(data, "priority"),
                        text_of(data, "queue_id")
                    ),
                ),
            ];
            if text_of(data, "from").is_empty() {
                header[1] = s(palette::INPUT, "?");
            }
            let mut lines = cx.line(ts, &[], &header);
            let excerpt = text_of(data, "excerpt");
            if !excerpt.trim().is_empty() {
                lines.extend(cx.block(excerpt, palette::INPUT_BAR));
            }
            lines
        }
        "input.started" => cx.line(
            ts,
            &[],
            &[s(
                palette::STARTED,
                &format!(
                    "  ↳ started [{}]{}",
                    text_of(data, "queue_id"),
                    if data["mid_turn"] == true {
                        " mid-turn"
                    } else {
                        ""
                    }
                ),
            )],
        ),
        "input.completed" => Vec::new(),
        "input.cancelled" => cx.line(
            ts,
            &[],
            &[s(
                palette::INTERRUPT,
                &format!(
                    "  ↳ cancelled [{}] {}",
                    text_of(data, "queue_id"),
                    if data["before_start"] == true {
                        "before the model saw it; returned to the queue"
                    } else {
                        "after the model saw it"
                    }
                ),
            )],
        ),
        "turn.started" => {
            let cause = text_of(data, "cause");
            let from = event
                .causation
                .as_deref()
                .and_then(|queue_id| cx.senders.get(queue_id));
            let mut label = vec![s(palette::TURN, &format!("turn {}", event.turn))];
            let detail = match (cause, from) {
                ("input", Some(from)) => format!(" · from {from}"),
                ("self", _) => " · started by the agent".to_owned(),
                ("", _) => String::new(),
                (cause, _) => format!(" · {cause}"),
            };
            label.push(s(palette::DETAIL, &detail));
            vec![
                Vec::new(),
                cx.rule(ts, "━", label, palette::TURN_RULE),
            ]
        }
        "turn.completed" => {
            let usage = &data["usage"];
            let count = |key: &str| tokens(usage[key].as_u64().unwrap_or(0));
            let tokens = format!(
                "in {} out {} cache r {} w {}",
                count("input_tokens"),
                count("output_tokens"),
                count("cache_read_input_tokens"),
                count("cache_creation_input_tokens"),
            );
            let cost = data["cost_usd"]
                .as_f64()
                .filter(|cost| *cost >= 0.00005)
                .map_or_else(String::new, |cost| format!(" · ${cost:.4}"));
            let outcome = text_of(data, "outcome");
            let style = match outcome {
                "completed" => palette::DETAIL,
                "interrupted" => palette::INTERRUPT,
                _ => palette::ERROR,
            };
            let label = vec![s(
                style,
                &format!(
                    "turn {} {outcome} · {} · {tokens}{cost}",
                    event.turn,
                    seconds(data["duration_ms"].as_u64()),
                ),
            )];
            vec![cx.rule(ts, "─", label, style)]
        }
        "assistant.text" => {
            let first = vec![cx.head(ts), s(palette::ASSISTANT, "● ")];
            let rest = vec![cx.pad(), s(Style::PLAIN, "  ")];
            let frame = Frame {
                width: cx.width,
                first: &first,
                rest: &rest,
            };
            render::markdown(text_of(data, "text"), &frame, &Theme::default(), cx.pretty)
        }
        "reply.sent" => {
            let channel = match text_of(data, "channel") {
                "chat" => "chat reply",
                "team" => "team message",
                "agent" => "message to agent",
                other => other,
            };
            let mut lines = cx.line(
                ts,
                &[],
                &[
                    s(palette::REPLY, &format!("⇢ {channel} ")),
                    s(palette::REPLY_BAR, text_of(data, "to")),
                ],
            );
            match data.get("text").and_then(Value::as_str) {
                Some(text) if !text.trim().is_empty() => {
                    lines.extend(cx.block(text, palette::REPLY_BAR));
                }
                _ => lines.extend(render::wrap(
                    &[s(
                        palette::DETAIL,
                        &format!("(text in {}, not readable)", text_of(data, "file")),
                    )],
                    cx.width,
                    &[cx.pad(), s(palette::REPLY_BAR, "┃ ")],
                    &[cx.pad(), s(palette::REPLY_BAR, "┃ ")],
                )),
            }
            lines
        }
        "tool.started" => cx.line(
            ts,
            &[s(palette::TOOL, &format!("$ {}  ", text_of(data, "tool")))],
            &[s(palette::TOOL_TEXT, text_of(data, "summary"))],
        ),
        "tool.completed" => {
            let ok = data["ok"] != false;
            let mut detail = format!(" {}", seconds(data["duration_ms"].as_u64()));
            if let Some(bytes) = data["output_bytes"].as_u64() {
                detail.push_str(&format!(" · {bytes} B"));
            }
            let total = data["output_lines"].as_u64().unwrap_or(0) as usize;
            if total > 1 {
                detail.push_str(&format!(" · {total} lines"));
            }
            let mut lines = cx.line(
                ts,
                &[s(Style::PLAIN, "  ")],
                &[
                    s(
                        if ok { palette::OK } else { palette::FAIL },
                        if ok { "✓" } else { "✗" },
                    ),
                    s(palette::DETAIL, &detail),
                ],
            );
            let preview: Vec<&str> = data["preview"]
                .as_array()
                .map(|lines| lines.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let shown = preview.len().min(cx.tool_lines);
            let gutter = format!("{}  │ ", " ".repeat(HEAD_COLUMNS));
            let room = cx.width.saturating_sub(render::width_of(&gutter)).max(8);
            let output_style = if ok { palette::OUTPUT } else { palette::FAIL.dimmed() };
            for text in &preview[..shown] {
                lines.push(vec![
                    s(palette::DETAIL, &gutter),
                    s(output_style, &render::truncate(&render::sanitize(text), room)),
                ]);
            }
            let folded = total.max(preview.len()).saturating_sub(shown);
            if shown > 0 && folded > 0 {
                lines.push(vec![
                    s(Style::PLAIN, &" ".repeat(HEAD_COLUMNS + 2)),
                    s(
                        palette::DETAIL,
                        &format!("… {folded} more line{}", if folded == 1 { "" } else { "s" }),
                    ),
                ]);
            }
            lines
        }
        "permission.denied" => cx.line(
            ts,
            &[s(palette::ERROR, &format!("⛔ denied {}  ", text_of(data, "tool")))],
            &[s(palette::TOOL_TEXT, text_of(data, "summary"))],
        ),
        "compact.started" => cx.line(ts, &[], &[s(palette::STATUS, "⋯ compacting")]),
        "compact.completed" => cx.line(
            ts,
            &[],
            &[s(
                palette::STATUS,
                &format!(
                    "⋯ compacted ({}): {} → {} tokens",
                    text_of(data, "trigger"),
                    data["pre_tokens"].as_u64().unwrap_or(0),
                    data["post_tokens"].as_u64().unwrap_or(0),
                ),
            )],
        ),
        "error" => cx.line(
            ts,
            &[],
            &[s(
                palette::ERROR,
                &format!("!! {}: {}", text_of(data, "class"), text_of(data, "message")),
            )],
        ),
        other => cx.line(
            ts,
            &[],
            &[s(
                palette::STATUS,
                &format!("· {other} {}", events::excerpt(&data.to_string(), 160)),
            )],
        ),
    }
}

/// The terminal's width in columns, when standard output is a terminal.
#[must_use]
pub fn terminal_width() -> Option<usize> {
    // SAFETY: TIOCGWINSZ writes only into the zeroed winsize it is given.
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut size) };
    (result == 0 && size.ws_col > 0).then_some(usize::from(size.ws_col))
}

/// Prints events with the gap lines between them.
struct Printer<'a> {
    options: &'a TailOptions,
    out: &'a mut dyn Write,
    last_seq: Option<u64>,
    senders: HashMap<String, String>,
}

impl Printer<'_> {
    fn width(&self) -> usize {
        if self.options.auto_width {
            if let Some(columns) = terminal_width() {
                return columns.max(20);
            }
        }
        self.options.width
    }

    fn remember(&mut self, event: &Event) {
        if event.kind == "input.queued" {
            if self.senders.len() > 1024 {
                self.senders.clear();
            }
            let queue_id = text_of(&event.data, "queue_id");
            if !queue_id.is_empty() {
                self.senders
                    .insert(queue_id.to_owned(), text_of(&event.data, "from").to_owned());
            }
        }
    }

    fn lines(&self, event: &Event, marker: &str) -> Vec<String> {
        let context = Context {
            width: self.width().saturating_sub(render::width_of(marker)),
            pretty: self.options.pretty,
            tool_lines: self.options.tool_lines,
            senders: &self.senders,
        };
        render_lines(event, &context)
            .iter()
            .map(|line| format!("{marker}{}", render::emit(line, self.options.pretty)))
            .collect()
    }

    /// Print `event` (if its type is selected), first noting any hole before it.
    fn event(&mut self, event: &Event) -> io::Result<()> {
        if let Some(previous) = self.last_seq {
            if event.seq > previous + 1 {
                self.gap(previous + 1, event.seq - 1)?;
            }
        }
        self.last_seq = Some(self.last_seq.map_or(event.seq, |seq| seq.max(event.seq)));
        self.remember(event);
        if !type_matches(&self.options.types, &event.kind) {
            return Ok(());
        }
        if self.options.json {
            serde_json::to_writer(&mut *self.out, event)?;
            writeln!(self.out)?;
        } else {
            for line in self.lines(event, "") {
                writeln!(self.out, "{line}")?;
            }
        }
        self.out.flush()
    }

    fn gap(&mut self, first: u64, last: u64) -> io::Result<()> {
        if self.options.json {
            return writeln!(
                self.out,
                "{}",
                serde_json::json!({"gap": {"first_seq": first, "last_seq": last}})
            );
        }
        let text = if first == last {
            format!("… event {first} is missing (pruned or unreadable)")
        } else {
            format!("… events {first}-{last} are missing (pruned or unreadable)")
        };
        let line = vec![Span::new(palette::INTERRUPT, &text)];
        writeln!(self.out, "{}", render::emit(&line, self.options.pretty))
    }
}

/// Events `--follow` prints before following when neither `--last` nor `--since` is given.
pub const FOLLOW_DEFAULT_LAST: usize = 200;

/// The index of the first segment to read so that at least `last` events matching `types` are
/// read, counting back from the newest segment.
fn start_for_last(
    segments: &[events::Segment],
    last: usize,
    types: &[String],
) -> io::Result<usize> {
    let mut count = 0;
    for index in (0..segments.len()).rev() {
        count += events::read_segments(&segments[index..=index])?
            .iter()
            .filter(|event| type_matches(types, &event.kind))
            .count();
        if count >= last {
            return Ok(index);
        }
    }
    Ok(0)
}

/// The events of the turn holding `target`: the segment holding it, widened to neighbouring
/// segments while the turn continues across their boundary.
fn turn_of(segments: &[events::Segment], target: u64) -> io::Result<Option<Vec<Event>>> {
    if segments.is_empty() {
        return Ok(None);
    }
    let index = events::segment_holding(segments, target)?;
    let (mut low, mut high) = (index, index);
    let mut window = events::read_segments(&segments[low..=high])?;
    let Some(turn) = window
        .iter()
        .find(|event| event.seq == target)
        .map(|event| event.turn)
    else {
        return Ok(None);
    };
    while low > 0 && window.first().is_none_or(|event| event.turn == turn) {
        low -= 1;
        let mut earlier = events::read_segments(&segments[low..=low])?;
        earlier.append(&mut window);
        window = earlier;
    }
    while high + 1 < segments.len() && window.last().is_none_or(|event| event.turn == turn) {
        high += 1;
        window.extend(events::read_segments(&segments[high..=high])?);
    }
    Ok(Some(window))
}

/// Run `tail` over the log in `dir`, writing to `out`. With `follow`, poll until `stop` returns
/// true. Returns `false` when `--at` named an event that does not exist. Only the segments the
/// selection needs are read: `--since` starts at the segment holding it, `--last` at the newest
/// segments that hold enough events, and `--at` at the turn's own segments.
pub fn run(
    dir: &Path,
    options: &TailOptions,
    out: &mut dyn Write,
    stop: &dyn Fn() -> bool,
) -> io::Result<bool> {
    let all = events::segments(dir)?;
    let mut printer = Printer {
        options,
        out,
        last_seq: None,
        senders: HashMap::new(),
    };
    if let Some(target) = options.at {
        let Some(window) = turn_of(&all, target)? else {
            return Ok(false);
        };
        let turn = window
            .iter()
            .find(|event| event.seq == target)
            .map_or(0, |event| event.turn);
        for event in &window {
            printer.remember(event);
        }
        for event in window.iter().filter(|event| event.turn == turn) {
            let marker = if event.seq == target { "▶ " } else { "  " };
            if options.json {
                serde_json::to_writer(&mut *printer.out, event)?;
                writeln!(printer.out)?;
            } else {
                for line in printer.lines(event, marker) {
                    writeln!(printer.out, "{line}")?;
                }
            }
        }
        printer.out.flush()?;
        return Ok(true);
    }
    let last = options
        .last
        .or_else(|| (options.follow && options.since.is_none()).then_some(FOLLOW_DEFAULT_LAST));
    let start = match (options.since, last) {
        (Some(since), _) => events::segment_holding(&all, since.saturating_add(1))?,
        (None, Some(last)) => start_for_last(&all, last, &options.types)?,
        (None, None) => 0,
    };
    let mut follower = all
        .get(start)
        .map_or_else(|| Follower::new(dir), |segment| Follower::at(dir, segment));
    let initial = follower.poll()?;
    // Senders of earlier inputs label the turn headers of what is printed.
    for event in &initial {
        printer.remember(event);
    }
    let mut selected: Vec<&Event> = initial
        .iter()
        .filter(|event| options.since.is_none_or(|since| event.seq > since))
        .collect();
    if let Some(since) = options.since {
        printer.last_seq = Some(since);
    }
    if let Some(last) = last {
        let matching: Vec<usize> = selected
            .iter()
            .enumerate()
            .filter(|(_, event)| type_matches(&options.types, &event.kind))
            .map(|(index, _)| index)
            .collect();
        if matching.len() > last {
            let start = matching[matching.len() - last];
            selected.drain(..start);
        }
        printer.last_seq = selected.first().map(|event| event.seq.saturating_sub(1));
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
    fn renders_headers_blocks_rules_and_tools() {
        let dir = scratch("render");
        sample(&dir);
        let (_, text) = output(&dir, &TailOptions::default());
        assert!(
            text.contains("wrapper started · generation 1 · claude 2.1.310 · session s-1"),
            "{text}"
        );
        assert!(text.contains("← owner  now · urgent  [q1]"), "{text}");
        assert!(text.contains("┃ status please"), "{text}");
        assert!(text.contains("━━ turn 1 · from owner ━"), "{text}");
        assert!(text.contains("$ Bash  git fetch origin"), "{text}");
        assert!(text.contains("✓ 1.2 s · 34 B"), "{text}");
        assert!(text.contains("● All green."), "{text}");
        assert!(
            text.contains("── turn 1 completed · 3.4 s · in 2 out 50 cache r 0 w 0 · $0.0123 ─"),
            "{text}"
        );
        assert!(!text.contains('\u{1b}'), "{text}");
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
    fn escape_sequences_in_text_are_neutralised() {
        let event = Event {
            v: 1,
            seq: 1,
            id: "evt-1-1".to_owned(),
            ts: 0,
            agent: "a".to_owned(),
            generation: 1,
            session: None,
            turn: 1,
            causation: None,
            kind: "assistant.text".to_owned(),
            data: json!({"text": "hi \u{1b}]52;c;cGF5bG9hZA==\u{7}there\u{202e}x\tend"}),
        };
        let lines = render(&event, 100);
        assert!(
            lines.iter().all(|line| !line.chars().any(char::is_control)),
            "{lines:?}"
        );
        assert!(
            lines[0].contains("hi \u{fffd}]52;c;cGF5bG9hZA==\u{fffd}there\u{fffd}x end"),
            "{lines:?}"
        );
    }

    #[test]
    fn prose_wraps_to_the_width_and_keeps_line_breaks() {
        let mut event =
            assistant("first paragraph is long enough to wrap twice at forty\nsecond line");
        event.ts = 0;
        let lines = render(&event, 40);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines.iter().all(|line| render::width_of(line) <= 40),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.trim() == "second line"),
            "{lines:?}"
        );
    }

    fn assistant(text: &str) -> Event {
        event(1, "assistant.text", json!({ "text": text }))
    }

    fn event(seq: u64, kind: &str, data: Value) -> Event {
        Event {
            v: 1,
            seq,
            id: format!("evt-1-{seq}"),
            ts: 1_793_059_038_000 + seq * 1000,
            agent: "coordinator".to_owned(),
            generation: 1,
            session: Some("s-1".to_owned()),
            turn: u64::from(seq >= 4),
            causation: (seq >= 4).then(|| "q-7".to_owned()),
            kind: kind.to_owned(),
            data,
        }
    }

    /// A turn exercising every kind of event and every Markdown construct the renderer knows.
    fn snapshot_events() -> Vec<Event> {
        let reply = "## Status\n\nThe gate is **green** and `main` moved to 59543339.\n\n\
                     - landed #588 headless-tail-pretty\n- next: rotation\n  - then watch\n\n\
                     ```sh\nagentctl tail coordinator --follow\n```";
        let prose = "I checked the queue and the gate. This sentence is long enough that it must wrap \
                     at the terminal width instead of running off as one line.\n\n\
                     ### Results\n\n1. first item with *emphasis*\n2. second item with a [link](https://example.com/x)\n\n\
                     | step | result |\n|---|---|\n| build | ok |\n| tests | 2 failed |\n\n\
                     > quoted owner text\n\n---\n\n```rust\nfn main() {\n\tprintln!(\"hi\");\n}\n```";
        vec![
            event(
                1,
                "lifecycle.started",
                json!({"harness": "claude", "harness_version": "2.1.310"}),
            ),
            event(
                2,
                "lifecycle.harness_spawned",
                json!({"pid": 4242, "resume": true}),
            ),
            event(
                3,
                "input.queued",
                json!({"queue_id": "q-7", "from": "owner", "deliver": "turn-boundary",
                "priority": "normal", "excerpt": "Please check the gate.\nThen **report** back."}),
            ),
            event(4, "turn.started", json!({"cause": "input"})),
            event(
                5,
                "input.started",
                json!({"queue_id": "q-7", "mid_turn": false}),
            ),
            event(
                6,
                "tool.started",
                json!({"tool": "Bash", "summary": "git -C agent-utils log --oneline -5 && python3 scripts/validate.py --base origin/main > /tmp/gate.log 2>&1; echo rc=$?"}),
            ),
            event(
                7,
                "tool.completed",
                json!({"tool": "Bash", "ok": true, "duration_ms": 1234, "output_bytes": 2048,
                "output_lines": 40, "preview": ["59543339 agentctl: watch and recover", "435de837 minibeads: reserve", "433116ec minibeads: close", "7f29d9c4 minibeads: note", "ed6e32d3 wrkslotsd", "x", "y", "z"]}),
            ),
            event(
                8,
                "tool.started",
                json!({"tool": "Read", "summary": "/w/README.md"}),
            ),
            event(
                9,
                "tool.completed",
                json!({"tool": "Bash", "ok": false, "duration_ms": 15000, "output_bytes": 31,
                "output_lines": 1, "preview": ["error: \u{1b}[31mno such file"]}),
            ),
            event(10, "assistant.text", json!({ "text": prose })),
            event(
                11,
                "reply.sent",
                json!({"channel": "chat", "to": "r-12 #001", "text": reply}),
            ),
            event(
                12,
                "reply.sent",
                json!({"channel": "team", "to": "tm-1", "text": null, "file": "/tmp/gone.md"}),
            ),
            event(
                13,
                "permission.denied",
                json!({"tool": "Bash", "summary": "git push origin main"}),
            ),
            event(14, "interrupt.requested", json!({})),
            event(
                15,
                "input.cancelled",
                json!({"queue_id": "q-8", "before_start": false}),
            ),
            event(
                16,
                "turn.completed",
                json!({"outcome": "interrupted", "duration_ms": 61000, "cost_usd": 0.5,
                "usage": {"input_tokens": 10, "output_tokens": 900, "cache_read_input_tokens": 50000}}),
            ),
            event(17, "status", json!({"status": "idle"})),
            event(
                19,
                "error",
                json!({"class": "protocol", "message": "unexpected line"}),
            ),
        ]
    }

    /// The snapshot text: clock times replaced (they depend on the local time zone) and the
    /// escape character shown as `␛`.
    fn snapshot_text(pretty: bool) -> String {
        let options = TailOptions {
            pretty,
            width: 72,
            ..TailOptions::default()
        };
        let mut buffer = Vec::new();
        let mut printer = Printer {
            options: &options,
            out: &mut buffer,
            last_seq: None,
            senders: HashMap::new(),
        };
        for event in snapshot_events() {
            printer.event(&event).unwrap();
        }
        let text = String::from_utf8(buffer).unwrap();
        let clock = regex::Regex::new(r"\d\d:\d\d:\d\d ").unwrap();
        clock.replace_all(&text, "HH:MM:SS ").replace('\u{1b}', "␛")
    }

    fn check_snapshot(name: &str, actual: &str) {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/headless/snapshots")
            .join(name);
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            fs::write(&path, actual).unwrap();
            return;
        }
        let expected = fs::read_to_string(&path).unwrap_or_default();
        assert!(
            expected == actual,
            "snapshot {} differs (rerun with UPDATE_SNAPSHOTS=1 to accept):\n{actual}",
            path.display()
        );
    }

    /// A log of `turns` turns of five events each, in segments of about two turns.
    fn segmented(dir: &Path, turns: u64) {
        let mut writer = EventWriter::open(dir, "a").unwrap();
        writer.set_segment_limit(1400);
        for _ in 0..turns {
            writer.begin_turn();
            writer
                .append("turn.started", None, json!({"cause": "self"}))
                .unwrap();
            for n in 0..3 {
                writer
                    .append(
                        "assistant.text",
                        None,
                        json!({"text": format!("line {n} {}", "w".repeat(60))}),
                    )
                    .unwrap();
            }
            writer
                .append("turn.completed", None, json!({"outcome": "completed"}))
                .unwrap();
        }
    }

    fn seqs(text: &str) -> Vec<u64> {
        text.lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| value["seq"].as_u64())
            .collect()
    }

    #[test]
    fn selections_read_across_segments_and_skip_the_ones_they_do_not_need() {
        let dir = scratch("segments");
        segmented(&dir, 12);
        let all = events::segments(&dir).unwrap();
        assert!(all.len() >= 5, "{all:?}");
        // A turn that straddles a segment boundary prints whole with --at.
        let boundary = events::first_seq(&all[2].path).unwrap().unwrap();
        let json = |options: TailOptions| {
            output(
                &dir,
                &TailOptions {
                    json: true,
                    ..options
                },
            )
        };
        let (found, text) = json(TailOptions {
            at: Some(boundary),
            ..TailOptions::default()
        });
        assert!(found);
        let turn: Vec<u64> = seqs(&text);
        assert_eq!(turn.len(), 5, "{text}");
        assert!(turn.contains(&boundary));
        // --last and --since never open the oldest segments: make them unreadable.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&all[0].path, fs::Permissions::from_mode(0o000)).unwrap();
        let (_, text) = json(TailOptions {
            last: Some(3),
            ..TailOptions::default()
        });
        assert_eq!(seqs(&text), vec![58, 59, 60]);
        let (_, text) = json(TailOptions {
            since: Some(55),
            ..TailOptions::default()
        });
        assert_eq!(seqs(&text), vec![56, 57, 58, 59, 60]);
        // --follow alone starts with the last FOLLOW_DEFAULT_LAST events: here, all of them.
        fs::set_permissions(&all[0].path, fs::Permissions::from_mode(0o600)).unwrap();
        let (_, text) = json(TailOptions {
            follow: true,
            ..TailOptions::default()
        });
        assert_eq!(seqs(&text), (1..=60).collect::<Vec<u64>>());
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn follow_starts_with_the_last_events_of_a_long_log() {
        let dir = scratch("followlast");
        segmented(&dir, 50);
        let (_, text) = output(
            &dir,
            &TailOptions {
                follow: true,
                json: true,
                ..TailOptions::default()
            },
        );
        let printed = seqs(&text);
        assert_eq!(printed.len(), FOLLOW_DEFAULT_LAST);
        assert_eq!(*printed.last().unwrap(), 250);
        assert!(!text.contains("gap"), "{text}");
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn snapshot_plain() {
        let text = snapshot_text(false);
        assert!(!text.contains('␛'), "{text}");
        check_snapshot("tail-plain.txt", &text);
    }

    #[test]
    fn snapshot_pretty() {
        let text = snapshot_text(true);
        assert!(text.contains("␛[1;32m● ␛[0m"), "{text}");
        check_snapshot("tail-pretty.txt", &text);
    }

    #[test]
    fn every_line_fits_the_width_in_both_modes() {
        for pretty in [false, true] {
            for width in [40, 72, 120] {
                let senders = HashMap::new();
                let context = Context {
                    width,
                    pretty,
                    tool_lines: 4,
                    senders: &senders,
                };
                for event in snapshot_events() {
                    for line in render_lines(&event, &context) {
                        let shown = render::emit(&line, false);
                        assert!(
                            render::width_of(&shown) <= width,
                            "{width} {pretty}: {shown:?} ({})",
                            render::width_of(&shown)
                        );
                    }
                }
            }
        }
    }
}
