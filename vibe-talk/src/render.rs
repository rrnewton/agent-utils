//! Message bodies rendered for the page: Markdown in, sanitized HTML out. `#217 markdown-blocks`.
//!
//! # Why on the server
//!
//! The page used to draw message text with a hundred lines of its own Markdown, one block per
//! line. A `- ` item stayed a dash with no indent, and a blank line between two paragraphs
//! vanished, so the closing lines of an agent's answer ran together where the chat service's own
//! client showed round bullets and a gap. The owner's decision was to stop hand-rolling it and
//! render here, with an off-the-shelf renderer and an off-the-shelf sanitizer, the way a forge with
//! its own server renders a README: [`comrak`] parses GitHub-flavoured Markdown and writes HTML,
//! and [`ammonia`] decides what of that HTML may exist. The page inserts the result and builds no
//! markup of its own from message text.
//!
//! comrak rather than the other two Rust CommonMark parsers: `pulldown-cmark` has no GFM bare-URL
//! autolinks, which chat messages are full of, and `markdown-rs` has them but offers no way to
//! adjust the parsed tree before it is written — which is how the provider dialects below, the
//! mention chips and the one-asterisk bold, are done without a second parser.
//!
//! # The pipeline
//!
//! 1. The provider's own link syntax becomes Markdown ([`Markup::prepare`]).
//! 2. comrak parses GFM — tables, strikethrough, task lists, bare-URL autolinks — with chat's two
//!    line rules: a single newline is a line break, and a blank line separates paragraphs.
//! 3. The parsed tree is adjusted ([`adjust`]): mentions become chips, an image becomes a link to
//!    it, a deep heading is held at the fourth level, and where the service writes bold with one
//!    asterisk, an asterisk emphasis is bold.
//! 4. comrak writes HTML with raw HTML in the SOURCE escaped: it is shown as the characters it is,
//!    as every chat client shows it, so the only markup in the output is markup comrak made.
//! 5. ammonia keeps exactly [`TAGS`] and the attributes listed in [`SANITIZER`], and drops the
//!    rest. A link keeps its address only when that is `http` or `https`, and every link opens in
//!    a new tab with `noopener noreferrer nofollow`.
//!
//! Step 5 is the boundary. Steps 1 to 4 decide what the message LOOKS like, and none of them is
//! trusted to keep anything out: a mistake there is a rendering bug, never an injection.
//!
//! # What the page is sent
//!
//! [`body_html`] answers EMPTY when the HTML says nothing the text does not — a message of plain
//! sentences, which is most chat — and the page then draws `content` as paragraphs itself, through
//! the same few lines it uses for a saved message from before this field existed. The same
//! convention as [`crate::model::Message::spoken_content`], for the same reason: a window of a
//! hundred messages should not carry a hundred copies of itself down a phone's connection to say
//! nothing new. When the field IS set, something was genuinely rendered.
//!
//! The source still travels beside it, because two things on the page need what was WRITTEN rather
//! than what is drawn: Copy text, which keeps a message's own markup (`#198 copy-message-text`),
//! and the device voice and the live relay, which say `spoken_content` and fall back to `content`
//! when the spoken form is the text itself. Everything else the page used to read the source for —
//! search, the Links view, the fold, a thread's name — reads the drawn body now.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use comrak::nodes::{AstNode, NodeValue};
use comrak::{Arena, Options};

use crate::config::Config;
use crate::model::{ChannelInfo, Message};

/// How one provider writes message text, where that differs from GitHub-flavoured Markdown.
///
/// Every provider shares Discord's mention syntax, `<@id>` and `<#id>`: the Slack reader keeps a
/// user mention in that form on purpose (see [`crate::slack::mrkdwn`]), so the chips are drawn for
/// all three. What differs is emphasis and links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Markup {
    /// Discord's Markdown, which is CommonMark's for everything rendered here: `*a*` is italic.
    #[default]
    Discord,
    /// Google Chat, read through a Discord-protocol bridge: `*a*` is BOLD, and a link may arrive
    /// as `<https://…|label>`.
    GoogleChat,
    /// Slack's `mrkdwn`: `*a*` is bold, and a link is `<https://…|label>`. A Slack provider's
    /// reader has already rewritten its links ([`crate::slack::mrkdwn::to_markdown`]); doing it
    /// again here changes nothing for those and covers a bridge that passes Slack's text through.
    Slack,
}

