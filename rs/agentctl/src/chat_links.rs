//! Ready links for chat threads and tracker issues (#658 chat-bridge-permalinks).
//!
//! A thread's permalink is built LOCALLY from the bridge's configured workspace
//! origin, the message's channel id and its thread root `ts` — never by asking
//! the provider, so nothing on the delivery path makes a network call. An issue
//! link substitutes the number into the configured `issue_url_template`, which
//! names minibeads files in this repository's layout and plain tracker issues
//! alike (`.../agent-utils-{n}.md`, `.../issues/{n}`).
//!
//! Agents cite threads and issues in replies with explicit tokens,
//! `[[thread:060]]` and `[[issue:1234]]`; [`render_reply_links`] rewrites only
//! those, outside code fences and inline code. Plain numbers and bare `#1234`
//! are never rewritten: `060`, `6114` and `#1234` in ordinary prose keep
//! meaning other things, and guessing would link them wrongly.

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
    let ts = canonical_ts(thread_root_ts)?;
    let segment: String = ts.chars().filter(|ch| *ch != '.').collect();
    // A permalink segment is ten second digits and six microsecond digits —
    // what Slack mints, and what its links carry. Any other length names
    // nothing Slack would resolve.
    if segment.len() != 16 {
        return None;
    }
    let url = format!("{origin}/archives/{conversation}/p{segment}");
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
/// (`060`). This is the reply alias's own spelling, so a token naming a thread
/// uses the number its prompt and `chat thread NNN` already use.
#[must_use]
pub fn thread_label(alias: u64) -> String {
    format!("{alias:03}")
}

/// One rendered reply: the wire text, and what could not be linked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedLinks {
    /// The reply with every resolvable token replaced by Slack link markup.
    pub text: String,
    /// One warning per unresolvable or malformed token, in order.
    pub warnings: Vec<String>,
}

/// Rewrite the link tokens in `body` for the wire. `thread_link` resolves a
/// thread number to its locally built permalink; `issue_template` is the
/// bridge's configured template, when any. A token that cannot be resolved
/// stays as plain text (the number alone) and earns a warning; the caller
/// decides where warnings go, and the send itself never fails over a link.
#[must_use]
pub fn render_reply_links(
    body: &str,
    thread_link: &dyn Fn(u64) -> Option<String>,
    issue_template: Option<&str>,
) -> RenderedLinks {
    let mut renderer = Renderer {
        thread_link,
        issue_template,
        warnings: Vec::new(),
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
    }
}

