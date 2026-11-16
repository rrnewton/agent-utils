//! Ready links for chat threads, messages and tracker issues (#658
//! chat-bridge-permalinks; design: `ai_docs/plans/2026-11-15-thread-and-message-short-refs.md`).
//!
//! A permalink is built LOCALLY from the bridge's configured workspace origin,
//! the channel id and the `ts` the bridge already holds — never by asking the
//! provider, so nothing on the delivery path makes a network call. An issue
//! link substitutes the number into the configured `issue_url_template`, which
//! names minibeads files in this repository's layout and plain tracker issues
//! alike (`.../project-{n}.md`, `.../issues/{n}`).
//!
//! Agents cite with tagged references only — `:thread:060`, `:email:1846`,
//! `:issue:1234` — and [`render_reply_links`] rewrites only those, outside
//! code fences and inline code. Plain numbers and bare `#1234` are never
//! rewritten by the token pass: they mean other things too often. The one
//! exception is the bare-number guard in the same pass: `thread NNN`,
//! `reply NNN` or `per your NNNN` naming a thread this bridge retains is
//! rewritten to the linked tagged form and reported; anything else that looks
//! like a bare thread reference is reported, never rewritten.

/// The longest link formed here, in bytes; real ones are well under two hundred.
const MAX_URL_BYTES: usize = 512;
/// Most warnings one rendered reply reports; the rest are counted, not listed.
const MAX_WARNINGS: usize = 16;

/// A Slack workspace's origin from `url` (`https://acme.slack.com/`): scheme,
/// host and port, no path. `None` for anything that is not an `https` URL with
/// a host — a link to hand a reader is never formed on any other scheme.
#[must_use]
pub fn workspace_origin(url: &str) -> Option<String> {
    let rest = url.trim().strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty()
        || authority.len() > 253
        || authority
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control() || byte == b'@')
    {
        return None;
    }
    if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
    }
    Some(format!("https://{authority}"))
}