impl Markup {
    /// Whether a single asterisk around text means bold rather than italic.
    ///
    /// THE ONE EMPHASIS RULE, and deliberately the smallest one that is right. Google Chat and
    /// Slack both write `*bold*` and `_italic_`; CommonMark reads `*a*` as italic. Rather than a
    /// second parser for their syntax, comrak still decides what is emphasis at all — flanking,
    /// code spans, a lone asterisk in arithmetic — and only the MEANING of an asterisk emphasis
    /// changes: it is drawn as strong. `**a**` is strong in every dialect already, so text written
    /// as Markdown by an agent reads the same; `_a_` stays italic, as both services draw it.
    #[must_use]
    pub fn asterisk_is_bold(self) -> bool {
        matches!(self, Self::GoogleChat | Self::Slack)
    }

    /// The text as Markdown, before it is parsed.
    fn prepare(self, text: &str) -> Cow<'_, str> {
        match self {
            Self::Discord => Cow::Borrowed(text),
            Self::GoogleChat | Self::Slack => crate::slack::mrkdwn::angle_links(text),
        }
    }
}

/// The markup the messages of `channel` are written in, from its provider's configuration.
#[must_use]
pub fn markup_for(config: &Config, channel: &ChannelInfo) -> Markup {
    channel
        .provider
        .as_deref()
        .or_else(|| config.default_provider_key())
        .and_then(|key| config.provider(key))
        .map(crate::config::ProviderConfig::markup)
        .unwrap_or_default()
}

/// Fill [`Message::content_html`] for a batch read from `channel`.
///
/// Called by the routes whose only reader is the page, and never by a path toward a model: the
/// voice agent, the digest and the MCP tools read `content`, and HTML would be bytes in their
/// context that say nothing to them.
pub fn fill(config: &Config, channel: &ChannelInfo, messages: &mut [Message]) {
    let markup = markup_for(config, channel);
    for message in messages {
        message.content_html = body_html(&message.content, markup);
    }
}

/// [`fill`] for a timeline page: its messages, and the first message of every thread it lists.
pub fn fill_page(config: &Config, channel: &ChannelInfo, page: &mut crate::threads::TimelinePage) {
    fill(config, channel, &mut page.messages);
    for summary in page.threads.iter_mut().chain(page.thread.iter_mut()) {
        if let Some(root) = &mut summary.root {
            fill(config, channel, std::slice::from_mut(root));
        }
    }
}

/// [`fill`] for pins, whose text is the snapshot the store kept: cut to its bound and marked, in
/// which case the ellipsis the page shows after it is rendered with it.
pub fn fill_pins<'p>(
    config: &Config,
    channel: &ChannelInfo,
    pins: impl IntoIterator<Item = &'p mut crate::store::Pin>,
) {
    let markup = markup_for(config, channel);
    for pin in pins {
        let snapshot = &pin.snapshot;
        pin.content_html = if snapshot.truncated {
            body_html(&format!("{}\u{2026}", snapshot.content), markup)
        } else {
            body_html(&snapshot.content, markup)
        };
    }
}

/// The sanitized HTML for one body, or EMPTY when drawing `text` as plain paragraphs is the same
/// thing. See the module note on what the page is sent.
#[must_use]
pub fn body_html(text: &str, markup: Markup) -> String {
    let html = to_html(text, markup);
    if html == plain_html(text) {
        String::new()
    } else {
        html
    }
}

/// The sanitized HTML for one body, always.
#[must_use]
pub fn to_html(text: &str, markup: Markup) -> String {
    let source = markup.prepare(text);
    let arena = Arena::new();
    let options = options();
    let root = comrak::parse_document(&arena, &source, &options);
    adjust(&arena, root, &source, markup);
    let mut html = String::new();
    comrak::format_html(root, &options, &mut html).expect("formatting into a String cannot fail");
    SANITIZER.clean(&html).to_string()
}

