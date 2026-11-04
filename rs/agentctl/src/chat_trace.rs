//! The main-session trace (#714 chat-transcript-primary, revised).
//!
//! Fenced `CHAT_REPLY` blocks remain the only way replies reach owner
//! threads. Everything else the agent writes — assistant text outside a
//! fenced block — would otherwise live only in its TUI. With
//! `--trace-thread`, this collector reads that text from the agent's
//! transcript (the #710 reader), removes every complete fenced block
//! (already posted, so never double-posted), and emits the remainder for
//! posting to one trace thread. Each post names the prompt(s) that started
//! the turn as its trigger; the trigger is provenance, not routing, and
//! travels only in the post's metadata (#832 chat-trace-format-tagging,
//! owner 2026-11-03): the text itself carries no label, so it reads (and
//! is read aloud) as the agent wrote it. Every post is also marked for
//! machines with [`trace_metadata`], and its text is escaped by
//! [`escape_for_chat`] so file names and relative links survive the chat.
//!
//! Filtering is deliberately minimal: drop fenced blocks and empty
//! remainders, coalesce within a window or to the turn's end, cap the
//! rate, and split over-long posts outside code fences. Thinking and
//! tool calls never decode to [`Event::Text`], so they are never emitted.

use std::collections::BTreeMap;

use agent_harness::{replies::OutsideBlocks, Event};

/// One trace post to make.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TracePost {
    /// The post's text, escaped for chat, with no label.
    pub text: String,
    /// The prompt(s) that started the turn (`owner message`, `reply CHAT_REPLY_007`,
    /// `agent lane-a`, ...), for the post's metadata.
    pub trigger: String,
}

/// The Slack metadata `event_type` of every trace post (#832).
pub const TRACE_EVENT_TYPE: &str = "agentctl_trace";

/// One trace post's metadata (#832 chat-trace-format-tagging): event type
/// [`TRACE_EVENT_TYPE`] and payload `{v: 1, kind, agent, team, session, day, seq, trigger}`.
/// `kind` is `root` for the day's thread root and `entry` for a post in it; `day` is the ET
/// date; `seq` counts the day's posts from 0 at the root; `team` and `session` are null when
/// unknown, and the root's `trigger` is null.
#[must_use]
pub fn trace_metadata(
    kind: &str,
    agent: &str,
    team: Option<&str>,
    session: Option<&str>,
    day: &str,
    seq: u64,
    trigger: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "event_type": TRACE_EVENT_TYPE,
        "event_payload": {
            "v": 1, "kind": kind, "agent": agent, "team": team, "session": session,
            "day": day, "seq": seq, "trigger": trigger,
        },
    })
}

/// File extensions after which Slack's auto-linker takes a bare name for a web address (many
/// are also country or generic top-level domains, such as `.rs`, `.py`, `.md`, `.sh`, `.io`).
const LINKED_EXTENSIONS: &[&str] = &[
    "ai", "app", "c", "cc", "cfg", "cpp", "css", "csv", "dev", "go", "h", "html", "ini", "io",
    "java", "js", "json", "jsonl", "kt", "lock", "log", "md", "me", "mjs", "pl", "py", "rb", "rs",
    "sh", "so", "sql", "to", "toml", "ts", "txt", "xml", "yaml", "yml",
];

/// Escape trace text for chat, the way the reply path's #794 work protects names
/// (#832 chat-trace-format-tagging). Outside code fences and inline code:
///
/// * a bare file name such as `preemptions.rs` or `src/cli.rs` is put in inline
///   code, so Slack does not turn it into a link to a web site of that name;
/// * a Markdown link to a relative path, `[the note](ai_docs/x.md)`, becomes
///   ``the note (`ai_docs/x.md`)``, since a relative target can only show as raw link markup.
///
/// Links with a scheme (`https:`, `mailto:`) and anchors (`#…`) are left as they are, and so is
/// text inside code.
#[must_use]
pub fn escape_for_chat(text: &str) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push(line.to_owned());
            continue;
        }
        if in_fence {
            out.push(line.to_owned());
            continue;
        }
        // Even pieces lie outside inline code; odd ones inside. An unmatched backtick leaves
        // the rest of the line as it is.
        let pieces: Vec<&str> = line.split('`').collect();
        let closed = pieces.len() % 2 == 1;
        let mut escaped = String::with_capacity(line.len());
        for (index, piece) in pieces.iter().enumerate() {
            if index > 0 {
                escaped.push('`');
            }
            if index % 2 == 0 && (closed || index + 1 < pieces.len()) {
                escaped.push_str(&escape_links(piece));
            } else {
                escaped.push_str(piece);
            }
        }
        out.push(escaped);
    }
    out.join("\n")
}