struct Renderer<'a> {
    thread_link: &'a dyn Fn(u64) -> Option<String>,
    issue_template: Option<&'a str>,
    warnings: Vec<String>,
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
    /// rewriting tokens elsewhere.
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

    /// Rewrite every token in one span of ordinary text.
    fn render_span(&mut self, span: &str, out: &mut String) {
        let mut rest = span;
        while let Some(open) = rest.find("[[") {
            let Some(close) = rest[open + 2..].find("]]").map(|end| open + 2 + end) else {
                break;
            };
            let inner = &rest[open + 2..close];
            let Some(rendered) = self.render_token(inner) else {
                // Not one of our tokens: copy through its opening and move on.
                out.push_str(&rest[..open + 2]);
                rest = &rest[open + 2..];
                continue;
            };
            out.push_str(&rest[..open]);
            out.push_str(&rendered);
            rest = &rest[close + 2..];
        }
        out.push_str(rest);
    }

    /// One token's replacement, or `None` when `inner` is not a token at all
    /// (someone else's `[[...]]`, left exactly as written, silently).
    fn render_token(&mut self, inner: &str) -> Option<String> {
        if let Some(payload) = inner.strip_prefix("thread:") {
            if payload.is_empty() || !payload.bytes().all(|byte| byte.is_ascii_digit()) {
                self.warn(format!(
                    "malformed thread link token [[{inner}]] left as written"
                ));
                return Some(format!("[[{inner}]]"));
            }
            let alias: u64 = payload.parse().ok()?;
            if payload != thread_label(alias) || alias == 0 {
                return None;
            }
            return Some(match (self.thread_link)(alias) {
                Some(url) => format!("<{url}|{}>", thread_label(alias)),
                None => {
                    self.warn(format!(
                        "thread {} has no local permalink; shown as plain text",
                        thread_label(alias)
                    ));
                    thread_label(alias)
                }
            });
        }
        if let Some(payload) = inner.strip_prefix("issue:") {
            if payload.is_empty() || !payload.bytes().all(|byte| byte.is_ascii_digit()) {
                self.warn(format!(
                    "malformed issue link token [[{inner}]] left as written"
                ));
                return Some(format!("[[{inner}]]"));
            }
            let number: u64 = payload.parse().ok()?;
            if payload != number.to_string() || number == 0 {
                return None;
            }
            return Some(
                match self.issue_template.and_then(|t| issue_url(t, number)) {
                    Some(url) => format!("<{url}|#{number}>"),
                    None => {
                        self.warn(format!(
                        "issue #{number} cannot be linked (no usable issue_url_template); shown as plain text"
                    ));
                        format!("#{number}")
                    }
                },
            );
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORKSPACE: &str = "https://acme.slack.com";
    const TEMPLATE: &str =
        "https://github.com/OWNER/REPO/blob/main/.minibeads/issues/agent-utils-{n}.md";

    fn known(alias: u64) -> Option<String> {
        (alias == 60).then(|| {
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "1700000000.000100").expect("permalink")
        })
    }

    #[test]
    fn a_thread_permalink_names_the_thread_root() {
        assert_eq!(
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "1700000000.000100"),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000000100".to_owned())
        );
        // A short fraction is normalised before the segment is formed.
        assert_eq!(
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "1700000000.1"),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000100000".to_owned())
        );
        // A trailing slash on the workspace changes nothing.
        assert_eq!(
            slack_thread_permalink("https://acme.slack.com/", "C0123ABCD", "1700000000.000100"),
            Some("https://acme.slack.com/archives/C0123ABCD/p1700000000000100".to_owned())
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
        assert_eq!(
            slack_thread_permalink("acme.slack.com", "C0123ABCD", good),
            None
        );
        assert_eq!(slack_thread_permalink(WORKSPACE, "general", good), None);
        assert_eq!(slack_thread_permalink(WORKSPACE, "C0123ABCD", "soon"), None);
        // Eleven second digits: no segment Slack resolves.
        assert_eq!(
            slack_thread_permalink(WORKSPACE, "C0123ABCD", "17000000000.000100"),
            None
        );
    }

    #[test]
    fn the_issue_template_takes_exactly_one_number() {
        assert_eq!(
            issue_url(TEMPLATE, 658),
            Some(
                "https://github.com/OWNER/REPO/blob/main/.minibeads/issues/agent-utils-658.md"
                    .to_owned()
            )
        );
        assert_eq!(
            issue_url("https://github.com/OWNER/REPO/issues/{n}", 42),
            Some("https://github.com/OWNER/REPO/issues/42".to_owned())
        );
        assert_eq!(issue_url(TEMPLATE, 0), None);
        assert!(issue_template_problem("http://example.com/{n}").is_some());
        assert!(issue_template_problem("https://example.com/issues/").is_some());
        assert!(issue_template_problem("https://example.com/{n}/{n}").is_some());
        assert!(issue_template_problem("https://example.com/{n}?x={y}").is_some());
        assert!(issue_template_problem(TEMPLATE).is_none());
    }

    #[test]
    fn a_known_thread_and_issue_render_as_links() {
        let rendered = render_reply_links(
            "See [[thread:060]] and [[issue:658]].",
            &known,
            Some(TEMPLATE),
        );
        assert_eq!(
            rendered.text,
            "See <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|060> and <https://github.com/OWNER/REPO/blob/main/.minibeads/issues/agent-utils-658.md|#658>."
        );
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    }

    #[test]
    fn an_unknown_thread_stays_plain_and_warns() {
        let rendered = render_reply_links("Back to [[thread:999]].", &known, Some(TEMPLATE));
        assert_eq!(rendered.text, "Back to 999.");
        assert_eq!(rendered.warnings.len(), 1);
        assert!(
            rendered.warnings[0].contains("999"),
            "{:?}",
            rendered.warnings
        );
        // Without a template an issue stays plain too, and warns.
        let rendered = render_reply_links("See [[issue:658]].", &known, None);
        assert_eq!(rendered.text, "See #658.");
        assert_eq!(rendered.warnings.len(), 1);
    }

    #[test]
    fn plain_numbers_and_bare_issue_references_are_left_alone() {
        let body =
            "Result 6114 in thread 060 closes #658; alias <CHAT_REPLY_060> and [[other:060]] stay.";
        let rendered = render_reply_links(body, &known, Some(TEMPLATE));
        assert_eq!(rendered.text, body);
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    }

    #[test]
    fn tokens_in_code_are_left_alone() {
        let body = "Run `[[thread:060]]` first.\n```\n[[issue:658]]\n```\nThen [[thread:060]].";
        let rendered = render_reply_links(body, &known, Some(TEMPLATE));
        assert_eq!(
            rendered.text,
            "Run `[[thread:060]]` first.\n```\n[[issue:658]]\n```\nThen <https://acme.slack.com/archives/C0123ABCD/p1700000000000100|060>."
        );
        assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    }
}
