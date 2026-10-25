//! The files a Slack message shares, drawn as lines of its body. `#313 slack-file-posts`.
//!
//! A message that is only a file — a pasted screenshot, a log uploaded with no comment — comes from
//! `conversations.history` and `conversations.replies` with an empty `text` and a `files` array,
//! and reached the page as an empty row. Each file now adds one line to the body, after the
//! message's own words when it has any, separated from them by a blank line:
//!
//! ```text
//! 📎 [screenshot.png](<https://acme.slack.com/files/U0123/F0456/screenshot.png>) · PNG
//! ```
//!
//! # In the body, not beside it
//!
//! The line is Markdown in `content`, as a Slack link already is once [`super::mrkdwn`] has read
//! it, rather than a new field of the message. So every reader of a body has it with no change of
//! its own: the page draws it with its link (`crate::render`), the voice says the file's name and
//! kind (`crate::speakable` reads a link as its words), Copy text copies it, search and a thread's
//! name find it, a summary sees it, and the cache keeps it. Every way a Slack message arrives —
//! history, Flat, a thread, the live poller — maps it in `Raw::to_message`, which is the one place
//! the line is made.
//!
//! # What is drawn
//!
//! Only what the file object carries, and none of it is required, because a compatible bridge may
//! send no more than an id and a name:
//!
//! - **The name**: `name`, else `title`. A deleted file (`mode: "tombstone"`) with neither is
//!   "a deleted file", and any other with neither "a file".
//! - **The kind**: `pretty_type`, which is what Slack's own client shows ("PNG", "Plain Text"),
//!   else `filetype`, else `mimetype`. Left out when there is none.
//! - **The link**: `permalink`, Slack's page for the file, which opens in Slack's own client.
//!   Only a plain `http(s)` address is linked; anything else leaves the name as text. Never
//!   `url_private` or a thumbnail: those are the file itself.
//!
//! A file whose `permalink` the message's own words already hold — an older Slack wrote "uploaded
//! a file: <link>" into `text`, and a bridge may too — is not listed a second time. At most
//! [`MAX_FILE_LINES`] files are listed; the rest are counted on one more line.
//!
//! # What is never done
//!
//! Nothing here fetches a file, a thumbnail or a preview, and nothing calls Slack: the line is made
//! from the message object Slack already sent. No file's content reaches this server.
//!
//! # Untrusted
//!
//! A file's name is chosen by whoever shared it, exactly as the text is written by them. It is
//! escaped as Markdown ([`escape`]) so that it draws as the characters it is — a `*` in it is not
//! emphasis, a `<https://…|label>` in it is not a link, a `]` in it does not end the file's own
//! link — and its control characters become spaces, so it stays on its own line. A `<@U…>` in it
//! is drawn as the renderer draws one anywhere, as a mention chip; drawing it notifies nobody. The
//! renderer's sanitizer is still the boundary; this is fidelity.

use serde_json::Value;

/// What every file line starts with. A paperclip, as chat clients mark an attachment; and since a
/// line starting with it opens with neither `[` nor `(`, it can never be read as a `[tag]` or a
/// `(tag)` heading (`crate::heading`).
pub const FILE_MARK: &str = "📎";

/// Most files one message lists a line for. Slack attaches at most ten to one message; a bridge
/// sending more has the rest counted on one more line.
pub const MAX_FILE_LINES: usize = 10;

/// Most characters of a file's name drawn. A file system's own limit is 255 bytes.
const MAX_NAME_CHARS: usize = 200;

/// Most characters of a file's kind drawn.
const MAX_KIND_CHARS: usize = 40;

/// Longest `permalink` linked, in bytes. Slack's are under two hundred.
const MAX_LINK_BYTES: usize = 2048;

/// Between a file's name and its kind. Not parentheses: from a bot account, a line ending in
/// `(PNG)` could be taken for a trailing `(tag)` heading once two such lines confirmed the tag.
const KIND_SEPARATOR: &str = " · ";

/// One file of a message, as far as its object says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SharedFile {
    /// The name to draw, already cleaned: the file's own, or a placeholder of this module's.
    name: String,
    /// Its kind, cleaned, when it has one.
    kind: Option<String>,
    /// Slack's page for it, when that is a plain web address.
    link: Option<String>,
}

/// The body of a message: `text`, Slack's text already read as Markdown, then one line for each
/// file in `files`, Slack's `files` array. `text` alone when the message shares no file.
#[must_use]
pub(crate) fn body(text: String, files: Option<&Value>) -> String {
    let shared = shared(files, &text);
    if shared.is_empty() {
        return text;
    }
    let mut out = text.trim_end().to_owned();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    for (index, file) in shared.iter().take(MAX_FILE_LINES).enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&line(file));
    }
    if let Some(more) = shared.len().checked_sub(MAX_FILE_LINES).filter(|&n| n > 0) {
        out.push('\n');
        out.push_str(&format!(
            "{FILE_MARK} and {more} more {}",
            if more == 1 { "file" } else { "files" }
        ));
    }
    out
}