/// [`escape_for_chat`] for one piece of text outside code: relative Markdown links first, then
/// bare names in what is left.
fn escape_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let Some((label, target, consumed)) = markdown_link(&rest[open..]) else {
            out.push_str(&escape_names(&rest[..=open]));
            rest = &rest[open + 1..];
            continue;
        };
        out.push_str(&escape_names(&rest[..open]));
        let absolute = target.contains(':') || target.starts_with('#');
        if absolute {
            out.push('[');
            out.push_str(&escape_names(label));
            out.push_str("](");
            out.push_str(target);
            out.push(')');
        } else {
            out.push_str(&escape_names(label));
            out.push_str(" (`");
            out.push_str(target);
            out.push_str("`)");
        }
        rest = &rest[open + consumed..];
    }
    out.push_str(&escape_names(rest));
    out
}

/// A Markdown link `[label](target)` at the start of `text`, with no space in the target, as
/// (label, target, bytes consumed).
fn markdown_link(text: &str) -> Option<(&str, &str, usize)> {
    let close = text.find("](")?;
    let label = &text[1..close];
    if label.contains('[') || label.contains(']') {
        return None;
    }
    let after = &text[close + 2..];
    let end = after.find(')')?;
    let target = &after[..end];
    if target.is_empty() || target.contains(char::is_whitespace) {
        return None;
    }
    Some((label, target, close + 2 + end + 1))
}

/// Put each bare file name in `text` in inline code (see [`LINKED_EXTENSIONS`]).
fn escape_names(text: &str) -> String {
    let name_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();
    let mut previous: Option<char> = None;
    while let Some((start, c)) = chars.next() {
        if !name_char(c) {
            out.push(c);
            previous = Some(c);
            continue;
        }
        let mut end = start + c.len_utf8();
        while let Some(&(at, next)) = chars.peek() {
            if !name_char(next) {
                break;
            }
            end = at + next.len_utf8();
            chars.next();
        }
        let token = &text[start..end];
        // A sentence's closing dots stay outside the name.
        let name = token.trim_end_matches('.');
        let trailing = &token[name.len()..];
        // Part of an address (`https://…`, `a@b.io`) or a path that begins `//`: not a name.
        let in_address = matches!(previous, Some(':' | '@')) || name.starts_with("//");
        let linked = name.rsplit_once('.').is_some_and(|(stem, extension)| {
            stem.chars()
                .last()
                .is_some_and(|last| last.is_ascii_alphanumeric() || last == '_')
                && LINKED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        });
        if linked && !in_address {
            out.push('`');
            out.push_str(name);
            out.push('`');
            out.push_str(trailing);
        } else {
            out.push_str(token);
        }
        previous = token.chars().last();
    }
    out
}

/// Per-turn counters for the trial.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TurnMetrics {
    /// Posts emitted for the turn.
    pub posts: usize,
    /// Total posted bytes.
    pub bytes: usize,
    /// Blocks dropped, by reason (`fence`, `empty`, `overflow`).
    pub drops: BTreeMap<String, usize>,
    /// Seconds from the turn's first prompt to its first post.
    pub first_post_latency_secs: Option<u64>,
}

/// A short label for one prompt, from the real bridge header
/// (`[from X via <plugin> <channel> · sent: … · reply: <CHAT_REPLY_ID> …]`),
/// an agentctl sender header, or a watcher/timer notice.
fn describe_prompt(text: &str) -> String {
    let first = text.lines().next().unwrap_or("");
    if first.starts_with("[from ") && first.contains(" via ") {
        if first.contains(" via agentctl") {
            if let Some(name) = first
                .strip_prefix("[from ")
                .and_then(|rest| rest.split(" via ").next())
                .filter(|name| !name.is_empty())
            {
                return format!("agent {name}");
            }
        }
        if let Some(after) = first.split("chat reply ").nth(1) {
            if let Some(id) = after
                .split([' ', ']', '·'])
                .next()
                .filter(|t| !t.is_empty())
            {
                return format!("reply {}", id.trim_matches(|c| c == '<' || c == '>'));
            }
        }
        if let Some(reply) = first
            .split(" · reply: ")
            .nth(1)
            .and_then(|rest| rest.split([' ', ']', '·']).next())
            .filter(|t| !t.is_empty() && *t != "agentctl")
        {
            let reply = reply.trim_matches(|c| c == '<' || c == '>');
            return format!("reply {reply}");
        }
        return "owner message".to_owned();
    }
    let lower = text.to_lowercase();
    if lower.contains("watch-agents") || lower.starts_with("[watcher]") {
        "watcher".to_owned()
    } else if lower.contains("tick-hub") || lower.starts_with("[timer]") {
        "timer".to_owned()
    } else {
        "prompt".to_owned()
    }
}

