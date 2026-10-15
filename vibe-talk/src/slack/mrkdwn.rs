//! Slack `mrkdwn` in both directions: read for a Discord-markdown reader, and written safely.
//!
//! The page, the speech preparer in [`crate::speakable`] and the voice agent all read message
//! bodies as Discord markdown. Slack's text differs in its angle-bracket tokens and its HTML
//! entities, so reads are rewritten into the forms those readers already understand:
//!
//! | Slack | vibe-talk |
//! |---|---|
//! | `<https://x|label>` | `[label](https://x)` |
//! | `<https://x>` | `https://x` |
//! | `<#C123|general>` / `<#C123>` | `#general` / `#general` when its name could be read, else `#C123` |
//! | `<!here>` `<!channel>` `<!everyone>` | `@here` `@channel` `@everyone` |
//! | `<!subteam^S1|@ops>` | `@ops` |
//! | `<!date^…|fallback>` | `fallback` |
//! | `<@U123>` | `<@U123>`, unchanged |
//! | `&amp;` `&lt;` `&gt;` | `&` `<` `>` |
//! | `&gt; question` then `answer` | `> question`, a blank line, then `answer` |
//! | `&gt;&gt;&gt; a` then `b` | `> a` then `> b` |
//! | `:crab:` `:wave::skin-tone-3:` | `🦀` `👋🏼` |
//! | `:party-parrot:`, a custom emoji | `:party-parrot:`, unchanged |
//!
//! User mentions are kept verbatim because vibe-talk's mention syntax is the same `<@id>`: a reply
//! that quotes one notifies the same person in Slack. Who it names travels beside the body, in
//! [`crate::model::Message::mentions`], so the page draws it and speech says it as the person's
//! name (`#318 slack-mention-names`, [`super::mentions`]). A channel mention Slack sends without its
//! name, `<#C123>` or `<#C123|>`, is given the name `conversations.info` reports for it, when the
//! read could learn one. Emphasis markers are left alone here, because the two dialects use the same
//! characters and the speech preparer strips them either way; what they MEAN differs — `*a*` is
//! bold in Slack and italic in Markdown — and that is settled where bodies are drawn, by
//! [`crate::render::Markup::asterisk_is_bold`].
//!
//! A quote ends where Slack ends it ([`end_quotes`], `#322 slack-quote-lines`): in Slack a `>`
//! quotes its own line and no other, and `>>>` quotes the rest of the message. A standard emoji
//! shortcode is the emoji it names, outside code and words (`super::emoji`, `#321
//! slack-emoji-shortcodes`); a custom emoji's stays as written.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

/// Rewrite Slack `mrkdwn` into the Discord-markdown forms the rest of the server reads.
#[must_use]
pub fn to_markdown(text: &str) -> String {
    to_markdown_with(text, &HashMap::new())
}