/// Whether `id` has the shape of a Slack conversation id: `C`, `G` or `D`, then
/// eight or more upper-case letters and digits. Mirrors vibe-talk's
/// `slack::ids::is_conversation_id`; the two must agree on what a channel is.
#[must_use]
pub fn is_conversation_id(id: &str) -> bool {
    id.len() >= 9
        && id.len() <= 32
        && matches!(id.as_bytes()[0], b'C' | b'G' | b'D')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

/// Normalise a Slack `ts` to its canonical six-digit-fraction form, refusing a
/// malformed one. Mirrors vibe-talk's `slack::ids::canonical_ts`.
#[must_use]
pub fn canonical_ts(ts: &str) -> Option<String> {
    let (secs, frac) = match ts.split_once('.') {
        Some((secs, frac)) if !frac.is_empty() => (secs, frac),
        Some(_) => return None,
        None => (ts, ""),
    };
    if secs.is_empty()
        || secs.len() > 12
        || frac.len() > 6
        || !secs.bytes().all(|byte| byte.is_ascii_digit())
        || !frac.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let mut fraction = frac.to_owned();
    while fraction.len() < 6 {
        fraction.push('0');
    }
    Some(format!("{secs}.{fraction}"))
}

/// The bare Slack `ts` a stored thread or message id names in
/// `conversation`: the id itself when it is a bare `ts`, or the `ts` of a
/// provider-qualified `<conversation>/threads/<ts>` or
/// `<conversation>/messages/<ts>` — the shapes real bridge records store
/// (#1705). `None` when the embedded conversation is not `conversation` or
/// the id has any other shape: a permalink is never built from an id whose
/// thread it cannot prove.
#[must_use]
pub fn bare_ts(id: &str, conversation: &str) -> Option<String> {
    if let Some(ts) = canonical_ts(id) {
        return Some(ts);
    }
    ["threads", "messages"].iter().find_map(|kind| {
        id.strip_prefix(&format!("{conversation}/{kind}/"))
            .and_then(canonical_ts)
    })
}

/// The permalink segment a `ts` names: its ten second digits and six
/// microsecond digits, or `None` for any other shape — nothing Slack resolves.
fn permalink_segment(ts: &str) -> Option<String> {
    let ts = canonical_ts(ts)?;
    let segment: String = ts.chars().filter(|ch| *ch != '.').collect();
    (segment.len() == 16).then_some(segment)
}

/// A Slack thread's permalink on the workspace at `workspace` (an origin, see
/// [`workspace_origin`]): the thread root `ts` in `conversation`. `None` when
/// any part cannot be read, so an unresolvable thread gets no link rather than
/// a plausible one.
#[must_use]
pub fn slack_thread_permalink(
    workspace: &str,
    conversation: &str,
    thread_root_ts: &str,
) -> Option<String> {
    let origin = workspace_origin(workspace)?;
    if !is_conversation_id(conversation) {
        return None;
    }
    let segment = permalink_segment(&bare_ts(thread_root_ts, conversation)?)?;
    let url = format!("{origin}/archives/{conversation}/p{segment}");
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

/// A Slack message's permalink: its own `ts`, carrying its thread root when it
/// is a reply there (`?thread_ts=…&cid=…`, the shape Slack opens at the reply).
/// `None` when any part cannot be read.
#[must_use]
pub fn slack_message_permalink(
    workspace: &str,
    conversation: &str,
    ts: &str,
    thread_root_ts: Option<&str>,
) -> Option<String> {
    let origin = workspace_origin(workspace)?;
    if !is_conversation_id(conversation) {
        return None;
    }
    let ts = bare_ts(ts, conversation)?;
    let segment = permalink_segment(&ts)?;
    let mut url = format!("{origin}/archives/{conversation}/p{segment}");
    if let Some(root) = thread_root_ts {
        let root = bare_ts(root, conversation)?;
        if root == ts || permalink_segment(&root).is_none() {
            return None;
        }
        url.push_str(&format!("?thread_ts={root}&cid={conversation}"));
    }
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

/// Why an `issue_url_template` cannot be used, or `None` when it can: it must
/// be one `https` address holding exactly one `{n}` placeholder and no other
/// brace, whitespace or control character.
#[must_use]
pub fn issue_template_problem(template: &str) -> Option<String> {
    if template.len() > MAX_URL_BYTES {
        return Some(format!(
            "issue_url_template must be at most {MAX_URL_BYTES} bytes"
        ));
    }
    if workspace_origin(template).is_none() {
        return Some("issue_url_template must be an https URL".to_owned());
    }
    if template.matches("{n}").count() != 1
        || template
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control())
        || template.replace("{n}", "").contains(['{', '}'])
    {
        return Some(
            "issue_url_template must hold exactly one {n} placeholder and no other brace"
                .to_owned(),
        );
    }
    None
}

/// The link for issue `number` under `template`: the template with its `{n}`
/// replaced. `None` when the template is unusable.
#[must_use]
pub fn issue_url(template: &str, number: u64) -> Option<String> {
    if number == 0 || issue_template_problem(template).is_some() {
        return None;
    }
    let url = template.replace("{n}", &number.to_string());
    (url.len() <= MAX_URL_BYTES).then_some(url)
}

/// How a thread's short number is written: at least three digits, zero padded
/// (`060`). This is the reply alias's own spelling, so `:thread:060`, the reply
/// fence, the prompt and `chat thread NNN` all write the number the same way.
#[must_use]
pub fn thread_label(alias: u64) -> String {
    format!("{alias:03}")
}

/// One rendered reply: the wire text, what could not be linked, and what the
/// bare-number guard rewrote or noticed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RenderedLinks {
    /// The reply with every resolvable reference in Slack link markup.
    pub text: String,
    /// One warning per unresolvable tagged reference, in order.
    pub warnings: Vec<String>,
    /// One notice per bare-number guard hit (rewritten or report-only).
    pub notices: Vec<String>,
}

/// Rewrite the references in `body` for the wire. `thread_link` and
/// `message_link` resolve a short number to its locally built permalink;
/// `issue_template` is the bridge's configured template, when any. A reference
/// that cannot be resolved stays as its tagged plain text and earns a warning;
/// the caller decides where warnings and notices go, and the send itself never
/// fails over a link.
#[must_use]
pub fn render_reply_links(
    body: &str,
    thread_link: &dyn Fn(u64) -> Option<String>,
    message_link: &dyn Fn(u64) -> Option<String>,
    issue_template: Option<&str>,
) -> RenderedLinks {
    let mut renderer = Renderer {
        thread_link,
        message_link,
        issue_template,
        warnings: Vec::new(),
        notices: Vec::new(),
        suppressed: 0,
    };
    let mut text = String::with_capacity(body.len());
    let mut in_fence = false;
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            text.push_str(line);
            continue;
        }
        if in_fence {
            text.push_str(line);
            continue;
        }
        renderer.render_line(line, &mut text);
    }
    if renderer.suppressed > 0 {
        renderer.warnings.push(format!(
            "and {} more link warning(s) like these",
            renderer.suppressed
        ));
    }
    RenderedLinks {
        text,
        warnings: renderer.warnings,
        notices: renderer.notices,
    }
}