/// Split over-long text at paragraph breaks outside code fences,
/// hard-splitting at char boundaries as a last resort. Fence state is
/// tracked per line, so a compact code block cannot stick it on.
fn split_long(text: &str, max: usize) -> Vec<String> {
    if text.len() <= max {
        return vec![text.to_owned()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    for line in text.split('\n') {
        let fence_line = line.trim_start().starts_with("```");
        if fence_line {
            in_fence = !in_fence;
        }
        let blank_break = line.trim().is_empty() && !in_fence;
        if blank_break && current.len() > max {
            parts.push(std::mem::take(&mut current).trim().to_owned());
            continue;
        }
        if !in_fence && current.len() + line.len() + 1 > max && !current.is_empty() {
            // Break at the last blank line already held, else hard-split.
            if let Some(at) = current.rfind("\n\n") {
                let rest = current.split_off(at + 2);
                parts.push(std::mem::take(&mut current).trim().to_owned());
                current = rest;
            } else {
                let mut cut = max.min(current.len());
                while cut > 0 && !current.is_char_boundary(cut) {
                    cut -= 1;
                }
                let rest = current.split_off(cut);
                parts.push(std::mem::take(&mut current).trim().to_owned());
                current = rest;
            }
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    // Hard-split any part still over the budget (one very long line).
    let mut out = Vec::new();
    for part in parts {
        let mut rest = part.as_str();
        while rest.len() > max {
            let mut cut = max;
            while cut > 0 && !rest.is_char_boundary(cut) {
                cut -= 1;
            }
            let at = rest[..cut].rfind(' ').unwrap_or(cut);
            out.push(rest[..at].trim().to_owned());
            rest = rest[at..].trim_start();
        }
        if !rest.is_empty() {
            out.push(rest.to_owned());
        }
    }
    out.into_iter().filter(|p| !p.is_empty()).collect()
}

/// The trace thread id for a root post: `<channel>/threads/<ts>` from the
/// root receipt's `<channel>/messages/<ts>` message id.
#[must_use]
pub fn trace_thread_id(channel: &str, message_id: &str) -> String {
    let ts = message_id.rsplit('/').next().unwrap_or(message_id);
    format!("{channel}/threads/{ts}")
}

/// Cap held trace posts at `max`, dropping the oldest; returns how many
/// were dropped so the caller can note the omission in the next post.
pub fn cap_held<T>(held: &mut Vec<T>, max: usize) -> usize {
    if held.len() > max {
        let dropped = held.len() - max;
        held.drain(..dropped);
        dropped
    } else {
        0
    }
}

/// What to do with a trace post whose publish failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PostFailure {
    /// Provably not applied and not retryable: drop the post.
    Drop,
    /// Keep it held and retry later with the same UUID.
    Retry,
    /// The trace thread is gone: clear it and make a fresh root.
    ThreadGone,
}

/// Classify a failed trace post from the outbound failure's fields.
#[must_use]
pub fn classify_failure(
    code: &str,
    detail: &str,
    not_applied: bool,
    retryable: bool,
) -> PostFailure {
    let _ = detail;
    let thread_gone = code.contains("thread") && code.contains("not_found");
    if thread_gone {
        PostFailure::ThreadGone
    } else if not_applied && !retryable {
        PostFailure::Drop
    } else {
        PostFailure::Retry
    }
}

/// Prepend an omission note, replacing (not stacking) an existing one:
/// the counts add, so the note always names every omitted post so far.
#[must_use]
pub fn note_omission(text: &str, dropped: usize) -> String {
    if let Some(rest) = text.strip_prefix('[') {
        if let Some((count, after)) = rest.split_once(" trace posts omitted]") {
            if let Ok(previous) = count.parse::<usize>() {
                let after = after.strip_prefix('\n').unwrap_or(after);
                return format!("[{} trace posts omitted]\n{after}", previous + dropped);
            }
        }
    }
    format!("[{dropped} trace posts omitted]\n{text}")
}

/// A posts-per-minute budget (the reply breaker's 8/min, shared shape).
pub struct RateBudget {
    limit: usize,
    window_start: f64,
    used: usize,
}

impl RateBudget {
    /// A budget of `limit` posts per 60 s.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            window_start: f64::NEG_INFINITY,
            used: 0,
        }
    }

    /// Whether one more post fits at `now`; consumes it when it does.
    pub fn take(&mut self, now: f64) -> bool {
        if now - self.window_start >= 60.0 {
            self.window_start = now;
            self.used = 0;
        }
        if self.used < self.limit {
            self.used += 1;
            true
        } else {
            false
        }
    }
}

/// Whether a transcript event is new trace material: strictly after both
/// the persisted high-water mark and the process start.
#[must_use]
pub fn is_new(at: Option<f64>, high_water: f64, started: f64) -> bool {
    at.is_some_and(|at| at > high_water && at >= started)
}

/// Collects transcript events into labelled trace posts. Feed events in
/// order; call [`Collector::tick`] as time passes so held text flushes
/// after the window or when the oldest held text ages past it.
pub struct Collector {
    window_secs: f64,
    max_bytes: usize,
    max_pending_bytes: usize,
    labels: Vec<String>,
    turn_started: Option<f64>,
    pending: Vec<(String, f64)>,
    pending_bytes: usize,
    stripper: OutsideBlocks,
    metrics: TurnMetrics,
    finished: Vec<TurnMetrics>,
}

impl Collector {
    /// A collector holding text for `window` before flushing it.
    #[must_use]
    pub fn new(window: std::time::Duration) -> Self {
        Self {
            window_secs: window.as_secs_f64(),
            max_bytes: 3_500,
            max_pending_bytes: 14_000,
            labels: Vec::new(),
            turn_started: None,
            pending: Vec::new(),
            pending_bytes: 0,
            stripper: OutsideBlocks::new(),
            metrics: TurnMetrics::default(),
            finished: Vec::new(),
        }
    }

    fn flush(&mut self, now: f64) -> Vec<TracePost> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let mut body = self
            .pending
            .drain(..)
            .map(|(text, _)| text)
            .collect::<Vec<_>>()
            .join("\n\n");
        self.pending_bytes = 0;
        if body.len() > self.max_pending_bytes {
            let omitted = body.len() - self.max_pending_bytes;
            let mut cut = self.max_pending_bytes;
            while !body.is_char_boundary(cut) {
                cut -= 1;
            }
            body.truncate(cut);
            body.push_str(&format!("\n\n[{omitted} bytes omitted]"));
            *self.metrics.drops.entry("overflow".to_owned()).or_default() += 1;
        }
        let trigger = if self.labels.is_empty() {
            "prompt".to_owned()
        } else {
            self.labels.join(", ")
        };
        let body = escape_for_chat(&body);
        let mut posts = Vec::new();
        for part in split_long(&body, self.max_bytes) {
            if self.metrics.posts == 0 {
                if let (Some(start), true) = (self.turn_started, now > 0.0) {
                    self.metrics.first_post_latency_secs = Some((now - start).max(0.0) as u64);
                }
            }
            self.metrics.posts += 1;
            self.metrics.bytes += part.len();
            posts.push(TracePost {
                text: part,
                trigger: trigger.clone(),
            });
        }
        posts
    }

    fn end_turn(&mut self, now: f64) -> Vec<TracePost> {
        // An unclosed block may still be completed and posted as a reply,
        // so its text is dropped here, never traced (no double post).
        let held = self.stripper.finish();
        if !held.trim().is_empty() {
            *self.metrics.drops.entry("unclosed".to_owned()).or_default() += 1;
        }
        self.stripper = OutsideBlocks::new();
        let posts = self.flush(now);
        if self.metrics.posts > 0 || !self.metrics.drops.is_empty() {
            self.finished.push(std::mem::take(&mut self.metrics));
        } else {
            self.metrics = TurnMetrics::default();
        }
        self.labels.clear();
        self.turn_started = None;
        posts
    }

    /// Flush held text when the oldest held block has aged past the window.
    pub fn tick(&mut self, now: f64) -> Vec<TracePost> {
        if self
            .pending
            .first()
            .is_some_and(|(_, at)| now - at >= self.window_secs)
        {
            self.flush(now)
        } else {
            Vec::new()
        }
    }

    /// Feed one event; returns any posts it completes.
    pub fn feed(&mut self, event: &Event) -> Vec<TracePost> {
        match event {
            Event::Prompt { at, text, .. } => {
                if self.turn_started.is_none() {
                    self.turn_started = *at;
                }
                let label = describe_prompt(text);
                if !self.labels.contains(&label) {
                    self.labels.push(label);
                }
                Vec::new()
            }
            Event::Text { at, text } => {
                let stripped = self.stripper.feed(text);
                if stripped.trim().is_empty() {
                    let reason = if text.trim().is_empty() {
                        "empty"
                    } else {
                        "fence"
                    };
                    *self.metrics.drops.entry(reason.to_owned()).or_default() += 1;
                    return Vec::new();
                }
                let stripped = stripped.trim().to_owned();
                self.pending_bytes += stripped.len();
                self.pending.push((stripped, at.unwrap_or(0.0)));
                Vec::new()
            }
            Event::TurnEnd { at } => self.end_turn(at.unwrap_or(0.0)),
            Event::ToolCall { .. } => Vec::new(),
        }
    }

    /// Take the metrics of turns finished since the last call.
    pub fn take_metrics(&mut self) -> Vec<TurnMetrics> {
        std::mem::take(&mut self.finished)
    }
}