/// How the page draws a body that has no HTML: each run of lines that are not blank is a
/// paragraph, and each line within it is separated by a line break. `web/voice.js` builds exactly
/// this from `content` with `textContent`, so an empty [`body_html`] means "this, character for
/// character", and the comparison in [`body_html`] is what keeps that true.
#[must_use]
pub fn plain_html(text: &str) -> String {
    let mut out = String::new();
    let mut open = false;
    for line in text.split('\n') {
        if line.trim_matches([' ', '\t']).is_empty() {
            if open {
                out.push_str("</p>\n");
                open = false;
            }
            continue;
        }
        out.push_str(if open { "<br>\n" } else { "<p>" });
        open = true;
        escape_text(&mut out, line);
    }
    if open {
        out.push_str("</p>\n");
    }
    out
}

/// Text escaped exactly as the sanitizer's serializer escapes a text node.
fn escape_text(out: &mut String, text: &str) {
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            other => out.push(other),
        }
    }
}

/// comrak's settings. A function rather than a static because they are a handful of booleans.
fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.tasklist = true;
    // A bare `https://…` or `www.…` is a link. Chat is full of pasted addresses, and a message
    // whose links have to be copied out by hand is the failure this whole module is about.
    options.extension.autolink = true;
    // `text` on one line and `---` under it is a sentence and a rule in chat, never a heading.
    // Every line is its own line here (below), so a setext heading would turn an ordinary
    // sentence into a title because somebody drew a line under it.
    options.parse.ignore_setext = true;
    // Chat's line rule: a newline is a line break, and only a blank line starts a new paragraph.
    options.render.hardbreaks = true;
    // Raw HTML in a message is shown as the text it is. Never passed through: a message is not
    // markup, and the sanitizer below would remove it anyway, along with its content's meaning.
    options.render.escape = true;
    // Classes the stylesheet uses to drop the bullet beside a checkbox.
    options.render.tasklist_classes = true;
    options
}

/// The only elements the page is ever sent.
pub const TAGS: [&str; 26] = [
    "a",
    "blockquote",
    "br",
    "code",
    "del",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "hr",
    "input",
    "li",
    "ol",
    "p",
    "pre",
    "s",
    "span",
    "strong",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
];

/// The class a mention chip carries. The page styles it, and it is the only class a `span` may
/// keep.
pub const MENTION_CLASS: &str = "mention";

/// The sanitizer, built once.
///
/// From [`ammonia::Builder::empty`], so nothing is allowed that is not named here: no generic
/// attributes (not even `title` or `lang`), no `style`, no `id`, no event handler, no `src`.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let alignment = || HashMap::from([("align", HashSet::from(["left", "center", "right"]))]);
    let mut builder = ammonia::Builder::empty();
    builder
        .tags(HashSet::from(TAGS))
        .generic_attributes(HashSet::new())
        .tag_attributes(HashMap::from([
            ("a", HashSet::from(["href"])),
            ("ol", HashSet::from(["start"])),
            ("input", HashSet::from(["checked"])),
        ]))
        .tag_attribute_values(HashMap::from([
            ("th", alignment()),
            ("td", alignment()),
            (
                "input",
                HashMap::from([("type", HashSet::from(["checkbox"]))]),
            ),
        ]))
        // Written onto EVERY link and every checkbox whatever the source said, so a link always
        // leaves the app and a task's box can never be ticked from the page. ONE attribute each:
        // the sanitizer keeps these in a hash map, so two on one element would come out in an
        // order that changes from one process to the next, and so would every body holding one.
        .set_tag_attribute_values(HashMap::from([
            ("a", HashMap::from([("target", "_blank")])),
            ("input", HashMap::from([("disabled", "")])),
        ]))
        .allowed_classes(HashMap::from([
            ("span", HashSet::from([MENTION_CLASS])),
            ("ul", HashSet::from(["contains-task-list"])),
            ("ol", HashSet::from(["contains-task-list"])),
            ("li", HashSet::from(["task-list-item"])),
            ("input", HashSet::from(["task-list-item-checkbox"])),
        ]))
        // A URL is a sink. `javascript:` and `data:` execute, and a relative address resolves
        // against this origin and can be made to look like somewhere else entirely. Such a link
        // keeps its text and loses its address, so the reader still sees what was written.
        .url_schemes(HashSet::from(["http", "https"]))
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer nofollow"))
        .strip_comments(true);
    builder
});