/// [`to_markdown`], naming a channel mentioned without its name from `channels`, by id.
/// `#318 slack-mention-names`.
#[must_use]
pub fn to_markdown_with(text: &str, channels: &HashMap<String, String>) -> String {
    let text = super::emoji::shortcodes(text);
    let mut out = String::with_capacity(text.len());
    let mut rest: &str = &text;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        let after = &rest[open + 1..];
        match after.find(['>', '<']) {
            Some(close) if after.as_bytes()[close] == b'>' => {
                out.push_str(&token(&after[..close], channels));
                rest = &after[close + 1..];
            }
            _ => {
                // An unmatched bracket is text. Slack escapes a literal one, so this is rare.
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(&decode_entities(rest));
    end_quotes(out)
}

/// End every quote where Slack ends it. `#322 slack-quote-lines`.
///
/// In Slack a line that starts with `>` is quoted, and only that line: the owner's answer written
/// straight under a quoted question is drawn under the quote, not in it. CommonMark, which the page,
/// the speech preparer and the voice agent read, makes the line after a quoted one more of the
/// quote — a lazy continuation — unless a blank line comes between. So a blank line is put between
/// a quoted line and the unquoted line that follows it, and nowhere else: a quote of several lines
/// that each start with `>` stays one quote, and a quote that ends the message changes nothing.
///
/// `>>>` quotes everything after it to the end of the message, in Slack, so from that line on
/// every line is written with a `>` of its own and the quote is one quote, blank lines included.
/// CommonMark read the three as three quotes inside one another.
///
/// Inside a code block the lines are code, `>` included, and are left as they are: a block starts
/// and ends at each ```` ``` ````, wherever on a line it is written, as Slack's do. An unquoted line
/// indented four columns or more is not given a blank line either, since after one it would be
/// read as a block of code: such a line stays in the quote, as before.
fn end_quotes(markdown: String) -> String {
    if !markdown.contains('>') {
        return markdown;
    }
    let mut out = String::with_capacity(markdown.len() + 16);
    let mut in_code = false;
    let mut after_quote = false;
    let mut rest_quoted = false;
    for (index, line) in markdown.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if rest_quoted {
            out.push('>');
            if !line.is_empty() {
                out.push(' ');
                out.push_str(line);
            }
            continue;
        }
        let starts_in_code = in_code;
        if line.matches("```").count() % 2 == 1 {
            in_code = !in_code;
        }
        if starts_in_code {
            after_quote = false;
            out.push_str(line);
            continue;
        }
        if let Some(after) = line.strip_prefix(">>>") {
            rest_quoted = true;
            let after = after.strip_prefix(' ').unwrap_or(after);
            out.push('>');
            if !after.is_empty() {
                out.push(' ');
                out.push_str(after);
            }
            continue;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let quoted = indent < 4 && line[indent..].starts_with('>');
        let blank = line.trim_matches([' ', '\t']).is_empty();
        if after_quote && !quoted && !blank && indent < 4 && !line.starts_with('\t') {
            out.push('\n');
        }
        out.push_str(line);
        after_quote = quoted;
    }
    out
}

/// One `<…>` token, without its brackets.
fn token(inner: &str, channels: &HashMap<String, String>) -> String {
    let (target, label) = match inner.split_once('|') {
        Some((target, label)) => (target, Some(decode_entities(label))),
        None => (inner, None),
    };
    if let Some(user) = target.strip_prefix('@') {
        // Kept verbatim: vibe-talk mentions are the same syntax. A legacy `<@U1|name>` loses the
        // label, which Slack no longer sends and which would otherwise make the id unreadable.
        return format!("<@{user}>");
    }
    if let Some(channel) = target.strip_prefix('#') {
        // Slack may send a channel mention with an empty label, `<#C123|>`, which is no name at
        // all: the name the read looked up, else the id, as with no label.
        let name = label
            .filter(|label| !label.trim().is_empty())
            .or_else(|| channels.get(channel).cloned())
            .unwrap_or_else(|| channel.to_owned());
        return format!("#{name}");
    }
    if let Some(special) = target.strip_prefix('!') {
        return match special {
            "here" | "channel" | "everyone" => format!("@{special}"),
            _ if special.starts_with("subteam^") => match label {
                Some(label) if label.starts_with('@') => label,
                Some(label) => format!("@{label}"),
                None => format!("@{}", &special["subteam^".len()..]),
            },
            _ => label.unwrap_or_else(|| format!("@{special}")),
        };
    }
    let target = decode_entities(target);
    match label {
        Some(label) if !label.is_empty() && label != target => format!("[{label}]({target})"),
        _ => target,
    }
}

/// Decode the three entities Slack escapes, in one left-to-right pass so `&amp;lt;` reads `&lt;`.
fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (decoded, consumed) = if tail.starts_with("&amp;") {
            ('&', 5)
        } else if tail.starts_with("&lt;") {
            ('<', 4)
        } else if tail.starts_with("&gt;") {
            ('>', 4)
        } else {
            ('&', 1)
        };
        out.push(decoded);
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    out
}

/// Escape outgoing text so that only USER mentions remain live.
///
/// Slack requires `&`, `<` and `>` to be escaped in message text, and treats an unescaped `<!here>`
/// or `<!channel>` as a notification for everyone in the conversation. This is the same policy the
/// Discord client applies with `allowed_mentions`: a `<@U…>` / `<@W…>` written in the body still
/// notifies that person, and nothing else can page a room. A bare `@here` is plain text to the Web
/// API and needs no treatment.
#[must_use]
pub fn escape_outgoing(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['&', '<', '>']) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(mention) = user_mention_at(tail) {
            out.push_str(mention);
            rest = &tail[mention.len()..];
            continue;
        }
        out.push_str(match tail.as_bytes()[0] {
            b'&' => "&amp;",
            b'<' => "&lt;",
            _ => "&gt;",
        });
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `<@U…>` or `<@W…>` at the start of `text`, when it is exactly a user mention.
fn user_mention_at(text: &str) -> Option<&str> {
    let body = text.strip_prefix("<@")?;
    let end = body.find('>')?;
    let id = &body[..end];
    (id.len() >= 2
        && matches!(id.as_bytes()[0], b'U' | b'W')
        && id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit()))
    .then(|| &text[..end + 3])
}