/// The ET calendar date (`YYYY-MM-DD`) of a Unix time, with US DST rules
/// (DST runs 02:00 local on the second Sunday of March to 02:00 local on
/// the first Sunday of November).
#[must_use]
pub fn et_date(unix_secs: f64) -> String {
    // Decide DST from the ET-local date: try EST, and if that local date
    // is in the DST window the offset is EDT (the one-hour ambiguity on
    // change days resolves to the post-change offset within the day).
    let est_days = ((unix_secs - 5.0 * 3600.0).div_euclid(86_400.0)) as i64;
    let (y, _, _) = civil_from_days(est_days);
    let in_dst = (nth_sunday(y, 3, 2)..nth_sunday(y, 11, 1)).contains(&est_days);
    let offset = if in_dst { -4.0 * 3600.0 } else { -5.0 * 3600.0 };
    let et_days = ((unix_secs + offset).div_euclid(86_400.0)) as i64;
    let (yy, mm, dd) = civil_from_days(et_days);
    format!("{yy:04}-{mm:02}-{dd:02}")
}

fn nth_sunday(year: i64, month: i64, n: i64) -> i64 {
    let first = days_from_civil(year, month, 1);
    let dow = (first + 4).rem_euclid(7);
    first + (7 - dow) % 7 + (n - 1) * 7
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_harness::Via;
    use std::time::Duration;

    fn prompt(at: f64, text: &str) -> Event {
        Event::Prompt {
            at: Some(at),
            text: text.into(),
            via: Via::Typed,
        }
    }
    fn text(at: f64, text: &str) -> Event {
        Event::Text {
            at: Some(at),
            text: text.into(),
        }
    }
    const REAL_HEADER: &str = "[from Ryan via slack #agent-utils-030 · sent: 2026-11-02 09:00 ET · reply: <CHAT_REPLY_007>]";

    #[test]
    fn fenced_text_is_dropped_and_remainder_traced() {
        let mut c = Collector::new(Duration::from_secs(45));
        c.feed(&prompt(0.0, REAL_HEADER));
        c.feed(&text(
            1.0,
            "<CHAT_REPLY_007>\nposted answer\n</CHAT_REPLY_007>",
        ));
        c.feed(&text(2.0, "Side note the owner never saw in chat."));
        let posts = c.feed(&Event::TurnEnd { at: Some(3.0) });
        assert_eq!(posts.len(), 1);
        // #832: no label in the text; the trigger travels in the metadata only.
        assert_eq!(posts[0].text, "Side note the owner never saw in chat.");
        assert_eq!(posts[0].trigger, "reply CHAT_REPLY_007");
        assert!(!posts[0].text.contains("after:"));
        assert!(posts[0].text.contains("Side note"));
        assert!(!posts[0].text.contains("posted answer"));
    }

    #[test]
    fn split_and_unclosed_blocks_do_not_leak_or_lose_text() {
        let mut c = Collector::new(Duration::from_secs(45));
        c.feed(&prompt(0.0, REAL_HEADER));
        // Block split across two Text events.
        c.feed(&text(1.0, "<CHAT_REPLY_007>\nposted"));
        c.feed(&text(2.0, " answer\n</CHAT_REPLY_007>\nkept tail"));
        // Unclosed block: dropped at turn end (it may be posted as a reply).
        c.feed(&text(3.0, "<CHAT_REPLY_008>\nnever closed, still trace"));
        let posts = c.feed(&Event::TurnEnd { at: Some(4.0) });
        let joined = posts
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!joined.contains("posted"), "{joined}");
        assert!(joined.contains("kept tail"), "{joined}");
        assert!(!joined.contains("never closed"), "{joined}");
    }

    #[test]
    fn batched_prompts_are_all_labelled() {
        let mut c = Collector::new(Duration::from_secs(45));
        c.feed(&prompt(0.0, REAL_HEADER));
        c.feed(&prompt(
            1.0,
            "[from lane-a via agentctl · reply: agentctl send lane-a]\nb",
        ));
        c.feed(&prompt(2.0, "[from agent-b via agentctl]\nc"));
        c.feed(&text(3.0, "Working notes from a batched turn."));
        let posts = c.feed(&Event::TurnEnd { at: Some(4.0) });
        assert_eq!(posts.len(), 1);
        assert_eq!(
            posts[0].trigger,
            "reply CHAT_REPLY_007, agent lane-a, agent agent-b"
        );
        assert_eq!(posts[0].text, "Working notes from a batched turn.");
    }

    #[test]
    fn failure_classification_and_omission_notes() {
        use super::PostFailure::*;
        assert_eq!(classify_failure("x", "nope", true, false), Drop);
        assert_eq!(classify_failure("x", "nope", true, true), Retry);
        assert_eq!(classify_failure("x", "nope", false, false), Retry);
        assert_eq!(
            classify_failure("thread_not_found", "", true, true),
            ThreadGone
        );
        assert_eq!(
            classify_failure("send", "thread not found", false, true),
            Retry
        );
        assert_eq!(note_omission("body", 3), "[3 trace posts omitted]\nbody");
        assert_eq!(
            note_omission("[3 trace posts omitted]\nbody", 2),
            "[5 trace posts omitted]\nbody"
        );
    }

    #[test]
    fn held_cap_drops_oldest() {
        let mut held: Vec<TracePost> = (0..5)
            .map(|i| TracePost {
                text: format!("p{i}"),
                trigger: "prompt".to_owned(),
            })
            .collect();
        assert_eq!(cap_held(&mut held, 3), 2);
        assert_eq!(held[0].text, "p2");
        assert_eq!(cap_held(&mut held, 3), 0);
    }

    #[test]
    fn bare_names_and_relative_links_are_escaped_outside_code() {
        assert_eq!(
            escape_for_chat("Fixed preemptions.rs and rs/agentctl/src/cli.rs."),
            "Fixed `preemptions.rs` and `rs/agentctl/src/cli.rs`."
        );
        assert_eq!(
            escape_for_chat("See [the note](ai_docs/2026-11-03-x.md) for more."),
            "See the note (`ai_docs/2026-11-03-x.md`) for more."
        );
        // Absolute links, anchors, addresses, code and ordinary words are left alone.
        for kept in [
            "Read [the PR](https://github.com/o/r/pull/1) and [above](#top).",
            "Mail a@b.io or open https://example.io/x.rs today.",
            "Already `cli.rs` in code, e.g. here, version 1.5, done.",
        ] {
            assert_eq!(escape_for_chat(kept), kept);
        }
        assert_eq!(
            escape_for_chat("[cli.rs](https://x.example/cli.rs)"),
            "[`cli.rs`](https://x.example/cli.rs)"
        );
        let fenced = "```\nlet p = \"preemptions.rs\";\n```\nthen notes.md";
        assert_eq!(
            escape_for_chat(fenced),
            "```\nlet p = \"preemptions.rs\";\n```\nthen `notes.md`"
        );
        // The collector posts the escaped text.
        let mut c = Collector::new(Duration::from_secs(45));
        c.feed(&prompt(0.0, REAL_HEADER));
        c.feed(&text(
            1.0,
            "Edited chat_trace.rs per [the issue](.minibeads/issues/x.md).",
        ));
        let posts = c.feed(&Event::TurnEnd { at: Some(2.0) });
        assert_eq!(
            posts[0].text,
            "Edited `chat_trace.rs` per the issue (`.minibeads/issues/x.md`)."
        );
    }

    #[test]
    fn trace_metadata_marks_roots_and_entries() {
        let root = trace_metadata(
            "root",
            "coordinator",
            Some("agent-utils"),
            Some("s-1"),
            "2026-11-03",
            0,
            None,
        );
        assert_eq!(
            root,
            serde_json::json!({"event_type": "agentctl_trace", "event_payload": {
                "v": 1, "kind": "root", "agent": "coordinator", "team": "agent-utils",
                "session": "s-1", "day": "2026-11-03", "seq": 0, "trigger": null}})
        );
        let entry = trace_metadata(
            "entry",
            "coordinator",
            None,
            None,
            "2026-11-03",
            7,
            Some("owner message"),
        );
        assert_eq!(entry["event_payload"]["kind"], "entry");
        assert_eq!(entry["event_payload"]["seq"], 7);
        assert_eq!(entry["event_payload"]["trigger"], "owner message");
        assert!(entry["event_payload"]["team"].is_null());
    }

    #[test]
    fn compact_code_block_does_not_stick_the_fence_state() {
        let code = "```\nlet x = 1;\n```";
        let body = format!("{code}\n\n{}", "word ".repeat(2_000));
        let parts = split_long(&body, 3_500);
        assert!(
            parts.iter().all(|p| p.len() <= 3_500),
            "{:?}",
            parts.iter().map(String::len).collect::<Vec<_>>()
        );
        assert!(parts.len() > 1);
    }

    #[test]
    fn operation_ids_are_uuids_and_thread_ids_well_formed() {
        let uuid = crate::chat_runtime::random_operation_uuid().unwrap();
        assert!(crate::chat_runtime::validate_operation_uuid(&uuid).is_ok());
        assert_eq!(trace_thread_id("C1", "C1/messages/1.5"), "C1/threads/1.5");
        assert_eq!(trace_thread_id("C1", "1.5"), "C1/threads/1.5");
    }

    #[test]
    fn window_flush_thread_id_budget_and_et_dates() {
        let mut c = Collector::new(Duration::from_secs(45));
        c.feed(&prompt(0.0, "unlabelled work"));
        c.feed(&text(10.0, "Held text with substance."));
        assert!(c.tick(50.0).is_empty());
        assert_eq!(c.tick(56.0).len(), 1);
        assert_eq!(
            trace_thread_id("C123", "C123/messages/999.5"),
            "C123/threads/999.5"
        );
        let mut budget = RateBudget::new(2);
        assert!(budget.take(0.0) && budget.take(1.0) && !budget.take(2.0) && budget.take(61.0));
        assert!(is_new(Some(10.0), 5.0, 0.0) && !is_new(Some(4.0), 5.0, 0.0));
        // Multibyte text splits without panicking.
        let wide = "é".repeat(5_000);
        assert!(split_long(&wide, 3_500).iter().all(|p| p.len() <= 3_500));
        // 2026-10-30 12:00 UTC is 08:00 EDT, same ET date.
        assert_eq!(et_date(1_793_361_600.0), "2026-10-30");
        // DST ends 2026-11-01 02:00 EST = 07:00 UTC; 06:30 UTC is 01:30 EST? (still Nov 1 ET)
        assert_eq!(et_date(1_793_514_600.0), "2026-11-01");
        // 2026-11-01 04:30 UTC is 00:30 EDT on Nov 1 (fallback is 06:00 UTC).
        assert_eq!(et_date(1_793_507_400.0), "2026-11-01");
        // 2026-11-01 08:00 UTC is 03:00 EST on Nov 1.
        assert_eq!(et_date(1_793_520_000.0), "2026-11-01");
        // DST starts 2026-03-08 02:00 EST = 07:00 UTC; 04:00 UTC is Mar 7 23:00 EST.
        assert_eq!(et_date(1_772_942_400.0), "2026-03-07");
    }
}