/// Drop Markdown link titles from `body`: `[label](<url> "title")` and
/// `[label](url "title")` become `[label](url)`. A title can hold a copied
/// tooltip with markup of its own inside (`"Opens here — <url|label>"`), which
/// Slack's vendor Markdown rendering mangles; with no title there is nothing
/// to mangle. Code fences and inline code are left as written.
#[must_use]
pub fn strip_link_titles(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    for (index, line) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        strip_line_titles(line, &mut out);
    }
    out
}

/// Append `line` to `out` with inline code copied verbatim and link titles
/// dropped outside it (the line-level twin of [`Renderer::render_line`]).
fn strip_line_titles(line: &str, out: &mut String) {
    let mut rest = line;
    loop {
        let Some(open) = rest.find('`') else {
            strip_span_titles(rest, out);
            return;
        };
        let run = rest[open..].chars().take_while(|ch| *ch == '`').count();
        let ticks: String = std::iter::repeat_n('`', run).collect();
        let after = &rest[open + run..];
        match after.find(&ticks) {
            Some(close) => {
                strip_span_titles(&rest[..open], out);
                out.push_str(&rest[open..open + run + close + run]);
                rest = &after[close + run..];
            }
            None => {
                strip_span_titles(&rest[..open], out);
                out.push_str(&rest[open..]);
                return;
            }
        }
    }
}

/// Drop the title of every `[…](…)` link in one span of ordinary text.
fn strip_span_titles(span: &str, out: &mut String) {
    let bytes = span.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b']' && bytes.get(index + 1) == Some(&b'(') {
            if let Some((url, end)) = parse_link_destination(&span[index + 2..]) {
                out.push_str("](");
                out.push_str(&url);
                out.push(')');
                index += 2 + end;
                continue;
            }
        }
        let ch_len = span[index..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&span[index..index + ch_len]);
        index += ch_len;
    }
}

/// The destination of a Markdown link at the start of `rest` (just after
/// `](`): its bare URL and the byte count consumed through the closing `)`,
/// with any quoted title dropped. `None` when no link destination follows.
fn parse_link_destination(rest: &str) -> Option<(String, usize)> {
    let mut position = 0;
    let bytes = rest.as_bytes();
    while bytes.get(position).is_some_and(u8::is_ascii_whitespace) {
        position += 1;
    }
    let url = if bytes.get(position) == Some(&b'<') {
        let close = rest[position..].find('>')? + position;
        let url = &rest[position + 1..close];
        position = close + 1;
        url.to_owned()
    } else {
        let start = position;
        while let Some(byte) = bytes.get(position) {
            if byte.is_ascii_whitespace() || *byte == b')' {
                break;
            }
            position += 1;
        }
        rest[start..position].to_owned()
    };
    if url.is_empty() {
        return None;
    }
    let mut probe = position;
    while bytes.get(probe).is_some_and(u8::is_ascii_whitespace) {
        probe += 1;
    }
    // A quoted title follows the destination; drop through its close quote.
    if matches!(bytes.get(probe), Some(b'"') | Some(b'\'')) {
        let quote = bytes[probe];
        let close = rest[probe + 1..]
            .find(quote as char)
            .map(|end| probe + 1 + end)?;
        probe = close + 1;
        while bytes.get(probe).is_some_and(u8::is_ascii_whitespace) {
            probe += 1;
        }
    }
    if bytes.get(probe) != Some(&b')') {
        return None;
    }
    Some((url, probe + 1))
}