/// Rewrite only the LINKS of this syntax into Markdown, leaving every other character as written.
/// `#217 markdown-blocks`.
///
/// For the page's renderer, which reads Google Chat's text this way too: both services send a
/// link as `<https://x|label>`, and to CommonMark that is an autolink whose address and text are
/// both `https://x|label`. Only links, and only `http` and `https` ones, because the rest of
/// [`to_markdown`] — entities, `<!here>`, channel names — is a Slack reader's job, and doing it to
/// text that has already been read would decode an entity twice. A Slack provider's bodies have
/// been through [`to_markdown`] already and hold none of these, so this changes nothing for them.
///
/// The address goes in angle brackets so that a parenthesis in it cannot end the link early, and
/// a bracket in the label is escaped so that it cannot either. A link whose label is its own
/// address, `<https://x|https://x>`, becomes the address alone in angle brackets, which is what
/// [`to_markdown`] makes of it too: to the renderer that is a link whose text is its address, so it
/// may be drawn with a short label (`#227 github-link-abbrev`).
///
/// A link that overlaps any of the byte ranges in `code` is left as written: the renderer passes
/// the code spans and blocks comrak found, where the text is shown exactly as typed. So is one
/// whose `<` is escaped with a backslash, which Markdown reads as the character itself: that is how
/// a Slack file's name keeps the characters it has (`#313 slack-file-posts`, `super::files`).
#[must_use]
pub fn angle_links_outside<'t>(text: &'t str, code: &[Range<usize>]) -> Cow<'t, str> {
    let mut out = String::new();
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find('<') {
        let at = from + found;
        // The backslashes before this `<` run back no further than the previous `<`, so counting
        // them reads each character once over the whole text.
        let escaped = text[from..at]
            .bytes()
            .rev()
            .take_while(|&byte| byte == b'\\')
            .count()
            % 2
            == 1;
        from = at + 1;
        if escaped {
            continue;
        }
        let Some((length, href, label)) = angle_link_at(&text[at..]) else {
            continue;
        };
        if code
            .iter()
            .any(|range| range.start < at + length && at < range.end)
        {
            continue;
        }
        out.push_str(&text[copied..at]);
        if label == href {
            out.push('<');
            out.push_str(href);
            out.push('>');
        } else {
            out.push('[');
            for character in label.chars() {
                if matches!(character, '[' | ']' | '\\') {
                    out.push('\\');
                }
                out.push(character);
            }
            out.push_str("](<");
            out.push_str(href);
            out.push_str(">)");
        }
        copied = at + length;
        from = copied;
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// `<https://x|label>` at the start of `text`, with a label that is not empty, as its length, the
/// address and the label. A bare `<https://x>` is already a CommonMark autolink and is left alone.
///
/// The search for the closing bracket stops at the next `<` too, because an address or a label
/// holding one is not a link. That is also what keeps a whole message linear: the next search
/// starts from that `<`, so no character is read twice, where a search to the end of the line
/// from every `<` in a line of them is quadratic.
fn angle_link_at(text: &str) -> Option<(usize, &str, &str)> {
    let close = 1 + text[1..].find(['>', '\n', '<'])?;
    if text.as_bytes()[close] != b'>' {
        return None;
    }
    let inner = &text[1..close];
    let (href, label) = inner.split_once('|')?;
    let scheme = href.get(..8).unwrap_or(href).to_ascii_lowercase();
    if !(scheme.starts_with("http://") || scheme.starts_with("https://"))
        || href.chars().any(char::is_whitespace)
    {
        return None;
    }
    let label = label.trim();
    (!label.is_empty()).then_some((inner.len() + 2, href, label))
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::collections::HashMap;

    use super::angle_links_outside;

    fn angle_links(text: &str) -> Cow<'_, str> {
        angle_links_outside(text, &[])
    }

    #[test]
    fn a_channel_mention_is_named_by_its_label_else_by_the_name_looked_up_else_by_its_id() {
        // `#318 slack-mention-names`. An empty label is no name: it used to read as a bare `#`.
        let looked_up: HashMap<String, String> =
            [("C0456EFGH".to_owned(), "ops".to_owned())].into();
        assert_eq!(
            super::to_markdown_with(
                "<#C0123ABCD|general> <#C0456EFGH> <#C0456EFGH|> <#C0789IJKL|> <#C0789IJKL>",
                &looked_up
            ),
            "#general #ops #ops #C0789IJKL #C0789IJKL"
        );
        assert_eq!(super::to_markdown("<#C0789IJKL|>"), "#C0789IJKL");
        // A user group carries its handle; a user mention is kept as it is.
        assert_eq!(
            super::to_markdown("<!subteam^S0123|@ops> <!subteam^S0456|eng> <@U0123ABCD>"),
            "@ops @eng <@U0123ABCD>"
        );
    }

    #[test]
    fn a_labelled_link_becomes_a_markdown_link_and_nothing_else_changes() {
        assert_eq!(
            angle_links("see <https://example.com/a_(b)|the *doc*> &amp; <@U1> <!here>"),
            "see [the *doc*](<https://example.com/a_(b)>) &amp; <@U1> <!here>"
        );
        assert_eq!(
            angle_links("<HTTP://x.test/|a [b] \\ c>"),
            "[a \\[b\\] \\\\ c](<HTTP://x.test/>)"
        );
    }

    #[test]
    fn what_is_not_a_labelled_web_link_is_left_as_written() {
        for text in [
            "no links",
            "<https://example.com>",
            "<https://example.com|>",
            "<javascript:alert(1)|click>",
            "<https://a b|label>",
            "<https://a|label",
            "<https://a|la\nbel>",
            "a < b > c",
        ] {
            assert_eq!(angle_links(text), text, "{text:?} was rewritten");
        }
    }

    #[test]
    fn a_link_labelled_with_its_own_address_is_the_address_alone() {
        // `#227 github-link-abbrev`: as `to_markdown` reads it, so the renderer sees a link whose
        // text is its address — and the address's own `|` can no longer hide it.
        assert_eq!(
            angle_links("see <https://example.com/a|https://example.com/a>, <https://b.test|b>"),
            "see <https://example.com/a>, [b](<https://b.test>)"
        );
        assert_eq!(
            super::to_markdown("<https://example.com/a|https://example.com/a>"),
            "https://example.com/a"
        );
        // A label that differs from the address at all, even by its scheme, is a name.
        assert_eq!(
            angle_links("<https://example.com|example.com>"),
            "[example.com](<https://example.com>)"
        );
    }

    #[test]
    fn a_link_whose_bracket_is_escaped_is_left_as_written() {
        // Markdown reads `\<` as the character: a file's name escaped to draw as it is.
        assert_eq!(
            angle_links("\\<https://a.test|b\\> then <https://c.test|d>"),
            "\\<https://a.test|b\\> then [d](<https://c.test>)"
        );
        // An escaped backslash escapes nothing after it.
        assert_eq!(
            angle_links("\\\\<https://a.test|b>"),
            "\\\\[b](<https://a.test>)"
        );
    }

    #[test]
    fn a_link_inside_code_is_left_as_written() {
        let text = "`<https://a.test|b>` then <https://c.test|d>";
        let code = 0..20;
        assert_eq!(
            angle_links_outside(text, std::slice::from_ref(&code)),
            "`<https://a.test|b>` then [d](<https://c.test>)"
        );
        // Overlapping a range at either end is inside it.
        assert_eq!(angle_links_outside(text, &[5..6, 40..41]), text);
    }

    #[test]
    fn a_line_of_angle_brackets_is_read_once() {
        // Each `<` used to search to the end of its line for a `>`, so a line of them took time
        // that grew with the square of its length: channel text, read on every page load.
        for text in ["<".repeat(64 * 1024), "<h".repeat(32 * 1024)] {
            let started = std::time::Instant::now();
            assert_eq!(angle_links(&text), text);
            let took = started.elapsed();
            assert!(
                took < std::time::Duration::from_secs(2),
                "64 KiB of angle brackets took {took:?}"
            );
        }
    }

    /// The page's rendering of a Slack body, as a Slack provider's channel is drawn.
    fn drawn(slack: &str) -> String {
        crate::render::to_html(&super::to_markdown(slack), crate::render::Markup::Slack)
    }

    #[test]
    fn a_quoted_line_ends_at_the_line_under_it() {
        // `#322 slack-quote-lines`: the owner's answer under a quoted question.
        let slack = "&gt; Should we ship it today?\nYes, after the smoke test.";
        assert_eq!(
            super::to_markdown(slack),
            "> Should we ship it today?\n\nYes, after the smoke test."
        );
        let html = drawn(slack);
        assert_eq!(
            html,
            "<blockquote>\n<p>Should we ship it today?</p>\n</blockquote>\n\
             <p>Yes, after the smoke test.</p>\n",
            "the answer was drawn inside the quote"
        );
        // Negative control: without the blank line CommonMark swallows the answer.
        let swallowed = crate::render::to_html(
            "> Should we ship it today?\nYes, after the smoke test.",
            crate::render::Markup::Slack,
        );
        assert!(
            swallowed.contains("today?<br />\nYes") || swallowed.contains("today?<br>\nYes"),
            "{swallowed}"
        );
    }

    #[test]
    fn several_quoted_lines_stay_one_quote_and_end_once() {
        let slack =
            "&gt; first line\n&gt; second line\n&gt;\n&gt; after a gap\nmy answer\nand more of it";
        assert_eq!(
            super::to_markdown(slack),
            "> first line\n> second line\n>\n> after a gap\n\nmy answer\nand more of it"
        );
        let html = drawn(slack);
        assert_eq!(html.matches("<blockquote>").count(), 1, "{html}");
        let (quote, after) = html.split_once("</blockquote>").expect("a quote");
        assert!(
            quote.contains("second line") && quote.contains("after a gap"),
            "{html}"
        );
        assert!(
            after.contains("my answer") && after.contains("and more of it"),
            "{html}"
        );
        assert!(!quote.contains("my answer"), "{html}");
    }

    #[test]
    fn a_quote_at_the_end_or_a_blank_line_after_it_changes_nothing() {
        for text in [
            "&gt; only a quote",
            "a question\n&gt; quoted at the end",
            "&gt; a quote\n\nalready apart",
            "no quote at all",
            "a &gt; b in the middle of a line\nnext",
        ] {
            assert!(
                !super::to_markdown(text).contains("\n\n\n"),
                "{text:?} grew a second blank line"
            );
            assert_eq!(
                super::to_markdown(text).matches('\n').count(),
                text.matches('\n').count(),
                "{text:?} was given a blank line"
            );
        }
        assert_eq!(super::to_markdown("&gt; only a quote"), "> only a quote");
    }

    #[test]
    fn quotes_between_answers_each_end_at_their_own_line() {
        assert_eq!(
            super::to_markdown("&gt; one?\nyes\n&gt; two?\nno"),
            "> one?\n\nyes\n> two?\n\nno"
        );
        let html = drawn("&gt; one?\nyes\n&gt; two?\nno");
        assert_eq!(html.matches("<blockquote>").count(), 2, "{html}");
        assert!(!html.contains("<p>one?<br"), "{html}");
    }

    #[test]
    fn three_brackets_quote_the_rest_of_the_message() {
        assert_eq!(
            super::to_markdown("intro\n&gt;&gt;&gt; quoted\nall of\n\nthis too"),
            "intro\n> quoted\n> all of\n>\n> this too"
        );
        let html = drawn("&gt;&gt;&gt; quoted\nall of\n\nthis too");
        assert_eq!(html.matches("<blockquote>").count(), 1, "{html}");
        assert!(html.trim_end().ends_with("</blockquote>"), "{html}");
        assert!(html.contains("this too"), "{html}");
    }

    #[test]
    fn a_quote_inside_code_is_code_and_is_left_alone() {
        for text in [
            "```\n&gt; not a quote\nstill code\n```",
            "see ```&gt; not a quote\nstill code``` done",
        ] {
            assert_eq!(
                super::to_markdown(text).matches('\n').count(),
                text.matches('\n').count(),
                "{text:?}"
            );
        }
        // After the block closes, quotes are quotes again.
        assert_eq!(
            super::to_markdown("```\ncode\n```\n&gt; q\nanswer"),
            "```\ncode\n```\n> q\n\nanswer"
        );
    }

    #[test]
    fn an_indented_line_after_a_quote_is_not_made_into_code() {
        // A blank line before it would make it an indented code block.
        assert_eq!(
            super::to_markdown("&gt; q\n    indented"),
            "> q\n    indented"
        );
        assert_eq!(super::to_markdown("&gt; q\n\tindented"), "> q\n\tindented");
        // Up to three spaces before the marker is still a quote, and still ended.
        assert_eq!(super::to_markdown("  &gt; q\nanswer"), "  > q\n\nanswer");
    }
}
