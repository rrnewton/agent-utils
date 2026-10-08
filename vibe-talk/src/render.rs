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
//!    it, a deep heading is held at the fourth level, where the service writes bold with one
//!    asterisk, an asterisk emphasis is bold, and a link whose text is its own address is drawn
//!    with a short label made from it, `widgets#123` (`#227 github-link-abbrev`, see
//!    [`crate::link_labels`]).
//! 4. comrak writes HTML with raw HTML in the SOURCE escaped: it is shown as the characters it is,
//!    as every chat client shows it, so the only markup in the output is markup comrak made.
//! 5. ammonia keeps exactly [`TAGS`] and the attributes listed in [`SANITIZER`], and drops the
//!    rest. A link keeps its address only when that is `http` or `https`, and every link opens in
//!    a new tab with `noopener noreferrer nofollow`. A link may keep a `title`, which step 3
//!    writes only onto a link it gave a short label, as the address beneath it.
//!
//! Step 5 is the boundary. Steps 1 to 4 decide what the message LOOKS like, and none of them is
//! trusted to keep anything out: a mistake there is a rendering bug, never an injection.
//!
//! # What a body may cost
//!
//! Anyone in a channel writes the text rendered here, bots included, and it is rendered on every
//! page load, every refresh and every live event, inside the request that asked. So the work for
//! one body is bounded, three ways out of four before it starts, and a body past a bound is simply
//! drawn as plain paragraphs — the same answer as a sentence with no Markdown in it, never an error
//! and never unsanitized text. One bound for each way the work or the answer could outgrow the
//! text:
//!
//! - **Length** ([`MAX_RENDERED_BYTES`]). Within the other bounds, comrak and the sanitizer are
//!   linear in what they read, so this holds one body to a few milliseconds.
//! - **Table cells** ([`MAX_TABLE_CELLS`]). GFM pads every row of a table out to its header's
//!   width, so a wide header over many one-character rows is a few kilobytes of text and C × R
//!   cells: 2,000 characters made 840 KB of HTML, and 20,000 made 83 MB in 38 seconds. The cells
//!   are counted from the text before it is parsed ([`table_cells_bound`]); past the budget, the
//!   message renders with tables off and its pipes are shown as written, which is how Discord,
//!   Google Chat and Slack draw them anyway.
//! - **Depth** ([`MAX_DEPTH`]). comrak builds twenty thousand nested quotes from twenty thousand
//!   `>` in a millisecond, but the sanitizer's HTML parser does work for every open element at
//!   every new one, and took 2.2 seconds over thirty-two thousand. The parsed tree is measured
//!   before anything is written.
//! - **Growth** ([`MAX_GROWTH`]). What comes out may be several times what went in — a link is
//!   its address twice and a `rel` — but a body whose HTML grew far beyond that is sent as text
//!   instead, so a window of a hundred messages, and the offline copy a phone keeps of it, stays
//!   the size of its words.
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
use std::ops::Range;
use std::sync::LazyLock;

use comrak::arena_tree::NodeEdge;
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
    ///
    /// Its other half: neither service has a double-underscore bold, so there `__init__.py` is the
    /// file it names, and the strong emphasis CommonMark reads in it is put back as the
    /// underscores that were written. Discord keeps CommonMark's reading, where Discord's own
    /// client underlines it.
    #[must_use]
    pub fn asterisk_is_bold(self) -> bool {
        matches!(self, Self::GoogleChat | Self::Slack)
    }

    /// The text as Markdown, before it is parsed.
    ///
    /// A labelled link is rewritten only OUTSIDE code. Inside a code span or a code block it is
    /// text the writer meant to show as written, and comrak, not this module, says where code is:
    /// the text is parsed once as it stands to find out, and only when it holds a link to rewrite
    /// at all, which almost no message does.
    fn prepare<'t>(self, text: &'t str, options: &Options) -> Cow<'t, str> {
        match self {
            Self::Discord => Cow::Borrowed(text),
            Self::GoogleChat | Self::Slack => {
                use crate::slack::mrkdwn::angle_links_outside;
                match angle_links_outside(text, &[]) {
                    Cow::Borrowed(text) => Cow::Borrowed(text),
                    Cow::Owned(_) => angle_links_outside(text, &code_ranges(text, options)),
                }
            }
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

/// The sanitized HTML for one body, always: rendered, or, past one of the bounds in the module
/// note, the [`plain_html`] the page would draw without it.
#[must_use]
pub fn to_html(text: &str, markup: Markup) -> String {
    if text.len() > MAX_RENDERED_BYTES {
        return plain_html(text);
    }
    let options = options(table_cells_bound(text) <= MAX_TABLE_CELLS);
    let source = markup.prepare(text, &options);
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, &source, &options);
    if deeper_than(root, MAX_DEPTH) {
        return plain_html(text);
    }
    adjust(&arena, root, &source, markup);
    let mut html = String::new();
    comrak::format_html(root, &options, &mut html).expect("formatting into a String cannot fail");
    let html = SANITIZER.clean(&html).to_string();
    if html.len() > text.len().saturating_mul(MAX_GROWTH) + GROWTH_ALLOWANCE {
        return plain_html(text);
    }
    html
}

