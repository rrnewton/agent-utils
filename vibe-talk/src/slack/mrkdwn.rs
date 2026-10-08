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
//! | `<#C123|general>` / `<#C123>` | `#general` / `#C123` |
//! | `<!here>` `<!channel>` `<!everyone>` | `@here` `@channel` `@everyone` |
//! | `<!subteam^S1|@ops>` | `@ops` |
//! | `<!date^…|fallback>` | `fallback` |
//! | `<@U123>` | `<@U123>`, unchanged |
//! | `&amp;` `&lt;` `&gt;` | `&` `<` `>` |
//!
//! User mentions are kept verbatim because vibe-talk's mention syntax is the same `<@id>`: a reply
//! that quotes one notifies the same person in Slack, and the speech preparer already reads it as
//! "a mention". Emphasis markers are left alone here, because the two dialects use the same
//! characters and the speech preparer strips them either way; what they MEAN differs — `*a*` is
//! bold in Slack and italic in Markdown — and that is settled where bodies are drawn, by
//! [`crate::render::Markup::asterisk_is_bold`].

use std::borrow::Cow;

/// Rewrite Slack `mrkdwn` into the Discord-markdown forms the rest of the server reads.
#[must_use]
pub fn to_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..open]));
        let after = &rest[open + 1..];
        match after.find(['>', '<']) {
            Some(close) if after.as_bytes()[close] == b'>' => {
                out.push_str(&token(&after[..close]));
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
    out
}

/// One `<…>` token, without its brackets.
fn token(inner: &str) -> String {
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
        return format!("#{}", label.unwrap_or_else(|| channel.to_owned()));
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
/// a bracket in the label is escaped so that it cannot either.
#[must_use]
pub fn angle_links(text: &str) -> Cow<'_, str> {
    let mut out = String::new();
    let mut copied = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find('<') {
        let at = from + found;
        from = at + 1;
        let Some((length, href, label)) = angle_link_at(&text[at..]) else {
            continue;
        };
        out.push_str(&text[copied..at]);
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
        copied = at + length;
        from = copied;
    }
    if copied == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[copied..]);
    Cow::Owned(out)
}

/// `<https://x|label>` at the start of `text`, with a label that is not empty and not the address
/// itself, as its length, the address and the label. A bare `<https://x>` is already a CommonMark
/// autolink and is left alone.
fn angle_link_at(text: &str) -> Option<(usize, &str, &str)> {
    let inner = &text[1..text.find(['>', '\n'])?];
    if text.as_bytes().get(1 + inner.len()) != Some(&b'>') || inner.contains('<') {
        return None;
    }
    let (href, label) = inner.split_once('|')?;
    let scheme = href.get(..8).unwrap_or(href).to_ascii_lowercase();
    if !(scheme.starts_with("http://") || scheme.starts_with("https://"))
        || href.chars().any(char::is_whitespace)
    {
        return None;
    }
    let label = label.trim();
    (!label.is_empty() && label != href).then_some((inner.len() + 2, href, label))
}

#[cfg(test)]
mod tests {
    use super::angle_links;

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
            "<https://example.com|https://example.com>",
            "<javascript:alert(1)|click>",
            "<https://a b|label>",
            "<https://a|label",
            "<https://a|la\nbel>",
            "a < b > c",
        ] {
            assert_eq!(angle_links(text), text, "{text:?} was rewritten");
        }
    }
}