/// The changes made to the parsed tree before it is written. See the module note, step 3.
///
/// Two walks, because the second reads what the first made: an image is a link by the time links
/// are checked, and a block of raw HTML is text by the time mentions are looked for.
fn adjust<'a>(arena: &'a Arena<'a>, root: &'a AstNode<'a>, source: &str, markup: Markup) {
    let lines = line_starts(source);
    for node in root.descendants().collect::<Vec<_>>() {
        let mut ast = node.data.borrow_mut();
        match &mut ast.value {
            // An image would be fetched by every phone that drew the row, from wherever the
            // message pointed: a read receipt for whoever wrote it. A link to it is what Discord
            // and the chat clients show, and it costs nothing until it is tapped.
            NodeValue::Image(link) => {
                let link = std::mem::take(link);
                ast.value = NodeValue::Link(link);
            }
            // The page styles four levels. A fifth would be stripped to bare text and run into the
            // paragraph after it.
            NodeValue::Heading(heading) => heading.level = heading.level.min(4),
            NodeValue::Emph if markup.asterisk_is_bold() => {
                let at = offset(&lines, ast.sourcepos.start.line, ast.sourcepos.start.column);
                if at.and_then(|at| source.as_bytes().get(at)) == Some(&b'*') {
                    ast.value = NodeValue::Strong;
                }
            }
            // A block of raw HTML is text like any other line in chat. Escaped by comrak as one
            // run with its newlines collapsed; as a paragraph its lines stay lines.
            NodeValue::HtmlBlock(block) => {
                let literal = std::mem::take(&mut block.literal);
                ast.value = NodeValue::Paragraph;
                drop(ast);
                for (index, line) in literal.trim_end_matches('\n').split('\n').enumerate() {
                    if index > 0 {
                        node.append(arena.alloc(NodeValue::LineBreak.into()));
                    }
                    node.append(arena.alloc(NodeValue::Text(line.to_owned().into()).into()));
                }
            }
            _ => {}
        }
    }
    merge_text(root);
    for node in root.descendants().collect::<Vec<_>>() {
        let ast = node.data.borrow();
        match &ast.value {
            NodeValue::Link(link) if !web_address(&link.url) => {
                let url = link.url.clone();
                drop(ast);
                unwrap_link(arena, node, url);
            }
            NodeValue::Text(text) => {
                let pieces = mention_pieces(text);
                drop(ast);
                if pieces.iter().any(|piece| matches!(piece, Piece::Chip(_))) {
                    replace_with(arena, node, pieces);
                }
            }
            _ => {}
        }
    }
}

/// Whether a link's address is one a tap can be trusted with. The sanitizer enforces the same
/// rule; this is only where the page is told what to SHOW instead.
fn web_address(url: &str) -> bool {
    let scheme = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    scheme.starts_with("http://") || scheme.starts_with("https://")
}

/// Replace a link that may not be followed with its own text and, after it, the address it
/// named: `label (javascript:…)`.
///
/// The value of the channel view is being able to point at the real message, so a refused link is
/// not silently reduced to its label — the sanitizer would keep the label and drop the address,
/// and the reader would never learn what the message really pointed at. An address that is
/// already the whole of the visible text, as an autolinked e-mail address is, is not repeated.
fn unwrap_link<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>, url: String) {
    let shown: String = node
        .descendants()
        .filter_map(|kid| match &kid.data.borrow().value {
            NodeValue::Text(text) => Some(text.to_string()),
            _ => None,
        })
        .collect();
    for kid in node.children().collect::<Vec<_>>() {
        node.insert_before(kid);
    }
    let repeated = url == shown || url.strip_prefix("mailto:") == Some(shown.as_str());
    if !url.is_empty() && !repeated {
        node.insert_before(arena.alloc(NodeValue::Text(format!(" ({url})").into()).into()));
    }
    node.detach();
}

/// Join runs of adjacent text nodes, so a mention split across two of them is still one.
///
/// comrak ends a text node at every character that might have opened something, and `<` is one:
/// `<@123>` can arrive as `<` and `@123>`.
fn merge_text<'a>(root: &'a AstNode<'a>) {
    let nodes: Vec<&AstNode<'_>> = root.descendants().collect();
    for node in nodes {
        if node.parent().is_none() {
            continue;
        }
        while let Some(next) = node.next_sibling() {
            let tail = match &next.data.borrow().value {
                NodeValue::Text(tail) => tail.to_string(),
                _ => break,
            };
            let mut ast = node.data.borrow_mut();
            let NodeValue::Text(text) = &mut ast.value else {
                break;
            };
            text.to_mut().push_str(&tail);
            drop(ast);
            next.detach();
        }
    }
}