struct Renderer<'a> {
    thread_link: &'a dyn Fn(u64) -> Option<String>,
    message_link: &'a dyn Fn(u64) -> Option<String>,
    issue_template: Option<&'a str>,
    warnings: Vec<String>,
    notices: Vec<String>,
    suppressed: usize,
}

impl Renderer<'_> {
    fn warn(&mut self, warning: String) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(warning);
        } else {
            self.suppressed += 1;
        }
    }

    /// Append `line` to `out`, copying inline code spans verbatim and
    /// rewriting references elsewhere.
    fn render_line(&mut self, line: &str, out: &mut String) {
        let mut rest = line;
        loop {
            let Some(open) = rest.find('`') else {
                self.render_span(rest, out);
                return;
            };
            let run = rest[open..].chars().take_while(|ch| *ch == '`').count();
            let ticks: String = std::iter::repeat_n('`', run).collect();
            let after = &rest[open + run..];
            match after.find(&ticks) {
                Some(close) => {
                    self.render_span(&rest[..open], out);
                    out.push_str(&rest[open..open + run + close + run]);
                    rest = &after[close + run..];
                }
                None => {
                    // An unclosed run codes the rest of the line.
                    self.render_span(&rest[..open], out);
                    out.push_str(&rest[open..]);
                    return;
                }
            }
        }
    }

    /// Rewrite every reference in one span of ordinary text: tagged tokens
    /// first-class, and the bare-number guard over what remains.
    fn render_span(&mut self, span: &str, out: &mut String) {
        let bytes = span.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            // A tagged reference: `:thread:…`, `:email:…` or `:issue:…`. Not
            // inside a word or a fence name (`CHAT_REPLY_060` is the
            // fence quoting itself, never a citation).
            if bytes[index] == b':'
                && (index == 0
                    || !(bytes[index - 1].is_ascii_alphanumeric()
                        || matches!(bytes[index - 1], b'_' | b':')))
            {
                if let Some((rendered, consumed)) = self.render_tagged(&span[index..]) {
                    out.push_str(&rendered);
                    index += consumed;
                    continue;
                }
            }
            // The guard's word forms: `thread NNN`, `reply NNN`, `per your NNNN`.
            if matches!(bytes[index], b't' | b'T' | b'r' | b'R' | b'p' | b'P') {
                if let Some((rendered, consumed)) = self.render_guard_phrase(span, index) {
                    out.push_str(&rendered);
                    index += consumed;
                    continue;
                }
            }
            // The guard's report-only form: `(NNN)`.
            if bytes[index] == b'(' {
                let inner = &span[index + 1..];
                let digits = inner.bytes().take_while(u8::is_ascii_digit).count();
                if digits == 3 && inner.as_bytes().get(digits) == Some(&b')') {
                    self.notices.push(format!(
                        "bare thread reference ({}) left as written; cite it as :thread:{}",
                        &inner[..digits],
                        &inner[..digits]
                    ));
                    out.push_str(&span[index..index + 1 + digits + 1]);
                    index += 1 + digits + 1;
                    continue;
                }
            }
            let ch_len = span[index..].chars().next().map_or(1, char::len_utf8);
            out.push_str(&span[index..index + ch_len]);
            index += ch_len;
        }
    }

    /// One tagged reference at the start of `rest`: its replacement and byte
    /// length, or `None` when `rest` opens with no well-formed tag (the text
    /// is then copied as written, silently — someone else's `:word:`).
    fn render_tagged(&mut self, rest: &str) -> Option<(String, usize)> {
        for (prefix, kind) in [
            (":thread:", Kind::Thread),
            (":email:", Kind::Message),
            (":issue:", Kind::Issue),
        ] {
            let Some(payload) = rest.strip_prefix(prefix) else {
                continue;
            };
            let digits = payload.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return None;
            }
            let number: u64 = payload[..digits].parse().ok()?;
            let consumed = prefix.len() + digits;
            return Some(match kind {
                Kind::Thread => {
                    // Thread numbers are written as the alias is: zero padded.
                    if payload[..digits] != thread_label(number) || number == 0 {
                        return None;
                    }
                    let label = format!(":thread:{}", thread_label(number));
                    match (self.thread_link)(number) {
                        Some(url) => (format!("<{url}|{label}>"), consumed),
                        None => {
                            self.warn(format!(
                                "{label} has no local permalink; shown as plain text"
                            ));
                            (label, consumed)
                        }
                    }
                }
                Kind::Message => {
                    if payload[..digits] != number.to_string() || number == 0 {
                        return None;
                    }
                    let label = format!(":email:{number}");
                    match (self.message_link)(number) {
                        Some(url) => (format!("<{url}|{label}>"), consumed),
                        None => {
                            self.warn(format!(
                                "{label} has no local permalink; shown as plain text"
                            ));
                            (label, consumed)
                        }
                    }
                }
                Kind::Issue => {
                    if payload[..digits] != number.to_string() || number == 0 {
                        return None;
                    }
                    let label = format!(":issue:{number}");
                    match self.issue_template.and_then(|t| issue_url(t, number)) {
                        Some(url) => (format!("<{url}|{label}>"), consumed),
                        None => {
                            self.warn(format!(
                                "{label} cannot be linked (no usable issue_url_template); shown as plain text"
                            ));
                            (label, consumed)
                        }
                    }
                }
            });
        }
        None
    }

    /// The guard's word forms at `index` in `span`: `thread NNN`, `reply NNN`
    /// or `per your NNNN` at a word boundary, with 3–4 digits after it. A form
    /// naming a retained thread is rewritten to the linked tagged reference; a
    /// form naming nothing retained is copied and reported. `None` when no
    /// form starts here.
    fn render_guard_phrase(&mut self, span: &str, index: usize) -> Option<(String, usize)> {
        let bytes = span.as_bytes();
        if index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_') {
            return None;
        }
        let rest = &span[index..];
        let phrase = ["per your ", "thread ", "reply "]
            .into_iter()
            .find(|phrase| {
                rest.len() >= phrase.len() && rest[..phrase.len()].eq_ignore_ascii_case(phrase)
            })?;
        let digits_start = phrase.len();
        let digit_len = rest[digits_start..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if !(3..=4).contains(&digit_len) {
            return None;
        }
        let after = rest[digits_start + digit_len..].chars().next();
        if after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return None;
        }
        let digits = &rest[digits_start..digits_start + digit_len];
        let alias: u64 = digits.parse().ok()?;
        let prefix = &rest[..digits_start];
        let label = format!(":thread:{}", thread_label(alias));
        match (self.thread_link)(alias) {
            Some(url) => {
                self.notices.push(format!(
                    "bare thread reference \"{prefix}{digits}\" rewritten as {label}"
                ));
                Some((format!("{prefix}<{url}|{label}>"), digits_start + digit_len))
            }
            None => {
                self.notices.push(format!(
                    "possible bare thread reference \"{prefix}{digits}\" left as written; cite it as {label}"
                ));
                Some((
                    rest[..digits_start + digit_len].to_owned(),
                    digits_start + digit_len,
                ))
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Thread,
    Message,
    Issue,
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "https://acme.slack.com";
    const TEMPLATE: &str =
        "https://github.com/OWNER/REPO/blob/main/.minibeads/issues/project-{n}.md";

    fn thread_link(alias: u64) -> Option<String> {
        (alias == 60)
            .then(|| slack_thread_permalink(WORKSPACE, "C0123ABCD", "1700000000.000100"))
            .flatten()
    }

    fn message_link(number: u64) -> Option<String> {
        (number == 1846)
            .then(|| {
                slack_message_permalink(
                    WORKSPACE,
                    "C0123ABCD",
                    "1700000050.000300",
                    Some("1700000000.000100"),
                )
            })
            .flatten()
    }

    fn render(body: &str) -> RenderedLinks {
        render_reply_links(body, &thread_link, &message_link, Some(TEMPLATE))
    }

    #[test]
    fn a_thread_permalink_names_the_thread_root() {
        assert_eq!(
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "1700000000.000100"),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000000100".to_owned())
        );
        assert_eq!(
            slack_thread_permalink("https://acme.slack.com/", "C0123ABCD", "1700000000.1"),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000100000".to_owned())
        );
        assert_eq!(
            slack_message_permalink(
                WORKSPACE,
                "C0123ABCD",
                "1700000050.000300",
                Some("1700000000.000100")
            ),
            Some(
                "https://acme.slack.com/archives/C0123ABCD/p1700000050000300?thread_ts=1700000000.000100&cid=C0123ABCD"
                    .to_owned()
            )
        );
    }

    #[test]
    fn qualified_ids_name_the_same_permalinks_as_bare_ones() {
        // Real bridge records qualify both ids with their conversation
        // (#1705): the permalink must be the bare form's.
        assert_eq!(
            slack_thread_permalink(
                WORKSPACE,
                "C0CC755QYSK",
                "C0CC755QYSK/threads/1794772223.198289"
            ),
            Some("https://acme.slack.com/archives/C0CC755QYSK/p1794772223198289".to_owned())
        );
        assert_eq!(
            slack_message_permalink(
                WORKSPACE,
                "C0CC755QYSK",
                "C0CC755QYSK/messages/1794772300.000200",
                Some("C0CC755QYSK/threads/1794772223.198289"),
            ),
            Some(
                "https://acme.slack.com/archives/C0CC755QYSK/p1794772300000200?thread_ts=1794772223.198289&cid=C0CC755QYSK"
                    .to_owned()
            )
        );
        // A top-level message's own permalink carries no thread query.
        assert_eq!(
            slack_message_permalink(
                WORKSPACE,
                "C0CC755QYSK",
                "C0CC755QYSK/messages/1794772223.198289",
                None,
            ),
            Some("https://acme.slack.com/archives/C0CC755QYSK/p1794772223198289".to_owned())
        );
    }

    #[test]
    fn bare_ts_reads_both_stored_shapes_and_refuses_the_rest() {
        let conversation = "C0CC755QYSK";
        assert_eq!(
            bare_ts("1794772223.198289", conversation).as_deref(),
            Some("1794772223.198289")
        );
        assert_eq!(
            bare_ts("1794772223.1", conversation).as_deref(),
            Some("1794772223.100000")
        );
        assert_eq!(
            bare_ts("C0CC755QYSK/threads/1794772223.198289", conversation).as_deref(),
            Some("1794772223.198289")
        );
        assert_eq!(
            bare_ts("C0CC755QYSK/messages/1794772223.1", conversation).as_deref(),
            Some("1794772223.100000")
        );
        // The embedded conversation must be the message's own.
        assert_eq!(
            bare_ts("C999999999/threads/1794772223.198289", conversation),
            None
        );
        assert_eq!(
            bare_ts("C999999999/messages/1794772223.198289", conversation),
            None
        );
        // Any other shape is refused.
        assert_eq!(
            bare_ts("C0CC755QYSK/channels/1794772223.198289", conversation),
            None
        );
        assert_eq!(
            bare_ts("C0CC755QYSK/threads/1794772223.198289/extra", conversation),
            None
        );
        assert_eq!(bare_ts("C0CC755QYSK/threads/soon", conversation), None);
        assert_eq!(bare_ts("threads/1794772223.198289", conversation), None);
        assert_eq!(bare_ts("", conversation), None);
    }

    #[test]
    fn a_qualified_id_for_another_conversation_forms_no_permalink() {
        assert_eq!(
            slack_thread_permalink(
                WORKSPACE,
                "C0CC755QYSK",
                "C999999999/threads/1794772223.198289"
            ),
            None
        );
        assert_eq!(
            slack_message_permalink(
                WORKSPACE,
                "C0CC755QYSK",
                "C999999999/messages/1794772223.198289",
                None,
            ),
            None
        );
    }

    #[test]
    fn an_unreadable_part_forms_no_permalink() {
        let good = "1700000000.000100";
        assert_eq!(slack_thread_permalink("", "C0123ABCD", good), None);
        assert_eq!(
            slack_thread_permalink("http://acme.slack.com", "C0123ABCD", good),
            None
        );
        assert_eq!(slack_thread_permalink(WORKSPACE, "general", good), None);
        assert_eq!(slack_thread_permalink(WORKSPACE, "C0123ABCD", "soon"), None);
        assert_eq!(
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "17000000000.000100"),
            None
        );
        // A reply named as its own root is no reply shape.
        assert_eq!(
            slack_message_permalink(WORKSPACE, "C0123ABCD", good, Some(good)),
            None
        );
    }

    #[test]
    fn the_issue_template_takes_exactly_one_number() {
        assert_eq!(
            issue_url(TEMPLATE, 658),
            Some(
                "https://github.com/OWNER/REPO/blob/main/.minibeads/issues/project-658.md"
                    .to_owned()
            )
        );
        assert_eq!(issue_url(TEMPLATE, 0), None);
        assert!(issue_template_problem("http://example.com/{n}").is_some());
        assert!(issue_template_problem("https://example.com/issues/").is_some());
        assert!(issue_template_problem("https://example.com/{n}/{n}").is_some());
        assert!(issue_template_problem(TEMPLATE).is_none());
    }

    #[test]
    fn known_references_render_as_links() {
        let rendered = render("See :thread:060, :email:1846 and :issue:658.");
        assert_eq!(
            rendered.text,
            "See <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|:thread:060>, <https://acme.slack.com/archives/C0123ABCD/p1700000050000300?thread_ts=1700000000.000100&cid=C0123ABCD|:email:1846> and <https://github.com/OWNER/REPO/blob/main/.minibeads/issues/project-658.md|:issue:658>."
        );
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        assert!(rendered.notices.is_empty(), "{:?}", rendered.notices);
    }

    #[test]
    fn unknown_references_stay_tagged_and_warn() {
        let rendered = render("Back to :thread:999 and :email:9999.");
        assert_eq!(rendered.text, "Back to :thread:999 and :email:9999.");
        assert_eq!(rendered.warnings.len(), 2, "{:?}", rendered.warnings);
        let rendered = render_reply_links("See :issue:658.", &thread_link, &message_link, None);
        assert_eq!(rendered.text, "See :issue:658.");
        assert_eq!(rendered.warnings.len(), 1);
    }

    #[test]
    fn plain_numbers_and_bare_issue_references_are_left_alone() {
        let body = "Result 6114 closes #658; the fence is CHAT_REPLY_060 and :other:060 stays.";
        let rendered = render(body);
        assert_eq!(rendered.text, body);
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        assert!(rendered.notices.is_empty(), "{:?}", rendered.notices);
    }

    #[test]
    fn the_guard_rewrites_known_word_forms_and_only_reports_the_rest() {
        let rendered = render("As said in thread 060 and per your 0060, see (061) and thread 999.");
        assert_eq!(
            rendered.text,
            "As said in thread <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|:thread:060> and per your <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|:thread:060>, see (061) and thread 999."
        );
        // Two rewrites, one parenthesised report, one unresolved word form.
        assert_eq!(rendered.notices.len(), 4, "{:?}", rendered.notices);
    }

    #[test]
    fn references_in_code_are_left_alone() {
        let body =
            "Run `:thread:060` first.\n```\n:issue:658 and thread 060\n```\nThen :thread:060.";
        let rendered = render(body);
        assert_eq!(
            rendered.text,
            "Run `:thread:060` first.\n```\n:issue:658 and thread 060\n```\nThen <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|:thread:060>."
        );
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
        assert!(rendered.notices.is_empty(), "{:?}", rendered.notices);
    }

    #[test]
    fn link_titles_are_stripped_outside_code() {
        let body = "Read [the doc](<https://example.com/doc.md> \"Opens here — <https://example.com/doc.md>\") and [x](https://example.com \"t\"); keep `[a](https://e \"t\")`.";
        assert_eq!(
            strip_link_titles(body),
            "Read [the doc](https://example.com/doc.md) and [x](https://example.com); keep `[a](https://e \"t\")`."
        );
        // No title, no change.
        assert_eq!(strip_link_titles("[a](https://e)"), "[a](https://e)");
    }
}
