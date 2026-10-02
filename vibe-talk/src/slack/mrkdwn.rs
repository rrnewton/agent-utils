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
//! "a mention". Emphasis markers are left alone; both dialects use `*` and `_`, and the speech
//! preparer strips them either way.

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