/// The longest body rendered, in bytes. Four times the longest message Discord or Google Chat
/// accepts, so every message of theirs renders, in any script; a longer one — a Slack post near
/// its own forty-thousand-character ceiling — is drawn as plain paragraphs. See the module note.
pub const MAX_RENDERED_BYTES: usize = 16 * 1024;

/// The most table cells one body may make, counted before parsing by [`table_cells_bound`]: a
/// table of twelve columns and a hundred and fifty rows, larger than anything written in a chat,
/// at about twenty kilobytes of HTML.
pub const MAX_TABLE_CELLS: usize = 2_000;

/// The deepest the parsed tree may nest, counting every node from the document down to a word:
/// a dozen lists inside one another with a link in bold at the bottom. Far below where the
/// sanitizer's cost for nesting is measurable, and far beyond what a phone could indent.
pub const MAX_DEPTH: usize = 32;

/// How many times the length of its text a body's HTML may be, beyond [`GROWTH_ALLOWANCE`].
///
/// A link written as a bare address is the largest ordinary growth: its address twice, the
/// `target` and the `rel`, six to ten times a short address on a line of its own. A list of
/// nothing but addresses still renders; what does not is HTML that grew from something that is
/// not writing, such as hundreds of empty checkboxes.
pub const MAX_GROWTH: usize = 8;

/// The HTML every body may have beyond [`MAX_GROWTH`] times its text, so a short message is never
/// held to a ratio its few characters cannot meet.
pub const GROWTH_ALLOWANCE: usize = 8 * 1024;

/// An upper bound on the table cells comrak would make of `text`, read without parsing it.
///
/// A table's width is its delimiter row's cell count, and each of those cells holds at least one
/// hyphen, so no table is wider than the line of its run with the most runs of hyphens. Its rows
/// all sit in one run of non-blank lines, because a blank line ends a table. So each run of lines
/// contributes at most its widest line times its length, and the bound is their sum.
///
/// Deliberately generous: any line counts, whatever else is on it, and only a line of nothing but
/// spaces and tabs is blank — exactly the lines comrak calls blank, or fewer. Overcounting only
/// turns tables off for a message that has none, which changes nothing; undercounting is the one
/// mistake that matters, and line endings are split as comrak splits them so a lone `\r` cannot
/// hide a table from this count.
#[must_use]
pub fn table_cells_bound(text: &str) -> usize {
    let mut total = 0_usize;
    let (mut widest, mut lines) = (0_usize, 0_usize);
    let all_lines = text
        .split('\n')
        .flat_map(|line| line.strip_suffix('\r').unwrap_or(line).split('\r'));
    for line in all_lines {
        if line.bytes().all(|byte| matches!(byte, b' ' | b'\t')) {
            total = total.saturating_add(widest.saturating_mul(lines));
            (widest, lines) = (0, 0);
            continue;
        }
        lines += 1;
        let runs = line
            .split(|character| character != '-')
            .filter(|run| !run.is_empty())
            .count();
        widest = widest.max(runs);
    }
    total.saturating_add(widest.saturating_mul(lines))
}