/// One piece of a text node: words, or a chip.
enum Piece {
    Text(String),
    Chip(String),
}

/// A text node's text, cut at every mention.
fn mention_pieces(text: &str) -> Vec<Piece> {
    let mut pieces = Vec::new();
    let mut plain = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        plain.push_str(&rest[..at]);
        let tail = &rest[at..];
        match crate::discord::markup::mention_at(tail) {
            Some((length, chip)) => {
                if !plain.is_empty() {
                    pieces.push(Piece::Text(std::mem::take(&mut plain)));
                }
                pieces.push(Piece::Chip(chip));
                rest = &tail[length..];
            }
            None => {
                plain.push('<');
                rest = &tail[1..];
            }
        }
    }
    plain.push_str(rest);
    if !plain.is_empty() {
        pieces.push(Piece::Text(plain));
    }
    pieces
}

/// Put `pieces` where `node` was.
///
/// A chip is a raw node, written verbatim: its text is the mention's id with `@` or `#`, which
/// [`crate::discord::markup::mention_at`] admits only as ASCII letters and digits, so there is
/// nothing in it to escape — and the sanitizer still reads it, keeping the `span` and its one
/// allowed class and nothing else.
fn replace_with<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>, pieces: Vec<Piece>) {
    for piece in pieces {
        let value = match piece {
            Piece::Text(text) => NodeValue::Text(text.into()),
            Piece::Chip(chip) => {
                NodeValue::Raw(format!("<span class=\"{MENTION_CLASS}\">{chip}</span>"))
            }
        };
        node.insert_before(arena.alloc(value.into()));
    }
    node.detach();
}

/// Where each line of `source` begins, by byte, for reading a node's position back.
fn line_starts(source: &str) -> Vec<usize> {
    let bytes = source.as_bytes();
    let mut starts = vec![0];
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'\n' => starts.push(at + 1),
            b'\r' if bytes.get(at + 1) != Some(&b'\n') => starts.push(at + 1),
            _ => {}
        }
        at += 1;
    }
    starts
}

/// The byte a 1-based line and byte column name, when they name one.
fn offset(lines: &[usize], line: usize, column: usize) -> Option<usize> {
    let start = *lines.get(line.checked_sub(1)?)?;
    Some(start + column.checked_sub(1)?)
}

#[cfg(test)]
mod tests {
    use super::{body_html, plain_html, to_html, Markup, SANITIZER};

    const LINK: &str = r#"target="_blank" rel="noopener noreferrer nofollow""#;

    fn html(text: &str) -> String {
        to_html(text, Markup::Discord)
    }

    #[test]
    fn the_owners_message_draws_as_paragraphs_and_one_list_with_its_continuations_inside() {
        // The shape of the message in `#217 markdown-blocks`: a paragraph, a blank line, six items
        // two of which run on to an indented second line, blank lines, and two closing
        // paragraphs. The page drew the items as literal dashes and ran the last two together.
        let message = "Here is where things stand after the run:\n\
                       \n\
                       - the build is green on both runners\n\
                       - the flaky test is quarantined\n  \
                         and has an issue filed against it\n\
                       - docs are updated\n\
                       - the release notes are drafted\n\
                       - the tag is not pushed yet\n  \
                         because it waits on your review\n\
                       - nothing else is open\n\
                       \n\
                       \n\
                       I will push the tag when you say so.\n\
                       \n\
                       Thanks!";
        assert_eq!(
            html(message),
            "<p>Here is where things stand after the run:</p>\n\
             <ul>\n\
             <li>the build is green on both runners</li>\n\
             <li>the flaky test is quarantined<br>\nand has an issue filed against it</li>\n\
             <li>docs are updated</li>\n\
             <li>the release notes are drafted</li>\n\
             <li>the tag is not pushed yet<br>\nbecause it waits on your review</li>\n\
             <li>nothing else is open</li>\n\
             </ul>\n\
             <p>I will push the tag when you say so.</p>\n\
             <p>Thanks!</p>\n"
        );
        assert_eq!(body_html(message, Markup::Discord), html(message));
    }