/// A thread title for a root that is only files: its first file's name and kind as plain text,
/// with no link. `None` when the message has words of its own, or shares no file; its body's first
/// line is the title then, as for every other root.
///
/// Plain because the title is drawn as text: a file line's Markdown would show its address.
#[must_use]
pub(super) fn plain_title(message: &Value) -> Option<String> {
    let text = message.get("text").and_then(Value::as_str).unwrap_or("");
    if !text.trim().is_empty() {
        return None;
    }
    let first = shared(message.get("files"), "").into_iter().next()?;
    let mut title = format!("{FILE_MARK} {}", first.name);
    if let Some(kind) = first.kind {
        title.push_str(KIND_SEPARATOR);
        title.push_str(&kind);
    }
    Some(title)
}

/// The files of `files` to list, in Slack's order, leaving out any whose link `text` already holds.
fn shared(files: Option<&Value>, text: &str) -> Vec<SharedFile> {
    files
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .map(read)
                .filter(|file| file.link.as_deref().is_none_or(|link| !text.contains(link)))
                .collect()
        })
        .unwrap_or_default()
}

/// What one entry of a `files` array says. An entry that is not an object is still a file, of
/// which nothing is known.
fn read(file: &Value) -> SharedFile {
    let field = |key: &str, max: usize| {
        file.get(key)
            .and_then(Value::as_str)
            .and_then(|text| clean(text, max))
    };
    let name = field("name", MAX_NAME_CHARS)
        .or_else(|| field("title", MAX_NAME_CHARS))
        .unwrap_or_else(|| {
            if file.get("mode").and_then(Value::as_str) == Some("tombstone") {
                "a deleted file".to_owned()
            } else {
                "a file".to_owned()
            }
        });
    let kind = field("pretty_type", MAX_KIND_CHARS)
        .or_else(|| field("filetype", MAX_KIND_CHARS))
        .or_else(|| field("mimetype", MAX_KIND_CHARS));
    let link = file
        .get("permalink")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|link| is_plain_web_address(link))
        .map(str::to_owned);
    SharedFile { name, kind, link }
}

/// One file's line of the body, as Markdown.
fn line(file: &SharedFile) -> String {
    let name = escape(&file.name);
    let mut line = format!("{FILE_MARK} ");
    match &file.link {
        // The address in angle brackets, so a parenthesis in it cannot end the link early.
        Some(link) => line.push_str(&format!("[{name}](<{link}>)")),
        None => line.push_str(&name),
    }
    if let Some(kind) = &file.kind {
        line.push_str(KIND_SEPARATOR);
        line.push_str(&escape(kind));
    }
    line
}

/// `text` on one line — control characters, line breaks among them, made spaces — trimmed, and cut
/// to `max` characters with an ellipsis. `None` when nothing is left.
fn clean(text: &str, max: usize) -> Option<String> {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= max {
        return Some(trimmed.to_owned());
    }
    let cut: String = trimmed.chars().take(max.saturating_sub(1)).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// Whether `link` may be a link's address as it stands: `http` or `https`, bounded, and holding no
/// character that would end an angle-bracketed address, start a chat service's `<link|label>`, or
/// change what the renderer reads.
fn is_plain_web_address(link: &str) -> bool {
    let scheme = link.get(..8).unwrap_or(link).to_ascii_lowercase();
    (scheme.starts_with("http://") || scheme.starts_with("https://"))
        && link.len() <= MAX_LINK_BYTES
        && !link.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '|' | '\\' | '`' | '"')
        })
}