/// Whether the tree under `root` nests more than `limit` deep, counting `root`.
fn deeper_than<'a>(root: &'a AstNode<'a>, limit: usize) -> bool {
    let mut depth = 0_usize;
    for edge in root.traverse() {
        match edge {
            NodeEdge::Start(_) => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            NodeEdge::End(_) => depth -= 1,
        }
    }
    false
}

/// Where `text` holds code, by byte, as comrak reads it with `options`: each code span from its
/// first backtick to its last, and each code block as the whole lines it spans.
fn code_ranges(text: &str, options: &Options) -> Vec<Range<usize>> {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, text, options);
    let lines = line_starts(text);
    let line_start = |line: usize| lines.get(line.wrapping_sub(1)).copied();
    let mut ranges = Vec::new();
    for node in root.descendants() {
        let ast = node.data.borrow();
        let (start, end) = (ast.sourcepos.start, ast.sourcepos.end);
        let range = match ast.value {
            NodeValue::Code(_) => offset(&lines, start.line, start.column)
                .zip(offset(&lines, end.line, end.column))
                .map(|(from, to)| from..to + 1),
            NodeValue::CodeBlock(_) => line_start(start.line)
                .map(|from| from..line_start(end.line + 1).unwrap_or(text.len())),
            _ => None,
        };
        ranges.extend(range);
    }
    ranges
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