    #[test]
    fn a_single_newline_is_a_line_break_and_a_blank_line_a_new_paragraph() {
        assert_eq!(
            html("one\ntwo\n\nthree"),
            "<p>one<br>\ntwo</p>\n<p>three</p>\n"
        );
    }

    #[test]
    fn numbered_and_nested_lists_keep_their_numbers_and_their_depth() {
        assert_eq!(
            html("1. one\n2. two"),
            "<ol>\n<li>one</li>\n<li>two</li>\n</ol>\n"
        );
        assert_eq!(
            html("3. three\n4. four"),
            "<ol start=\"3\">\n<li>three</li>\n<li>four</li>\n</ol>\n"
        );
        assert_eq!(
            html("- a\n  - inner\n    1. deep\n- b"),
            "<ul>\n<li>a\n<ul>\n<li>inner\n<ol>\n<li>deep</li>\n</ol>\n</li>\n</ul>\n</li>\n\
             <li>b</li>\n</ul>\n"
        );
        // A list may follow its introduction with no blank line between, as chat writes it.
        assert_eq!(
            html("Two things:\n- a\n- b"),
            "<p>Two things:</p>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n"
        );
    }

    #[test]
    fn a_bare_address_is_a_link_that_leaves_the_app() {
        assert_eq!(
            html("see https://example.com/a?b=1#c, and www.example.org."),
            format!(
                "<p>see <a href=\"https://example.com/a?b=1#c\" {LINK}>https://example.com/a?b=1#c</a>, \
                 and <a href=\"http://www.example.org\" {LINK}>www.example.org</a>.</p>\n"
            )
        );
        assert_eq!(
            html("[the fix](https://example.com/pull/1) and <https://example.com/x>"),
            format!(
                "<p><a href=\"https://example.com/pull/1\" {LINK}>the fix</a> and \
                 <a href=\"https://example.com/x\" {LINK}>https://example.com/x</a></p>\n"
            )
        );
    }

    #[test]
    fn a_link_that_may_not_be_followed_shows_its_label_and_its_address_as_text() {
        assert_eq!(
            html(
                "[safe](https://example.com/x) [script](javascript:alert(1)) \
                 [data](data:text/html,x) [relative](/admin/delete)"
            ),
            format!(
                "<p><a href=\"https://example.com/x\" {LINK}>safe</a> script (javascript:alert(1)) \
                 data (data:text/html,x) relative (/admin/delete)</p>\n"
            )
        );
        // An autolinked e-mail address is its own text already.
        assert_eq!(
            html("write to ops@example.com"),
            "<p>write to ops@example.com</p>\n"
        );
    }

    #[test]
    fn an_image_is_a_link_to_it_and_is_never_fetched() {
        assert_eq!(
            html("![the graph](https://example.com/g.png)"),
            format!("<p><a href=\"https://example.com/g.png\" {LINK}>the graph</a></p>\n")
        );
        assert!(!html("![x](javascript:alert(1))").contains("<a"));
    }

    #[test]
    fn inline_styles_and_mentions_render_and_code_is_taken_verbatim() {
        assert_eq!(
            html("**bold** *it* `code` ~~gone~~ <@123> <@!7> in <#456>, not <@&9>"),
            "<p><strong>bold</strong> <em>it</em> <code>code</code> <del>gone</del> \
             <span class=\"mention\">@123</span> <span class=\"mention\">@7</span> in \
             <span class=\"mention\">#456</span>, not &lt;@&amp;9&gt;</p>\n"
        );
        assert_eq!(
            html("`<@123>`\n```\n**not bold** <b>not markup</b> <@1>\n```"),
            "<p><code>&lt;@123&gt;</code></p>\n\
             <pre><code>**not bold** &lt;b&gt;not markup&lt;/b&gt; &lt;@1&gt;\n</code></pre>\n"
        );
        assert_eq!(
            html("> quoted line\n\nplain line"),
            "<blockquote>\n<p>quoted line</p>\n</blockquote>\n<p>plain line</p>\n"
        );
    }