/// `text` with every character that would change how the renderer draws it escaped with a
/// backslash, so it draws as written: emphasis, code, strikethrough, a link's brackets, an angle
/// bracket (an autolink, a chat service's `<link|label>`, an HTML tag), an `&` that would start an
/// entity, and the backslash itself.
///
/// An underscore INSIDE a word is left alone, because Markdown never reads one there as emphasis
/// and file names are full of them — `build_log_v2.txt`. Every other character is left as it is,
/// so a name with nothing special in it, which is nearly every name, is unchanged; that is also
/// what keeps the voice from saying a backslash and Copy text from carrying one.
fn escape(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &c) in chars.iter().enumerate() {
        let special = match c {
            '\\' | '`' | '*' | '~' | '[' | ']' | '<' | '>' => true,
            '&' => starts_entity(&chars[index + 1..]),
            '_' => {
                let before = index
                    .checked_sub(1)
                    .and_then(|before| chars.get(before))
                    .is_some_and(|c| c.is_alphanumeric());
                let after = chars.get(index + 1).is_some_and(|c| c.is_alphanumeric());
                !(before && after)
            }
            _ => false,
        };
        if special {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether the characters after an `&` would make it an entity to Markdown, `&amp;` or `&#38;`: a
/// run of letters, digits or `#`, then `;`. Generous on purpose; escaping an `&` that would not
/// have started one changes nothing that is drawn.
fn starts_entity(after: &[char]) -> bool {
    let run = after
        .iter()
        .take(33)
        .take_while(|c| c.is_ascii_alphanumeric() || **c == '#')
        .count();
    (1..=32).contains(&run) && after.get(run) == Some(&';')
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{body, plain_title, FILE_MARK, MAX_FILE_LINES};
    use crate::render::{body_html, Markup};

    const LINK: &str = "https://acme.slack.com/files/U0123/F0456/screenshot.png";

    fn image() -> serde_json::Value {
        json!({"id": "F0456", "name": "screenshot.png", "title": "Screenshot",
            "mimetype": "image/png", "filetype": "png", "pretty_type": "PNG",
            "url_private": "https://files.slack.com/files-pri/T1-F0456/screenshot.png",
            "permalink": LINK})
    }

    #[test]
    fn a_post_that_is_only_a_file_is_a_line_naming_it_linked_to_its_page() {
        let files = json!([image()]);
        let content = body(String::new(), Some(&files));
        assert_eq!(
            content,
            format!("{FILE_MARK} [screenshot.png](<{LINK}>) · PNG")
        );
        let html = body_html(&content, Markup::Slack);
        assert!(
            html.contains(&format!("href=\"{LINK}\""))
                && html.contains(">screenshot.png</a> · PNG"),
            "{html}"
        );
        assert!(
            !html.contains("files-pri"),
            "the file itself is never linked: {html}"
        );
    }

    #[test]
    fn words_and_files_both_show_the_files_after_a_blank_line() {
        let files = json!([image(), {"id": "F0789", "name": "notes.txt", "pretty_type": "Plain Text",
            "permalink": "https://acme.slack.com/files/U0123/F0789/notes.txt"}]);
        let content = body("- look at this\n".to_owned(), Some(&files));
        assert_eq!(
            content,
            format!(
                "- look at this\n\n{FILE_MARK} [screenshot.png](<{LINK}>) · PNG\n\
                 {FILE_MARK} [notes.txt](<https://acme.slack.com/files/U0123/F0789/notes.txt>) · Plain Text"
            )
        );
        // The blank line keeps the files out of a list the words end with, and a single line
        // break between them keeps each on its own line.
        let html = body_html(&content, Markup::Slack);
        assert!(html.contains("</ul>\n<p>"), "{html}");
        assert!(html.contains("PNG<br>"), "{html}");
    }

    #[test]
    fn a_message_with_no_files_is_its_text_unchanged() {
        assert_eq!(body("hello ".to_owned(), None), "hello ");
        assert_eq!(body("hello ".to_owned(), Some(&json!([]))), "hello ");
        assert_eq!(
            body("hello ".to_owned(), Some(&json!({"id": "F1"}))),
            "hello "
        );
        assert_eq!(body(String::new(), None), "");
    }

    #[test]
    fn a_bridge_that_sends_only_ids_and_names_has_what_it_sent_drawn() {
        let files = json!([{"id": "F1", "name": "build.log"}, {"id": "F2"}, "F3",
            {"id": "F4", "mode": "tombstone"}, {"id": "F5", "title": "Q3 plan", "filetype": "gdoc"},
            {"id": "F6", "name": "  ", "mimetype": "application/pdf"}]);
        assert_eq!(
            body(String::new(), Some(&files)),
            format!(
                "{FILE_MARK} build.log\n{FILE_MARK} a file\n{FILE_MARK} a file\n\
                 {FILE_MARK} a deleted file\n{FILE_MARK} Q3 plan · gdoc\n\
                 {FILE_MARK} a file · application/pdf"
            )
        );
    }

    #[test]
    fn a_link_that_is_not_a_plain_web_address_leaves_the_name_as_text() {
        for link in [
            "javascript:alert(1)",
            "https://acme.slack.com/files/a b",
            "https://acme.slack.com/files/<x>",
            "https://acme.slack.com/files/x|label",
            "ftp://acme.slack.com/x",
            "",
        ] {
            let files = json!([{"name": "x.png", "permalink": link}]);
            assert_eq!(
                body(String::new(), Some(&files)),
                format!("{FILE_MARK} x.png"),
                "{link:?} was linked"
            );
        }
        let files =
            json!([{"name": "x.png", "permalink": "HTTPS://acme.slack.com/files/x(1).png"}]);
        let content = body(String::new(), Some(&files));
        let html = body_html(&content, Markup::Slack);
        assert!(
            html.contains("href=\"HTTPS://acme.slack.com/files/x(1).png\""),
            "a parenthesis does not end the address: {html}"
        );
    }

    #[test]
    fn a_name_draws_as_the_characters_it_is() {
        let files = json!([{"name": "a*b*_c_ [1] `x` ~~y~~ &amp; <https://e.test|z> <b>\\.png",
            "permalink": LINK}]);
        let content = body(String::new(), Some(&files));
        let html = body_html(&content, Markup::Slack);
        assert!(
            html.contains(
                ">a*b*_c_ [1] `x` ~~y~~ &amp;amp; &lt;https://e.test|z&gt; &lt;b&gt;\\.png</a>"
            ),
            "{content}\n{html}"
        );
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
        assert!(!html.contains("<em>") && !html.contains("<strong>") && !html.contains("<code>"));
        // A mention is drawn as a chip, as anywhere, and stays inside the file's link.
        let files = json!([{"name": "<@U0123> notes.txt", "permalink": LINK}]);
        let html = body_html(&body(String::new(), Some(&files)), Markup::Slack);
        assert!(
            html.contains("<span class=\"mention\">@U0123</span> notes.txt</a>"),
            "{html}"
        );
        // Nearly every name has nothing to escape, and is left exactly as it is.
        let files = json!([{"name": "build_log_v2 (1) Q&A #3.txt", "permalink": LINK}]);
        assert_eq!(
            body(String::new(), Some(&files)),
            format!("{FILE_MARK} [build_log_v2 (1) Q&A #3.txt](<{LINK}>)")
        );
    }

    #[test]
    fn a_name_stays_on_one_line_and_within_bounds() {
        let long = "n".repeat(500);
        let files = json!([{"name": "two\nlines\u{7}.png", "pretty_type": "P\r\nNG"},
            {"name": long}]);
        let content = body(String::new(), Some(&files));
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2, "{content}");
        assert_eq!(lines[0], format!("{FILE_MARK} two lines .png · P  NG"));
        assert_eq!(
            lines[1].chars().count(),
            FILE_MARK.chars().count() + 1 + 200
        );
        assert!(lines[1].ends_with('…'));
    }

    #[test]
    fn past_ten_files_the_rest_are_counted() {
        let many: Vec<serde_json::Value> = (0..13)
            .map(|i| json!({"name": format!("f{i}.png")}))
            .collect();
        let content = body(String::new(), Some(&json!(many)));
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), MAX_FILE_LINES + 1);
        assert_eq!(
            lines[MAX_FILE_LINES],
            format!("{FILE_MARK} and 3 more files")
        );
        let eleven: Vec<serde_json::Value> = (0..11)
            .map(|i| json!({"name": format!("f{i}.png")}))
            .collect();
        assert!(body(String::new(), Some(&json!(eleven))).ends_with("and 1 more file"));
    }

    #[test]
    fn a_file_the_words_already_link_is_not_listed_twice() {
        let files = json!([image()]);
        let text = format!("<@U0123> uploaded a file: [screenshot.png]({LINK})");
        assert_eq!(body(text.clone(), Some(&files)), text);
    }

    #[test]
    fn the_voice_says_the_name_and_kind_and_never_the_address() {
        let content = body(String::new(), Some(&json!([image()])));
        let zone = crate::clock::zone("UTC").expect("a bundled zone");
        let said = crate::speakable::for_speech(&content, 0, &zone);
        assert!(said.contains("screenshot.png"), "{said}");
        assert!(said.contains("PNG"), "{said}");
        assert!(
            !said.contains("https") && !said.contains("slack.com"),
            "{said}"
        );
    }

    #[test]
    fn a_file_line_is_never_taken_for_a_heading() {
        let everything_counts = |_: &str| true;
        for files in [
            json!([image()]),
            json!([{"name": "(ops2) Backup check, 10:52 PM ET", "pretty_type": "PNG"}]),
            json!([{"name": "[build-bot] Backup check, 10:52 PM ET", "pretty_type": "PNG"}]),
        ] {
            let content = body(String::new(), Some(&files));
            assert_eq!(
                crate::heading::extract(&content, true, &everything_counts),
                None,
                "{content}"
            );
            assert_eq!(
                crate::heading::paren_candidate(&content, true),
                None,
                "{content}"
            );
        }
    }

    #[test]
    fn a_root_that_is_only_files_is_titled_by_its_first_file() {
        let root = json!({"ts": "1700000000.000100", "text": "", "files": [image(),
            {"name": "notes.txt"}]});
        assert_eq!(
            plain_title(&root).as_deref(),
            Some("📎 screenshot.png · PNG")
        );
        let worded = json!({"ts": "1700000000.000100", "text": "see", "files": [image()]});
        assert_eq!(plain_title(&worded), None);
        let bare = json!({"ts": "1700000000.000100", "text": ""});
        assert_eq!(plain_title(&bare), None);
    }
}