/// comrak's settings, with or without tables. A function rather than a static because they are a
/// handful of booleans.
fn options(tables: bool) -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    // Off only for a message whose tables would cost more than they show: see `MAX_TABLE_CELLS`.
    options.extension.table = tables;
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
/// attributes (not even `lang`), no `style`, no `id`, no event handler, no `src`.
///
/// A link's `title` is plain text the browser shows on hover, never a URL it loads, and the
/// sanitizer's serializer escapes it as it does any attribute. [`adjust`] clears every title a
/// message wrote and writes one only onto a link it gave a short label: the address the link goes
/// to, so the hover can never claim an address the link does not have. `#227 github-link-abbrev`.
static SANITIZER: LazyLock<ammonia::Builder<'static>> = LazyLock::new(|| {
    let alignment = || HashMap::from([("align", HashSet::from(["left", "center", "right"]))]);
    let mut builder = ammonia::Builder::empty();
    builder
        .tags(HashSet::from(TAGS))
        .generic_attributes(HashSet::new())
        .tag_attributes(HashMap::from([
            ("a", HashSet::from(["href", "title"])),
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
            // The other half of the same rule: `__a__` is not bold where `*a*` is, so the
            // underscores are text again, around whatever they held.
            NodeValue::Strong if markup.asterisk_is_bold() => {
                let at = offset(&lines, ast.sourcepos.start.line, ast.sourcepos.start.column);
                if at.and_then(|at| source.as_bytes().get(at)) == Some(&b'_') {
                    drop(ast);
                    let underscores = || arena.alloc(NodeValue::Text("__".into()).into());
                    node.insert_before(underscores());
                    for kid in node.children().collect::<Vec<_>>() {
                        node.insert_before(kid);
                    }
                    node.insert_before(underscores());
                    node.detach();
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
            NodeValue::Link(_) => {
                drop(ast);
                label_link(node);
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
///
/// Asked exactly as the sanitizer will ask it, so the two never disagree: of the `href` comrak
/// will write — its percent-encoding, then the two entities it uses, decoded as an HTML parser
/// decodes them — parsed by the same URL parser. An address the sanitizer would strip, such as
/// one whose host holds a `|`, is shown as text here, rather than reaching the page as a link
/// with nowhere to go.
fn web_address(url: &str) -> bool {
    let mut href = String::new();
    if comrak::html::escape_href(&mut href, url, false).is_err() {
        return false;
    }
    let href = href.replace("&#x27;", "'").replace("&amp;", "&");
    url::Url::parse(&href).is_ok_and(|parsed| matches!(parsed.scheme(), "http" | "https"))
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

/// Give a link whose visible text is its own address a short label, when
/// [`crate::link_labels`] has one for it, and the address as its `title`. `#227
/// github-link-abbrev`.
///
/// ON THE PARSED TREE, never the source, so what is a link is comrak's answer: a bare address, an
/// address in angle brackets, and the chat services' `<address|address>` once [`Markup::prepare`]
/// has made that a link. Code is never a link, so no code is ever relabelled. A link its writer
/// named keeps the name — the label replaces only text that said nothing but where the link goes —
/// and `www.example` is its own address as much as `http://www.example` is.
///
/// Every other link loses any title the message gave it, as it always has: the sanitizer keeps a
/// link's title now, and the only one it may keep is the one written here.
fn label_link<'a>(node: &'a AstNode<'a>) {
    let mut ast = node.data.borrow_mut();
    let NodeValue::Link(link) = &mut ast.value else {
        return;
    };
    link.title.clear();
    let mut shown = String::new();
    for kid in node.children() {
        match &kid.data.borrow().value {
            NodeValue::Text(text) => shown.push_str(text),
            _ => return,
        }
    }
    let address = crate::link_labels::strip_scheme(&link.url).unwrap_or(&link.url);
    if shown.is_empty() || crate::link_labels::strip_scheme(&shown).unwrap_or(&shown) != address {
        return;
    }
    let Some(label) = crate::link_labels::short_label(&link.url) else {
        return;
    };
    link.title = link.url.clone();
    drop(ast);
    // In place, so the walk in [`adjust`] that visits this text next finds it where it was.
    let kids: Vec<_> = node.children().collect();
    if let Some((first, rest)) = kids.split_first() {
        first.data.borrow_mut().value = NodeValue::Text(label.into());
        for kid in rest {
            kid.detach();
        }
    }
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
    use std::time::{Duration, Instant};

    use super::{
        body_html, plain_html, table_cells_bound, to_html, Markup, GROWTH_ALLOWANCE, MAX_DEPTH,
        MAX_GROWTH, MAX_RENDERED_BYTES, MAX_TABLE_CELLS, SANITIZER,
    };

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

    /// The table shape from the review of `#217 markdown-blocks`: a header and delimiter row of
    /// `columns` cells over `rows` one-character rows. GFM pads every row to the header's width,
    /// so before the cell budget this was `columns × rows` cells — 840 KB of HTML from 2,001
    /// characters, 83 MB from 20,001.
    fn padded_table(columns: usize, rows: usize, newline: &str) -> String {
        format!(
            "{}{newline}{}{newline}{}",
            "|a".repeat(columns),
            "|-".repeat(columns),
            format!("|b{newline}").repeat(rows)
        )
    }

    #[test]
    fn a_table_that_pads_out_to_millions_of_cells_costs_what_its_text_does() {
        let started = Instant::now();
        for (columns, rows) in [(250, 333), (500, 666), (1_500, 2_000)] {
            for newline in ["\n", "\r\n", "\r"] {
                let text = padded_table(columns, rows, newline);
                assert!(text.len() <= MAX_RENDERED_BYTES, "{} bytes", text.len());
                assert!(table_cells_bound(&text) > MAX_TABLE_CELLS);
                let html = body_html(&text, Markup::GoogleChat);
                assert!(
                    html.len() <= 2 * text.len(),
                    "{columns} columns over {rows} rows ({newline:?}): {} bytes of text became \
                     {} of HTML",
                    text.len(),
                    html.len()
                );
                assert!(!html.contains("<td"), "the padded table was still drawn");
            }
        }
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(5),
            "nine padded tables, the largest 14 KB, took {took:?}"
        );
    }

    #[test]
    fn a_table_within_the_budget_is_a_table_and_the_count_only_overcounts() {
        // Twelve columns by a hundred and fifty rows: the size the budget is set at.
        let row = |cell: &str| format!("|{}\n", format!("{cell}|").repeat(12));
        let text = format!("{}{}{}", row("h"), row("-"), row("1").repeat(150));
        assert!(table_cells_bound(&text) <= MAX_TABLE_CELLS);
        let html = to_html(&text, Markup::Discord);
        assert_eq!(html.matches("<td>").count(), 12 * 150);
        assert_eq!(html.matches("<th>").count(), 12);
        // One column more and it is over, and drawn as the text it is.
        let wide = text.replace("|h|", "|h|h|").replace("|-|", "|-|-|");
        assert!(table_cells_bound(&wide) > MAX_TABLE_CELLS);
        assert!(!to_html(&wide, Markup::Discord).contains("<table>"));

        // Each run of lines counts on its own, and a blank line ends one.
        assert_eq!(table_cells_bound("|a|b|\n|-|-|\n|1|2|"), 6);
        assert_eq!(table_cells_bound("|a|b|\n|-|-|\n \t\n|a|\n|-|"), 2 * 2 + 2);
        // A lone carriage return ends a line for comrak, so it does here, and `\r\n` is one line
        // ending rather than two with a blank line between them.
        assert_eq!(table_cells_bound("|a|b|\r|-|-|\r|1|2|"), 6);
        assert_eq!(table_cells_bound("|a|b|\r\n|-|-|\r\n|1|2|"), 6);
        // Prose with a hyphen in it counts too, which costs nothing: it has no table to turn off.
        assert_eq!(table_cells_bound("well-known\nup-to-date"), 2 * 2);
    }

    #[test]
    fn nesting_deeper_than_anything_written_is_drawn_as_text() {
        let started = Instant::now();
        for text in [
            ">".repeat(16_000),
            "> ".repeat(8_000),
            "- ".repeat(8_000) + "a",
            (0..100)
                .map(|depth| format!("{}- a\n", "  ".repeat(depth)))
                .collect(),
        ] {
            assert_eq!(body_html(&text, Markup::Discord), "", "{:?}…", &text[..20]);
        }
        let took = started.elapsed();
        assert!(
            took < Duration::from_secs(5),
            "four deeply nested bodies took {took:?}"
        );
        // A dozen levels of list, with emphasis and a link at the bottom, is well inside.
        let deep: String = (0..12)
            .map(|depth| format!("{}- level {depth}\n", "  ".repeat(depth)))
            .collect::<String>()
            + &"  ".repeat(12)
            + "- **see [it](https://example.com)**";
        let html = to_html(&deep, Markup::Discord);
        assert_eq!(html.matches("<ul>").count(), 13, "{html}");
        assert!(html.contains("<strong>see <a href=\"https://example.com\""));
        // ...and the limit is the one stated: the document, its quotes, a paragraph and a word.
        let at_limit = format!("{}a", "> ".repeat(MAX_DEPTH - 3));
        assert!(to_html(&at_limit, Markup::Discord).contains("<blockquote>"));
        let past = format!("{}a", "> ".repeat(MAX_DEPTH - 2));
        assert_eq!(body_html(&past, Markup::Discord), "");
    }

    #[test]
    fn a_body_past_the_length_bound_is_drawn_as_text() {
        let fits = "- an item\n".repeat(MAX_RENDERED_BYTES / 10);
        assert!(fits.len() <= MAX_RENDERED_BYTES);
        assert!(body_html(&fits, Markup::Discord).starts_with("<ul>"));
        let over = format!("{fits}- one more\n");
        assert!(over.len() > MAX_RENDERED_BYTES);
        assert_eq!(body_html(&over, Markup::Discord), "");
    }

    #[test]
    fn html_far_larger_than_its_text_is_sent_as_text() {
        // Empty checkboxes are the largest growth there is: about seventeen times their text.
        let boxes = "- [ ]\n".repeat(2_000);
        assert_eq!(body_html(&boxes, Markup::Discord), "");
        // A message of nothing but short addresses, one to a line, is the largest ordinary growth,
        // and still renders.
        let addresses = "- www.example.com\n".repeat(400);
        let html = body_html(&addresses, Markup::Discord);
        assert_eq!(html.matches("<a href=").count(), 400);
        assert!(html.len() <= addresses.len() * MAX_GROWTH + GROWTH_ALLOWANCE);
        // And a short message is never held to the ratio.
        assert!(body_html("- [ ]\n- [ ]", Markup::Discord).contains("checkbox"));
    }

    #[test]
    fn a_labelled_link_inside_code_is_left_as_written() {
        let link = "<https://example.com/d|the doc>";
        let anchor = format!("<a href=\"https://example.com/d\" {LINK}>the doc</a>");
        for markup in [Markup::GoogleChat, Markup::Slack] {
            for (text, expected) in [
                (
                    format!("`{link}` but {link}"),
                    format!(
                        "<p><code>&lt;https://example.com/d|the doc&gt;</code> but {anchor}</p>\n"
                    ),
                ),
                (
                    format!("```\n{link}\n```\n{link}"),
                    format!(
                        "<pre><code>&lt;https://example.com/d|the doc&gt;\n</code></pre>\n\
                         <p>{anchor}</p>\n"
                    ),
                ),
                (
                    format!("- `{link}`\n> `{link}` {link}"),
                    format!(
                        "<ul>\n<li><code>&lt;https://example.com/d|the doc&gt;</code></li>\n</ul>\n\
                         <blockquote>\n<p><code>&lt;https://example.com/d|the doc&gt;</code> \
                         {anchor}</p>\n</blockquote>\n"
                    ),
                ),
                (
                    format!("    {link}\n\n{link}"),
                    format!(
                        "<pre><code>&lt;https://example.com/d|the doc&gt;\n</code></pre>\n\
                         <p>{anchor}</p>\n"
                    ),
                ),
            ] {
                assert_eq!(to_html(&text, markup), expected, "{markup:?} {text:?}");
            }
        }
    }

    #[test]
    fn an_address_the_sanitizer_would_strip_is_shown_as_text() {
        // Discord's dialect reads `<https://a.test|label>` as a CommonMark autolink whose host
        // holds a `|`, which the sanitizer's URL parser refuses. It used to reach the page as a
        // link with no address; it is the text it was.
        assert_eq!(
            html("see <https://a.test|label>"),
            "<p>see https://a.test|label</p>\n"
        );
        assert_eq!(
            html("[x](<https://a b/>) [y](https://[::1/)"),
            "<p>x (https://a b/) y (https://[::1/)</p>\n"
        );
        // An address the parser takes stays a link, whatever comrak had to encode in it.
        assert_eq!(
            html("[q](<https://example.com/a b?c='d'&e>)"),
            format!("<p><a href=\"https://example.com/a%20b?c='d'&amp;e\" {LINK}>q</a></p>\n")
        );
    }

    #[test]
    fn double_underscores_are_text_where_one_asterisk_is_bold() {
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html("__init__.py and __a *b* c__ and _it_", markup),
                "<p>__init__.py and __a <strong>b</strong> c__ and <em>it</em></p>\n",
                "{markup:?}"
            );
            assert_eq!(
                to_html("**bold**", markup),
                "<p><strong>bold</strong></p>\n"
            );
        }
        assert_eq!(
            html("__init__.py"),
            "<p><strong>init</strong>.py</p>\n",
            "Discord keeps CommonMark's reading"
        );
    }

    // --- short labels for links whose text is their address: `#227 github-link-abbrev` -------

    /// A neutral repository every GitHub address below is in.
    const REPO: &str = "https://github.com/octo/widgets";

    /// The anchor a link to `href` is drawn as when it was given the short label `label`.
    fn short(href: &str, label: &str) -> String {
        format!("<a href=\"{href}\" title=\"{href}\" {LINK}>{label}</a>")
    }

    /// The anchor a link to `href` is drawn as with its text as written.
    fn anchor(href: &str, text: &str) -> String {
        format!("<a href=\"{href}\" {LINK}>{text}</a>")
    }

    #[test]
    fn a_bare_github_address_is_drawn_with_its_short_label_and_goes_where_it_went() {
        let pull = format!("{REPO}/pull/12");
        assert_eq!(
            html(&format!("landed {pull} today")),
            format!("<p>landed {} today</p>\n", short(&pull, "widgets#12"))
        );
        // Every way a link's text can be its own address: in angle brackets, `www.` without a
        // scheme, written out as a Markdown link's name, or without its scheme as one.
        for text in [
            format!("<{pull}>"),
            format!("[{pull}]({pull})"),
            format!("[github.com/octo/widgets/pull/12]({pull})"),
            format!("[HTTP://github.com/octo/widgets/pull/12]({pull})"),
        ] {
            assert_eq!(
                html(&text),
                format!("<p>{}</p>\n", short(&pull, "widgets#12")),
                "{text:?}"
            );
        }
        let www = "http://www.github.com/octo/widgets/issues/7";
        assert_eq!(
            html("www.github.com/octo/widgets/issues/7"),
            format!("<p>{}</p>\n", short(www, "widgets#7"))
        );
    }

    #[test]
    fn each_kind_of_github_page_has_its_label_in_a_rendered_body() {
        // The rules are `link_labels`'s and tested there; here, that every shape reaches the page.
        let sha = "4f21ab0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a";
        for (path, label) in [
            ("/pull/12", "widgets#12"),
            ("/issues/7", "widgets#7"),
            ("/discussions/3", "widgets#3"),
            ("/pull/12#issuecomment-1234", "widgets#12 (comment)"),
            ("/pull/12#discussion_r99", "widgets#12 (comment)"),
            ("/pull/12#pullrequestreview-5", "widgets#12 (review)"),
            ("/pull/12/files", "widgets#12 (files)"),
            ("/pull/12/commits", "widgets#12 (commits)"),
            ("/pull/12/checks", "widgets#12 (checks)"),
            (&format!("/commit/{sha}"), "widgets@4f21ab0"),
            (&format!("/pull/12/commits/{sha}"), "widgets@4f21ab0"),
            (&format!("/compare/v1.0...{sha}"), "widgets@v1.0...4f21ab0"),
            ("/releases/tag/v1.2.0", "widgets@v1.2.0"),
            ("/tree/main", "widgets@main"),
            ("/actions/runs/123456", "widgets run 123456"),
            ("/actions/runs/123456/job/789", "widgets run 123456 (job)"),
            (
                "/blob/main/src/render.rs#L10-L20",
                "widgets:render.rs#L10-L20",
            ),
        ] {
            let href = format!("{REPO}{path}");
            assert_eq!(
                html(&href),
                format!("<p>{}</p>\n", short(&href, label)),
                "{path}"
            );
        }
    }

    #[test]
    fn a_named_link_another_host_and_an_unknown_page_keep_their_text() {
        let pull = format!("{REPO}/pull/12");
        // Named by its writer: the name stays, and so does nothing else — not even a title the
        // Markdown gave it, which the sanitizer would keep now.
        assert_eq!(
            html(&format!(
                "[the fix]({pull}) and [the fix]({pull} \"hover\")"
            )),
            format!(
                "<p>{} and {}</p>\n",
                anchor(&pull, "the fix"),
                anchor(&pull, "the fix")
            )
        );
        // Text that is an address, over a DIFFERENT address, is a name too.
        let other = format!("{REPO}/pull/13");
        assert_eq!(
            html(&format!(
                "[{pull}]({other}) [{pull}](https://example.com/x)"
            )),
            format!(
                "<p>{} {}</p>\n",
                anchor(&other, &pull),
                anchor("https://example.com/x", &pull)
            )
        );
        // Formatted text is not the address alone.
        assert_eq!(
            html(&format!("[**{pull}**]({pull})")),
            format!("<p><a href=\"{pull}\" {LINK}><strong>{pull}</strong></a></p>\n")
        );
        // Another host, a repository's own page, and a page with no short form.
        for href in [
            "https://example.com/octo/widgets/pull/12".to_owned(),
            "https://gitlab.example.com/octo/widgets/-/merge_requests/4".to_owned(),
            REPO.to_owned(),
            format!("{REPO}/wiki/Home"),
            format!("{REPO}/tree/main/docs"),
            format!("{REPO}/pull/12?notification_referrer_id=abc"),
        ] {
            assert_eq!(
                html(&href),
                format!("<p>{}</p>\n", anchor(&href, &href)),
                "{href}"
            );
        }
        // An image's title goes with every other title a message writes.
        assert_eq!(
            html(&format!("![shot]({pull} \"t\")")),
            format!("<p>{}</p>\n", anchor(&pull, "shot"))
        );
    }

    #[test]
    fn an_address_in_code_is_never_a_link_and_never_relabelled() {
        let pull = format!("{REPO}/pull/12");
        assert_eq!(
            html(&format!("`{pull}` and `<{pull}>`\n```\n{pull}\n```")),
            format!(
                "<p><code>{pull}</code> and <code>&lt;{pull}&gt;</code></p>\n\
                 <pre><code>{pull}\n</code></pre>\n"
            )
        );
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html(&format!("`<{pull}|{pull}>`"), markup),
                format!("<p><code>&lt;{pull}|{pull}&gt;</code></p>\n"),
                "{markup:?}"
            );
        }
    }

    #[test]
    fn punctuation_next_to_an_address_stays_outside_its_label() {
        let pull = format!("{REPO}/pull/12");
        let link = short(&pull, "widgets#12");
        for (text, expected) in [
            (format!("({pull})"), format!("({link})")),
            (format!("see {pull}."), format!("see {link}.")),
            (format!("{pull}, {pull}!"), format!("{link}, {link}!")),
            (format!("\"{pull}\"?"), format!("\"{link}\"?")),
            (format!("({pull}).",), format!("({link}).")),
            (format!("**{pull}**"), format!("<strong>{link}</strong>")),
        ] {
            assert_eq!(html(&text), format!("<p>{expected}</p>\n"), "{text:?}");
        }
    }

    #[test]
    fn every_markup_shortens_the_forms_it_writes_an_address_in() {
        let pull = format!("{REPO}/pull/12");
        let link = format!("<p>{}</p>\n", short(&pull, "widgets#12"));
        for markup in [Markup::Discord, Markup::GoogleChat, Markup::Slack] {
            for text in [pull.clone(), format!("<{pull}>")] {
                assert_eq!(to_html(&text, markup), link, "{markup:?} {text:?}");
            }
        }
        // The chat services' `<address|address>`, and their `<address|name>`, which is a name.
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html(&format!("<{pull}|{pull}>"), markup),
                link,
                "{markup:?}"
            );
            assert_eq!(
                to_html(&format!("<{pull}|the fix>"), markup),
                format!("<p>{}</p>\n", anchor(&pull, "the fix")),
                "{markup:?}"
            );
        }
        // A Slack provider's bodies have been through its reader first: `<address>`, the address
        // as its own label, and the label Slack writes for an address typed without a scheme.
        for written in [
            format!("<{pull}>"),
            format!("<{pull}|{pull}>"),
            format!("<{pull}|github.com/octo/widgets/pull/12>"),
        ] {
            let read = crate::slack::mrkdwn::to_markdown(&written);
            assert_eq!(to_html(&read, Markup::Slack), link, "{written:?}");
        }
    }

    #[test]
    fn a_status_line_of_addresses_reads_as_short_references() {
        // The shape of the message in `#227 github-link-abbrev`, with neutral names: a count, then
        // addresses, each followed by what it was, two of them joined by "and".
        let [a, b, c, d] = [
            "https://github.com/octo/gizmo/pull/3871",
            "https://github.com/octo/gizmo/pull/3908",
            "https://github.com/octo/sprocket/pull/969",
            "https://github.com/octo/sprocket/pull/974",
        ];
        let text = format!(
            "Landed (4): {a} (record of accept/accept4), {b} (fix the poll loop), {c} and {d} \
             (bump the toolchain)"
        );
        assert_eq!(
            html(&text),
            format!(
                "<p>Landed (4): {} (record of accept/accept4), {} (fix the poll loop), {} and {} \
                 (bump the toolchain)</p>\n",
                short(a, "gizmo#3871"),
                short(b, "gizmo#3908"),
                short(c, "sprocket#969"),
                short(d, "sprocket#974")
            )
        );
    }

    #[test]
    fn a_body_of_nothing_but_github_addresses_still_renders_within_its_bounds() {
        let addresses: String = (1..=300)
            .map(|n| format!("- https://github.com/octo/widgets/pull/{n}\n"))
            .collect();
        let html = body_html(&addresses, Markup::Discord);
        assert_eq!(html.matches("title=").count(), 300, "{}", &html[..200]);
        assert!(html.contains(">widgets#300</a>"));
        assert!(html.len() <= addresses.len() * MAX_GROWTH + GROWTH_ALLOWANCE);
    }

    #[test]
    fn the_sanitizer_keeps_a_links_title_and_no_other_elements() {
        assert_eq!(
            SANITIZER
                .clean("<a href=\"https://ok.test/\" title=\"a &quot;b&quot; <c>\">x</a><p title=\"t\">y</p>")
                .to_string(),
            format!(
                "<a href=\"https://ok.test/\" title=\"a &quot;b&quot; &lt;c&gt;\" {LINK}>x</a><p>y</p>"
            )
        );
    }
}