    #[test]
    fn an_underscore_inside_a_word_is_text() {
        // The cases `#200 reply-context` gave the page's own renderer, which CommonMark answers the
        // same way.
        for (text, shown) in [
            (
                "call parse_reply_body now",
                "<p>call parse_reply_body now</p>\n",
            ),
            (
                "set ALPHA_E2E_DIR and B_V2_9",
                "<p>set ALPHA_E2E_DIR and B_V2_9</p>\n",
            ),
            (
                "see /srv/a_tree/b_dir/c_name.rs",
                "<p>see /srv/a_tree/b_dir/c_name.rs</p>\n",
            ),
            (
                "this is _really_ needed",
                "<p>this is <em>really</em> needed</p>\n",
            ),
            ("_snake_case_ stays", "<p><em>snake_case</em> stays</p>\n"),
            ("*intra*word", "<p><em>intra</em>word</p>\n"),
        ] {
            assert_eq!(html(text), shown, "{text:?}");
        }
    }

    #[test]
    fn a_long_run_of_underscores_renders_in_time_that_grows_with_its_length() {
        // The page's own renderer once took 4.7 seconds over 100,000 characters of these.
        let started = std::time::Instant::now();
        for text in [
            "a_".repeat(50_000),
            " _a".repeat(33_334),
            "*a".repeat(50_000),
        ] {
            assert!(!html(&text).is_empty());
        }
        let took = started.elapsed();
        assert!(
            took.as_secs() < 5,
            "300,000 characters of delimiters took {took:?}"
        );
    }

    #[test]
    fn deep_headings_flatten_to_the_fourth_level_and_a_rule_under_a_line_is_a_rule() {
        assert_eq!(
            html("# One\n#### Four\n###### Six"),
            "<h1>One</h1>\n<h4>Four</h4>\n<h4>Six</h4>\n"
        );
        assert_eq!(html("text\n---\nmore"), "<p>text</p>\n<hr>\n<p>more</p>\n");
    }

    #[test]
    fn tables_and_task_lists_keep_only_their_alignment_and_their_ticks() {
        assert_eq!(
            html("| a | b |\n|:--|--:|\n| 1 | 2 |"),
            "<table>\n<thead>\n<tr>\n<th align=\"left\">a</th>\n<th align=\"right\">b</th>\n\
             </tr>\n</thead>\n<tbody>\n<tr>\n<td align=\"left\">1</td>\n\
             <td align=\"right\">2</td>\n</tr>\n</tbody>\n</table>\n"
        );
        assert_eq!(
            html("- [ ] todo\n- [x] done"),
            "<ul class=\"contains-task-list\">\n\
             <li class=\"task-list-item\"><input type=\"checkbox\" \
             class=\"task-list-item-checkbox\" disabled=\"\"> todo</li>\n\
             <li class=\"task-list-item\"><input type=\"checkbox\" \
             class=\"task-list-item-checkbox\" checked=\"\" disabled=\"\"> done</li>\n</ul>\n"
        );
    }

    #[test]
    fn raw_html_in_a_message_is_shown_as_the_text_it_is() {
        assert_eq!(
            html("<script>alert(1)</script> <img src=x onerror=alert(1)> <b>x</b>"),
            "<p>&lt;script&gt;alert(1)&lt;/script&gt; &lt;img src=x onerror=alert(1)&gt; \
             &lt;b&gt;x&lt;/b&gt;</p>\n"
        );
        assert_eq!(
            html("<div onclick=\"x\">\nhello\n</div>"),
            "<p>&lt;div onclick=\"x\"&gt;<br>\nhello<br>\n&lt;/div&gt;</p>\n"
        );
        assert_eq!(
            html("<style>p{}</style>"),
            "<p>&lt;style&gt;p{}&lt;/style&gt;</p>\n"
        );
    }

    #[test]
    fn the_sanitizer_keeps_the_allowlist_and_nothing_else() {
        // What it does to HTML no step before it should ever produce: the boundary, tested on its
        // own rather than only through comrak, which escapes all of this first.
        let cleaned = SANITIZER
            .clean(
                "<p onclick=\"x()\" style=\"color:red\" class=\"evil\" id=\"i\" title=\"t\">a</p>\
                 <script>alert(1)</script><style>p{}</style><iframe src=\"https://x\"></iframe>\
                 <img src=\"https://x/y.png\" onerror=\"x()\">\
                 <a href=\"javascript:alert(1)\">j</a><a href=\"data:text/html,x\">d</a>\
                 <a href=\"/relative\">r</a><a href=\"https://ok.test/\" onmouseover=\"x()\" \
                 target=\"_self\">ok</a><span class=\"mention evil\" onclick=\"x\">@1</span>\
                 <input type=\"text\" value=\"v\" onfocus=\"x\"><form><button>b</button></form>\
                 <h5>five</h5><u>u</u><svg><circle/></svg>",
            )
            .to_string();
        assert_eq!(
            cleaned,
            format!(
                "<p>a</p>\
                 <a {LINK}>j</a><a {LINK}>d</a><a {LINK}>r</a>\
                 <a href=\"https://ok.test/\" {LINK}>ok</a><span class=\"mention\">@1</span>\
                 <input disabled=\"\">bfiveu"
            )
        );
    }

    #[test]
    fn google_chat_and_slack_read_one_asterisk_as_bold() {
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html("*bold* _it_ **strong** ~gone~ 2 * 3 * 4", markup),
                "<p><strong>bold</strong> <em>it</em> <strong>strong</strong> <del>gone</del> \
                 2 * 3 * 4</p>\n",
                "{markup:?}"
            );
            // Read from the source at the node's own position, so it holds inside the containers
            // that shift a line's columns.
            assert_eq!(
                to_html("- *a* item\n  *b* continued\n> *c*\n> > *d*", markup),
                "<ul>\n<li><strong>a</strong> item<br>\n<strong>b</strong> continued</li>\n</ul>\n\
                 <blockquote>\n<p><strong>c</strong></p>\n<blockquote>\n<p><strong>d</strong></p>\n\
                 </blockquote>\n</blockquote>\n",
                "{markup:?}"
            );
            assert_eq!(to_html("`*code*`", markup), "<p><code>*code*</code></p>\n");
        }
        assert_eq!(
            html("*it*"),
            "<p><em>it</em></p>\n",
            "Discord's asterisk is italic"
        );
    }

    #[test]
    fn a_labelled_angle_link_is_a_link_on_the_services_that_write_one() {
        let expected = format!("<p>see <a href=\"https://example.com/d\" {LINK}>the doc</a></p>\n");
        assert_eq!(
            to_html("see <https://example.com/d|the doc>", Markup::GoogleChat),
            expected
        );
        assert_eq!(
            to_html("see <https://example.com/d|the doc>", Markup::Slack),
            expected
        );
        // ...and Slack's own reader, which every native Slack body has been through already.
        let read = crate::slack::mrkdwn::to_markdown("see <https://example.com/d|the doc>");
        assert_eq!(to_html(&read, Markup::Slack), expected);
        assert_eq!(to_html(&read, Markup::Discord), expected);
    }

    #[test]
    fn plain_sentences_send_no_html_and_anything_rendered_does() {
        for text in [
            "",
            "   ",
            "Sounds good, I'll take a look",
            "line one\nline two\n\nsecond paragraph",
            "5 > 3 & \"so\" it's\u{a0}fine",
            "<script>alert(1)</script>",
        ] {
            assert_eq!(body_html(text, Markup::Discord), "", "{text:?} sent HTML");
            assert_eq!(to_html(text, Markup::Discord), plain_html(text), "{text:?}");
        }
        for text in [
            "- item",
            "**b**",
            "https://example.com",
            "<@1>",
            "a  \nb",
            "  indented",
        ] {
            assert_ne!(
                body_html(text, Markup::Discord),
                "",
                "{text:?} sent no HTML"
            );
        }
        // The same text can be plain in one dialect and not in another.
        assert_eq!(body_html("*a*", Markup::Discord), "<p><em>a</em></p>\n");
        assert_eq!(
            body_html("*a*", Markup::Slack),
            "<p><strong>a</strong></p>\n"
        );
    }

    #[test]
    fn the_plain_form_is_paragraphs_of_lines() {
        assert_eq!(
            plain_html("a\nb\n \t\nc &<>"),
            "<p>a<br>\nb</p>\n<p>c &amp;&lt;&gt;</p>\n"
        );
        assert_eq!(plain_html("\n\n"), "");
    }
}
