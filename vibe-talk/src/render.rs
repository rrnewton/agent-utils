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
//! 1. The provider's own link syntax becomes Markdown, and where the provider ends a list at an
//!    unindented line, so does the text ([`Markup::prepare`]).
//!    Under the soft single-newline rule, a line typed as a `•` bullet is made a Markdown item
//!    ([`soft_rule_lines`]), and a table whose line breaks were lost before it got here is put
//!    back on its lines ([`repair_glued_tables`]).
//! 2. comrak parses GFM — tables, strikethrough, task lists, bare-URL autolinks. A blank line
//!    separates paragraphs; a single newline is a space or a line break, as `[render]
//!    single_newline` says ([`SingleNewline`]; soft, a space, since 2026-10-26).
//! 3. The parsed tree is adjusted ([`adjust`]): a list the writer indented under an item is nested
//!    in it ([`nest_lists`], `#237 nested-lists`), mentions become chips, an image becomes a link
//!    to it, a deep heading is held at the fourth level, where the service writes bold with one
//!    asterisk, an asterisk emphasis is bold, and a link whose text is its own address is drawn
//!    with a short label made from it, `widgets#123` (`#227 github-link-abbrev`, see
//!    [`crate::link_labels`]). A Slack app link to a message stays a link, which no other
//!    `slack:` address does (`#543 in-app-message-links`).
//! 4. comrak writes HTML with raw HTML in the SOURCE escaped: it is shown as the characters it is,
//!    as every chat client shows it, so the only markup in the output is markup comrak made.
//! 5. ammonia keeps exactly [`TAGS`] and the attributes listed in [`SANITIZER`], and drops the
//!    rest. A link keeps its address only when that is `http` or `https`, and every link opens in
//!    a new tab with `noopener noreferrer nofollow`. A link may keep a `title`, which step 3
//!    writes only onto a link it gave a short label, as the address beneath it.
//! 6. A link to a chat message is marked with what it names, `data-msg`, so the page can open
//!    the message itself rather than leave for the service (`#543 in-app-message-links`, see
//!    [`crate::message_links`]). After the sanitizer, so only an address it kept is read.
//!
//! Step 5 is the boundary. Steps 1 to 4 decide what the message LOOKS like, and none of them is
//! trusted to keep anything out: a mistake there is a rendering bug, never an injection. Step 6
//! writes one attribute the sanitizer itself allows, its value percent-encoded ids, into a tag the
//! sanitizer wrote; a body marked by it is sanitized again unchanged wherever it is checked.
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
use std::hash::{BuildHasher, Hash};
use std::ops::Range;
use std::sync::{LazyLock, Mutex, PoisonError};

use comrak::arena_tree::NodeEdge;
use comrak::nodes::{AstNode, ListType, NodeList, NodeValue};
use comrak::{Arena, Options};

use crate::config::Config;
use crate::heading::HeadingTags;
use crate::model::{ChannelInfo, MentionName, Message};

pub mod highlight;

/// How one provider writes message text, where that differs from GitHub-flavoured Markdown.
///
/// Every provider shares Discord's mention syntax, `<@id>` and `<#id>`: the Slack reader keeps a
/// user mention in that form on purpose (see [`crate::slack::mrkdwn`]), so the chips are drawn for
/// all three. What differs is emphasis and links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Deserialize)]
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

    /// Whether a line written at the left margin straight after a list item ends the list.
    ///
    /// CommonMark says it does not: an unindented line of prose after an item, with no blank line
    /// between, is a LAZY continuation of the item's paragraph, so `- c\nIn review:` draws
    /// "In review:" inside the last bullet. Google Chat draws it on its own line under the list —
    /// its API guide's own example of a received message is `* 5 pm\nWhat time works for you?`,
    /// shown as a list and then a sentence — and Slack draws every line of such a message as the
    /// line it is, never indented under an item's text. Discord's client is not documented either
    /// way, so Discord keeps CommonMark's reading. `#237 nested-lists`, and see
    /// [`end_lists_at_unindented_lines`].
    ///
    /// Only a line that reads as a line of its own. A writer whose editor wraps long lines puts
    /// the rest of an item's sentence at the margin too, and that is more of the item in every
    /// client: a line that starts in lower case, one that a code span, emphasis or a link runs on
    /// to, or one after a line as long as editors wrap that stops without ending its sentence
    /// ([`continues_item`]).
    #[must_use]
    pub fn list_ends_at_unindented_line(self) -> bool {
        matches!(self, Self::GoogleChat | Self::Slack)
    }

    /// Whether bullets written at the left margin straight under a numbered line are its items.
    ///
    /// Google Chat's own syntax has bulleted lists and NO numbered ones: `1. Goal` is a line of
    /// text there, and the bullets under it are drawn as a list indented beneath it — so an agent's
    /// outline, `1. Goal\n- step\n- step\n\n2. Goal`, reads as steps under each goal. CommonMark
    /// reads the same text as a one-item numbered list, then a bulleted list level with it, which
    /// is the flat outline the owner photographed in `#237 nested-lists`. A heading line after a
    /// goal's steps with more bullets straight under it, `In review:`, is more of the same goal,
    /// so that every bullet of the outline is drawn at one indent, as Google Chat draws them
    /// ([`more_steps_after`]). Only Google Chat: Discord draws both as lists, and Slack draws both
    /// as lines, at the same margin.
    #[must_use]
    pub fn bullets_under_numbered_line_nest(self) -> bool {
        matches!(self, Self::GoogleChat)
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
                    Cow::Borrowed(_) => Cow::Borrowed(text),
                    Cow::Owned(_) => Cow::Owned(
                        angle_links_outside(text, &code_ranges(text, options)).into_owned(),
                    ),
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

/// What the message cache's full-text index holds of a body: its text as the page draws it, and
/// the addresses of its links. `#208 persistent-cache`.
///
/// The page searches what a row SAYS — the drawn text, with emphasis gone and a link drawn by its
/// short label (`widgets#123`), and the addresses its links go to — so an index built from the
/// source alone would never offer a row whose match is only in the drawn form. This draws each
/// body exactly as the page is sent it, through [`headed_body`] with the provider's own markup,
/// and reads the result the way a browser's `textContent` does: every text node, entities
/// decoded, no tag adding anything. A body the page draws as plain paragraphs is its text.
///
/// The closure that confirms a paren heading confirms none and records nothing: indexing a row
/// must not change how another one is drawn. That costs nothing here, because a heading only
/// marks words the body already holds.
///
/// [`SearchText::VERSION`] names these rules. A change to the rendering, the link labels or this
/// reading of the result is a new version, and a cache built under the old one rebuilds its index
/// text in the background. A test over a fixed corpus pins the output to the version, so a change
/// that forgets the bump fails.
#[derive(Clone, Debug, Default)]
pub struct SearchText {
    markups: HashMap<String, Markup>,
}

impl SearchText {
    /// The label of the rules this indexes by, under the soft single-newline rule; the hard rule
    /// indexes under it with `+hard` after it. `render-2`: single newlines soft, `•` lines items,
    /// glued tables repaired (`markdown-tables-render`).
    pub const VERSION: &'static str = "render-2";

    /// The rendering for every provider `config` names, each with its own markup.
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        Self {
            markups: config
                .providers
                .iter()
                .map(|provider| (provider.key.clone(), provider.markup()))
                .collect(),
        }
    }

    /// One body as the page draws it, written in `markup` by an author who is (`bot`) or is not a
    /// bot account: its visible text and its link addresses, in order.
    #[must_use]
    pub fn drawn(text: &str, markup: Markup, bot: bool) -> crate::cache::Rendered {
        Self::drawn_named(text, markup, bot, &[])
    }

    /// [`SearchText::drawn`], with each of `mentions` drawn by the name the provider gave it, as
    /// the page draws a message's [`Message::mentions`]. `#318 slack-mention-names`.
    #[must_use]
    pub fn drawn_named(
        text: &str,
        markup: Markup,
        bot: bool,
        mentions: &[MentionName],
    ) -> crate::cache::Rendered {
        let (html, _heading) =
            headed_body_by(render_named, text, markup, bot, &|_| false, mentions);
        if html.is_empty() {
            return crate::cache::Rendered {
                text: text.to_owned(),
                hrefs: Vec::new(),
            };
        }
        text_content(&html)
    }
}

impl crate::cache::SearchTextSource for SearchText {
    fn version(&self) -> String {
        match single_newline() {
            SingleNewline::Soft => Self::VERSION.to_owned(),
            SingleNewline::Hard => format!("{}+hard", Self::VERSION),
        }
    }

    fn rendered(&self, provider_key: &str, message: &Message) -> crate::cache::Rendered {
        let markup = self.markups.get(provider_key).copied().unwrap_or_default();
        Self::drawn_named(
            &message.content,
            markup,
            message.author_is_bot,
            &message.mentions,
        )
    }
}

/// The text content of sanitized HTML, as a browser reads it, and the address of every link in
/// it.
///
/// Only for what [`SANITIZER`] wrote: well-formed tags, attribute values in double quotes, and
/// text whose only markup characters are entities. A tag adds nothing to the text, because the
/// HTML puts a newline after every block element itself.
fn text_content(html: &str) -> crate::cache::Rendered {
    let mut text = String::with_capacity(html.len());
    let mut hrefs = Vec::new();
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        push_decoded(&mut text, &rest[..open]);
        let tag = &rest[open + 1..];
        let mut quote = None;
        let close = tag.char_indices().find_map(|(at, c)| {
            match (quote, c) {
                (None, '"' | '\'') => quote = Some(c),
                (Some(q), c) if c == q => quote = None,
                (None, '>') => return Some(at),
                _ => {}
            }
            None
        });
        let Some(close) = close else {
            // An unclosed tag is not something the sanitizer writes; what follows is not text.
            rest = "";
            break;
        };
        if let Some(href) = anchor_href(&tag[..close]) {
            hrefs.push(href);
        }
        rest = &tag[close + 1..];
    }
    push_decoded(&mut text, rest);
    crate::cache::Rendered { text, hrefs }
}

/// The decoded `href` of an opening `<a …>` tag, given what lies between its angle brackets.
fn anchor_href(tag: &str) -> Option<String> {
    let attributes = tag.strip_prefix('a')?;
    if !attributes.starts_with(|c: char| c.is_ascii_whitespace()) {
        return None;
    }
    let start = attributes.find(" href=\"")? + " href=\"".len();
    let value = &attributes[start..];
    let end = value.find('"')?;
    let mut href = String::with_capacity(end);
    push_decoded(&mut href, &value[..end]);
    Some(href)
}

/// Append `text` to `out` with its character references decoded. A reference this does not know
/// is kept as written.
fn push_decoded(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let decoded = after
            .find(';')
            .filter(|&semi| semi <= 12)
            .and_then(|semi| character_reference(&after[..semi]).map(|c| (c, semi)));
        match decoded {
            Some((c, semi)) => {
                out.push(c);
                rest = &after[semi + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
}

/// The character a reference names, without its `&` and `;`.
fn character_reference(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

/// Fill [`Message::content_html`] and [`Message::heading`] for a batch read from `channel`.
///
/// Called by the routes whose only reader is the page, and never by a path toward a model: the
/// voice agent, the digest and the MCP tools read `content`, and HTML would be bytes in their
/// context that say nothing to them.
///
/// Every message's paren tag is recorded in `headings` BEFORE any heading is found, so a window
/// that holds two of an author's `(tag)` messages confirms the tag for both. `#242 rich-headers`.
pub fn fill(
    config: &Config,
    headings: &HeadingTags,
    channel: &ChannelInfo,
    messages: &mut [Message],
) {
    let markup = markup_for(config, channel);
    for message in messages.iter() {
        if let Some(tag) = crate::heading::paren_candidate(&message.content, message.author_is_bot)
        {
            headings.record(&channel.id, &message.author_id, &message.id, tag);
        }
    }
    for message in messages {
        let confirmed = |tag: &str| headings.confirmed(&channel.id, &message.author_id, tag);
        (message.content_html, message.heading) = headed_body(
            &message.content,
            markup,
            message.author_is_bot,
            &confirmed,
            &message.mentions,
        );
    }
}

/// [`fill`] for a timeline page: its messages, and the first message of every thread it lists.
pub fn fill_page(
    config: &Config,
    headings: &HeadingTags,
    channel: &ChannelInfo,
    page: &mut crate::threads::TimelinePage,
) {
    fill(config, headings, channel, &mut page.messages);
    for summary in page.threads.iter_mut().chain(page.thread.iter_mut()) {
        if let Some(root) = &mut summary.root {
            fill(config, headings, channel, std::slice::from_mut(root));
        }
    }
}

/// [`fill`] for pins, whose text is the snapshot the store kept: cut to its bound and marked, in
/// which case the ellipsis the page shows after it is rendered with it.
///
/// A pin is a message the server holds for the channel, so its paren tag is recorded as a read's
/// are, before any heading is found: pins alone confirm a tag, with no read of the channel since
/// this process started. The record counts message ids, so a pin of a message a read also served
/// is still one message.
pub fn fill_pins<'p>(
    config: &Config,
    headings: &HeadingTags,
    channel: &ChannelInfo,
    pins: impl IntoIterator<Item = &'p mut crate::store::Pin>,
) {
    let markup = markup_for(config, channel);
    let pins: Vec<&mut crate::store::Pin> = pins.into_iter().collect();
    for pin in &pins {
        let snapshot = &pin.snapshot;
        if let Some(tag) =
            crate::heading::paren_candidate(&snapshot.content, snapshot.author_is_bot)
        {
            headings.record(&channel.id, &snapshot.author_id, &snapshot.message_id, tag);
        }
    }
    for pin in pins {
        let snapshot = &pin.snapshot;
        let text: Cow<'_, str> = if snapshot.truncated {
            Cow::Owned(format!("{}\u{2026}", snapshot.content))
        } else {
            Cow::Borrowed(&snapshot.content)
        };
        let confirmed = |tag: &str| headings.confirmed(&channel.id, &snapshot.author_id, tag);
        // A pin keeps the text it was pinned with and not who its mentions named: its chips are
        // drawn by id.
        (pin.content_html, pin.heading) =
            headed_body(&text, markup, snapshot.author_is_bot, &confirmed, &[]);
    }
}

/// [`body_html`] together with the heading the text opens with, when one is found and its words
/// can be marked in the body. `#242 rich-headers`. `bot` and `confirmed` are
/// [`crate::heading::extract`]'s: whether the author is a bot account, and whether a paren tag
/// counts. `mentions` names the people the text's mentions name, whose chips are drawn with
/// their names ([`to_html_named`]).
///
/// THE BODY IS NOT CUT. The words the heading restates, `text[..cut]`, stay in it, so a page that
/// does not show the heading draws exactly what it drew before. They are marked instead, so a page
/// that does show it can hide them:
///
/// - **A rendered body** has them wrapped in `<span class="rh-src">` — with the space after them,
///   and with the line break after them when the heading uses the whole first line — or, when the
///   first line is the whole first paragraph, carries the class on that `<p>`. Only when the body
///   begins with a paragraph that begins with exactly what those words render to on their own; any
///   other shape, and the message has no heading ([`mark_restated`]).
/// - **A plain body** stays empty, as every plain body is, and the page marks `content[..cut]`
///   itself as it draws the paragraphs. The same shape, from the same rule, so a plain body is not
///   sent as HTML only to carry a class.
#[must_use]
pub fn headed_body(
    text: &str,
    markup: Markup,
    bot: bool,
    confirmed: &dyn Fn(&str) -> bool,
    mentions: &[MentionName],
) -> (String, Option<crate::model::Heading>) {
    headed_body_by(to_html_named, text, markup, bot, confirmed, mentions)
}

/// How a body is rendered: [`to_html_named`], through the cache, or [`render_named`], past it.
type Render = fn(&str, Markup, &[MentionName]) -> String;

/// [`headed_body`], rendering with `render`.
fn headed_body_by(
    render: Render,
    text: &str,
    markup: Markup,
    bot: bool,
    confirmed: &dyn Fn(&str) -> bool,
    mentions: &[MentionName],
) -> (String, Option<crate::model::Heading>) {
    let html = render(text, markup, mentions);
    let plain = html == plain_html(text);
    let Some(found) = crate::heading::extract(text, bot, confirmed) else {
        return (if plain { String::new() } else { html }, None);
    };
    let heading = found.wire(text);
    if plain {
        return (String::new(), Some(heading));
    }
    match mark_restated(render, &html, &text[..found.cut], markup, mentions) {
        Some(marked) => (marked, Some(heading)),
        None => (html, None),
    }
}

/// The class on the words a heading restates. The page hides them while it shows the heading.
pub const RESTATED_CLASS: &str = "rh-src";

/// `html`, the rendering of a whole body, with the part that renders `prefix` marked as restated;
/// `None` when the body does not begin that way.
///
/// The prefix is rendered on its own, and must be, character for character, the start of the body's
/// first paragraph: that is the proof that the marked part draws exactly those words and nothing
/// else. What follows it must be the rest of the line (a space), the line's end (a line break), or
/// the paragraph's end. The marked string is then put through the sanitizer again and must come out
/// unchanged, so nothing reaches the page that the sanitizer did not write.
fn mark_restated(
    render: Render,
    html: &str,
    prefix: &str,
    markup: Markup,
    mentions: &[MentionName],
) -> Option<String> {
    let alone = render(prefix, markup, mentions);
    let words = alone.strip_prefix("<p>")?.strip_suffix("</p>\n")?;
    if words.contains("<p>") || words.contains("</p>") {
        return None;
    }
    let rest = html.strip_prefix("<p>")?.strip_prefix(words)?;
    let open = format!("<span class=\"{RESTATED_CLASS}\">");
    let marked = if rest.starts_with("</p>") {
        format!("<p class=\"{RESTATED_CLASS}\">{words}{rest}")
    } else {
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        let (gap, after) = rest.split_at(spaces);
        if let Some(after) = after.strip_prefix("<br>\n") {
            format!("<p>{open}{words}{gap}<br>\n</span>{after}")
        } else if let Some(after) = after.strip_prefix('\n') {
            // A soft line break ([`SingleNewline::Soft`]): the heading's line still ends where it
            // was written, as it did under the hard rule, so the words after it do not run on
            // from the agent's title when the heading is not shown.
            format!("<p>{open}{words}{gap}<br>\n</span>{after}")
        } else if spaces > 0 {
            format!("<p>{open}{words}{gap}</span>{after}")
        } else {
            return None;
        }
    };
    (SANITIZER.clean(&marked).to_string() == marked).then_some(marked)
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
    to_html_named(text, markup, &[])
}

/// [`to_html`], drawing each user mention that `mentions` names as the person's name rather than
/// their id: `<@U0123ABCD>` is the chip `@Ada Lovelace`. `#318 slack-mention-names`. A mention it
/// does not name is drawn by id, as every mention was before.
///
/// Answered from [`RENDERED`] when the same body was rendered recently.
#[must_use]
pub fn to_html_named(text: &str, markup: Markup, mentions: &[MentionName]) -> String {
    let rule = newline_rule(text, markup);
    if let Some(html) = RENDERED.get(text, markup, rule, mentions) {
        return html;
    }
    let html = render_named(text, markup, mentions);
    RENDERED.put(text, markup, rule, mentions, &html);
    html
}

/// [`to_html_named`] without the cache: neither read nor written. For the message cache's
/// full-text index, which renders every message it holds when it rebuilds, and would otherwise
/// push the bodies the timeline is reading out of [`RENDERED`] with old ones.
fn render_named(text: &str, markup: Markup, mentions: &[MentionName]) -> String {
    render_visiting(text, markup, mentions, |_, _| ()).0
}

/// The bytes one generation of [`RENDERED`] holds before it becomes the old one: its bodies' text
/// and HTML and an allowance for each entry (`RenderedBody::bytes`).
pub const RENDERED_GENERATION_BYTES: usize = 8 * 1024 * 1024;

/// Bodies rendered recently, so a page load, a refresh and a live event that send the same
/// messages render each of them once. `#531 server-side-syntax-highlight`: highlighting a long
/// code block costs milliseconds, and the timeline sends the same window again on every refresh.
///
/// What a body renders to depends only on its text, its markup, the single-newline rule it is drawn
/// under and the names of its mentions, so those are the key, compared in full: a hash only finds
/// the candidates. Two generations, each
/// bounded by [`RENDERED_GENERATION_BYTES`]: a body is put in the young one, and when that is full
/// it becomes the old one and the oldest are dropped. A body found in the old generation moves to
/// the young one, so what is still being read stays.
static RENDERED: LazyLock<RenderCache> = LazyLock::new(RenderCache::default);

#[derive(Default)]
struct RenderCache {
    hasher: std::collections::hash_map::RandomState,
    generations: Mutex<Generations>,
}

#[derive(Default)]
struct Generations {
    young: HashMap<u64, Vec<RenderedBody>>,
    young_bytes: usize,
    old: HashMap<u64, Vec<RenderedBody>>,
}

struct RenderedBody {
    markup: Markup,
    rule: SingleNewline,
    text: String,
    mentions: Vec<MentionName>,
    html: String,
}

impl RenderedBody {
    fn is(
        &self,
        text: &str,
        markup: Markup,
        rule: SingleNewline,
        mentions: &[MentionName],
    ) -> bool {
        self.markup == markup && self.rule == rule && self.text == text && self.mentions == mentions
    }

    /// What the entry holds, counted generously: its strings, its mentions, and a fixed allowance
    /// for the allocations, headers and map slot around them, so a generation of very short bodies
    /// is bounded by memory too and not only by their text.
    fn bytes(&self) -> usize {
        const OVERHEAD: usize = 256;
        let mentions: usize = self
            .mentions
            .iter()
            .map(|mention| mention.id.len() + mention.name.len() + 64)
            .sum();
        OVERHEAD + self.text.len() + self.html.len() + mentions
    }
}

impl RenderCache {
    fn key(
        &self,
        text: &str,
        markup: Markup,
        rule: SingleNewline,
        mentions: &[MentionName],
    ) -> u64 {
        self.hasher.hash_one((
            markup,
            rule,
            text,
            mentions
                .iter()
                .map(|mention| (&mention.id, &mention.name))
                .collect::<Vec<_>>(),
        ))
    }

    fn get(
        &self,
        text: &str,
        markup: Markup,
        rule: SingleNewline,
        mentions: &[MentionName],
    ) -> Option<String> {
        let key = self.key(text, markup, rule, mentions);
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = generations.young.get(&key).and_then(|bodies| {
            bodies
                .iter()
                .find(|body| body.is(text, markup, rule, mentions))
        }) {
            return Some(found.html.clone());
        }
        let bodies = generations.old.get_mut(&key)?;
        let at = bodies
            .iter()
            .position(|body| body.is(text, markup, rule, mentions))?;
        let body = bodies.swap_remove(at);
        let html = body.html.clone();
        generations.insert(key, body);
        Some(html)
    }

    fn put(
        &self,
        text: &str,
        markup: Markup,
        rule: SingleNewline,
        mentions: &[MentionName],
        html: &str,
    ) {
        let key = self.key(text, markup, rule, mentions);
        let body = RenderedBody {
            markup,
            rule,
            text: text.to_owned(),
            mentions: mentions.to_vec(),
            html: html.to_owned(),
        };
        self.generations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, body);
    }
}

impl Generations {
    fn insert(&mut self, key: u64, body: RenderedBody) {
        let bodies = self.young.entry(key).or_default();
        if bodies
            .iter()
            .any(|have| have.is(&body.text, body.markup, body.rule, &body.mentions))
        {
            return;
        }
        self.young_bytes += body.bytes();
        bodies.push(body);
        if self.young_bytes > RENDERED_GENERATION_BYTES {
            self.old = std::mem::take(&mut self.young);
            self.young_bytes = 0;
        }
    }
}

/// [`to_html`] under the single-newline rule `rule`, whatever this process was configured with.
#[must_use]
pub fn to_html_as(text: &str, markup: Markup, rule: SingleNewline) -> String {
    render_visiting_as(text, markup, &[], rule, |_, _| ()).0
}

/// [`to_html_named`], handing the parsed and adjusted tree, and the text it was parsed from, to
/// `visit` before it is written. `visit` runs only when the body is rendered: not when a bound
/// draws it as [`plain_html`] instead, and then its answer is `None`.
fn render_visiting<T>(
    text: &str,
    markup: Markup,
    mentions: &[MentionName],
    visit: impl for<'a> FnOnce(&'a AstNode<'a>, &str) -> T,
) -> (String, Option<T>) {
    render_visiting_as(text, markup, mentions, newline_rule(text, markup), visit)
}

/// [`render_visiting`] under the single-newline rule `rule`.
fn render_visiting_as<T>(
    text: &str,
    markup: Markup,
    mentions: &[MentionName],
    rule: SingleNewline,
    visit: impl for<'a> FnOnce(&'a AstNode<'a>, &str) -> T,
) -> (String, Option<T>) {
    if text.len() > MAX_RENDERED_BYTES {
        return (plain_html(text), None);
    }
    let tables = table_cells_bound(text) <= MAX_TABLE_CELLS;
    let options = options(tables, rule);
    let mut source = markup.prepare(text, &options);
    if rule == SingleNewline::Soft {
        if let Some(listed) = soft_rule_lines(&source, &options, markup) {
            source = Cow::Owned(listed);
        }
    }
    if tables {
        if let Some(repaired) = repair_glued_tables(&source, &options) {
            if table_cells_bound(&repaired) <= MAX_TABLE_CELLS {
                source = Cow::Owned(repaired);
            }
        }
    }
    let arena = Arena::new();
    let mut root = comrak::parse_document(&arena, &source, &options);
    if deeper_than(root, MAX_DEPTH) {
        return (plain_html(text), None);
    }
    // Where a chat service ends a list at a line CommonMark runs into it, the text is told so with
    // a blank line and parsed again. `#237 nested-lists`.
    let mut inserted = Vec::new();
    if markup.list_ends_at_unindented_line() {
        if let Some(split) = end_lists_at_unindented_lines(&arena, root, &source, &options) {
            (source, inserted, root) = (Cow::Owned(split.source), split.inserted, split.root);
            if deeper_than(root, MAX_DEPTH) {
                return (plain_html(text), None);
            }
        }
    }
    adjust(&arena, root, &source, &inserted, markup, mentions);
    // Measured again: nesting the lists the writer indented makes the tree deeper than it parsed.
    if deeper_than(root, MAX_DEPTH) {
        return (plain_html(text), None);
    }
    let visited = visit(root, &source);
    let limit = text.len().saturating_mul(MAX_GROWTH) + GROWTH_ALLOWANCE;
    // Highlighting is what grows code most, a span for every token, so a body it takes past the
    // bound is written again without it before it is given up as plain paragraphs: its code is
    // still drawn as code, only in one colour.
    let highlighter = highlight::Highlighter::for_one_body();
    let mut plugins = comrak::options::Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(&highlighter);
    let html = write_html(root, &options, &plugins);
    let html = if html.len() > limit && has_code_block(root) {
        write_html(root, &options, &comrak::options::Plugins::default())
    } else {
        html
    };
    if html.len() > limit {
        return (plain_html(text), None);
    }
    (html, Some(visited))
}

/// The tree written as HTML with `plugins`, then sanitized.
fn write_html<'a>(
    root: &'a AstNode<'a>,
    options: &Options,
    plugins: &comrak::options::Plugins<'_>,
) -> String {
    let mut html = String::new();
    comrak::format_html_with_plugins(root, options, &mut html, plugins)
        .expect("formatting into a String cannot fail");
    mark_message_links(&SANITIZER.clean(&html).to_string())
}

/// `html`, sanitized, with every link to a chat message marked with what it names: the
/// [`crate::message_links::ATTRIBUTE`] right after its `href`. `#543 in-app-message-links`.
///
/// After the sanitizer, so only an address it kept is read, as the browser will read it. Every
/// `<a href="` in its output is a link comrak made — raw HTML in a message is escaped long before,
/// and code is text — and the value written is percent-encoded, so it needs no escaping. The
/// sanitizer keeps the attribute when it sees the body again ([`mark_restated`]).
fn mark_message_links(html: &str) -> String {
    const OPEN: &str = "<a href=\"";
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find(OPEN) {
        let start = at + OPEN.len();
        let Some(end) = rest[start..].find('"').map(|end| start + end) else {
            break;
        };
        out.push_str(&rest[..=end]);
        let mut href = String::with_capacity(end - start);
        push_decoded(&mut href, &rest[start..end]);
        if let Some(target) = crate::message_links::recognize(&href) {
            out.push_str(&format!(
                " {}=\"{}\"",
                crate::message_links::ATTRIBUTE,
                target.attribute()
            ));
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Whether the tree holds a fenced or indented code block.
fn has_code_block<'a>(root: &'a AstNode<'a>) -> bool {
    root.descendants()
        .any(|node| matches!(node.data.borrow().value, NodeValue::CodeBlock(_)))
}

// --- read-aloud blocks: `#376 read-aloud-blocks` ----------------------------------------------
//
// A long message is read one BLOCK at a time, the blocks being the elements the page draws it as:
// a paragraph, a heading, a list item, a block quote, a code block, a table. The page highlights
// the block being read and moves between blocks, so the server and the page must agree on what
// the blocks are, in order, without either sending the other any markup.
//
// They agree by both walking the same drawn structure by the same rule:
//
// - **The page** walks the body it inserted ([`body_html`], or [`plain_html`]'s paragraphs when
//   that is empty), element by element (`readingBlocks` in web/voice.js).
// - **The server** walks the tree this module renders that body from, and takes each block's
//   Markdown from the source positions of its node ([`source_blocks`]), so what is SAID of a block
//   goes through the same speech rules as a whole message.
//
// The rule, both sides: a `p`, an `h1` to `h6`, a `blockquote`, a `pre` and a `table` are each one
// block and nothing inside them is another; an `li` is one block, holding everything in it except
// the lists nested in it, whose items are blocks of their own; `ul`, `ol` and `div` are walked
// through; anything else is no block. A `hr` is never one.
//
// THE SERVER CHECKS ITSELF: the kinds it found in the tree must be the kinds [`html_block_kinds`]
// reads from the HTML it rendered, in order, or the message is one block. The page checks the
// same against the kinds it is sent, and highlights the whole body when they differ. A mismatch
// costs the highlight its precision, never the read.

/// What one block of a drawn body is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// A paragraph, `p`.
    Paragraph,
    /// A heading, `h1` to `h6`.
    Heading,
    /// A list item, `li`, without the lists nested in it.
    Item,
    /// A block quote, `blockquote`, whole.
    Quote,
    /// A code block, `pre`: said as a short description of it.
    Code,
    /// A table: said as a short description of it.
    Table,
}

/// One block of a body, in drawing order: its kind and the Markdown it was drawn from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceBlock {
    /// The element the page draws it as.
    pub kind: BlockKind,
    /// The text it was drawn from: a slice of what was parsed, never anything the page sent, less
    /// a list item's bullet.
    pub source: String,
}

/// The blocks `text` is drawn as, in order, written in `markup`, with `mentions` named as
/// [`to_html_named`] names them. See the section note.
///
/// One block, the whole text, when the tree and the HTML disagree about the blocks.
#[must_use]
pub fn source_blocks(text: &str, markup: Markup, mentions: &[MentionName]) -> Vec<SourceBlock> {
    let (html, walked) = render_visiting(text, markup, mentions, |root, source| {
        let lines = line_starts(source);
        let mut found = Vec::new();
        tree_blocks(root, source, &lines, Walk::Blocks, &mut found);
        found
    });
    let Some(blocks) = walked.filter(|_| html != plain_html(text)) else {
        // Drawn as plain paragraphs, by the page from the text itself.
        return plain_blocks(text);
    };
    let kinds: Vec<BlockKind> = blocks.iter().map(|block| block.kind).collect();
    if !blocks.is_empty() && kinds == html_block_kinds(&html) {
        blocks
    } else {
        vec![SourceBlock {
            kind: BlockKind::Paragraph,
            source: text.to_owned(),
        }]
    }
}

/// [`plain_html`]'s paragraphs: each run of lines that are not blank.
fn plain_blocks(text: &str) -> Vec<SourceBlock> {
    let mut blocks = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    for line in text.split('\n').chain(std::iter::once("")) {
        if line.trim_matches([' ', '\t']).is_empty() {
            if !run.is_empty() {
                blocks.push(SourceBlock {
                    kind: BlockKind::Paragraph,
                    source: run.join("\n"),
                });
                run.clear();
            }
        } else {
            run.push(line);
        }
    }
    blocks
}

/// Where a walk is: among blocks, or inside a list item, where only nested lists hold more.
#[derive(Clone, Copy)]
enum Walk {
    Blocks,
    ListsOnly,
}

/// The blocks under `node`, by the section note's rule, with their source.
fn tree_blocks<'a>(
    node: &'a AstNode<'a>,
    source: &str,
    lines: &[usize],
    walk: Walk,
    found: &mut Vec<SourceBlock>,
) {
    for child in node.children() {
        let ast = child.data.borrow();
        let kind = match (&ast.value, walk) {
            (NodeValue::List(_), _) => {
                drop(ast);
                tree_blocks(child, source, lines, Walk::Blocks, found);
                continue;
            }
            (_, Walk::ListsOnly) => continue,
            (NodeValue::Paragraph, Walk::Blocks) => BlockKind::Paragraph,
            (NodeValue::Heading(_), Walk::Blocks) => BlockKind::Heading,
            (NodeValue::BlockQuote, Walk::Blocks) => BlockKind::Quote,
            (NodeValue::CodeBlock(_), Walk::Blocks) => BlockKind::Code,
            (NodeValue::Table(_), Walk::Blocks) => BlockKind::Table,
            (NodeValue::Item(_) | NodeValue::TaskItem(_), Walk::Blocks) => BlockKind::Item,
            _ => continue,
        };
        let (start, end) = (ast.sourcepos.start, ast.sourcepos.end);
        drop(ast);
        let from = offset(lines, start.line, start.column).unwrap_or(source.len());
        let mut to = offset(lines, end.line, end.column).map_or(source.len(), |at| at + 1);
        // Never past where whatever follows it begins: a list's last item can claim the lines after
        // the list as its own.
        if let Some(next) = following(child) {
            let at = next.data.borrow().sourcepos.start;
            if let Some(at) = offset(lines, at.line, at.column) {
                to = to.min(at);
            }
        }
        found.push(SourceBlock {
            kind,
            source: if kind == BlockKind::Item {
                item_source(&item_text(child, source, lines, from))
            } else {
                slice_chars(source, from, to).to_owned()
            },
        });
        if kind == BlockKind::Item {
            tree_blocks(child, source, lines, Walk::ListsOnly, found);
        }
    }
}

/// What a list item holds besides the lists nested in it, from `from`, where it begins: each of
/// its other children in order, wherever in the source it came from. A paragraph after a nested
/// list, and a block that `#237 nested-lists` moved into the item, are its words too.
fn item_text<'a>(item: &'a AstNode<'a>, source: &str, lines: &[usize], from: usize) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for kid in item.children() {
        let ast = kid.data.borrow();
        if matches!(ast.value, NodeValue::List(_)) {
            continue;
        }
        let (start, end) = (ast.sourcepos.start, ast.sourcepos.end);
        drop(ast);
        // The first from the item's own start, so its marker comes with it to be taken off.
        let begin = if parts.is_empty() {
            from
        } else {
            offset(lines, start.line, start.column).unwrap_or(source.len())
        };
        let mut finish = offset(lines, end.line, end.column).map_or(source.len(), |at| at + 1);
        if let Some(next) = following(kid) {
            let at = next.data.borrow().sourcepos.start;
            if let Some(at) = offset(lines, at.line, at.column).filter(|at| *at >= begin) {
                finish = finish.min(at);
            }
        }
        parts.push(slice_chars(source, begin, finish));
    }
    parts.join(
        "

",
    )
}

/// The node after `node` in the document, outside it: its next sibling, or its parent's, and so on.
fn following<'a>(node: &'a AstNode<'a>) -> Option<&'a AstNode<'a>> {
    let mut at = node;
    loop {
        if let Some(next) = at.next_sibling() {
            return Some(next);
        }
        at = at.parent()?;
    }
}

/// A list item's text without the bullet the page draws for it, which a voice would say as a dash,
/// and a task's box said in words. A number stays: "2." is part of what the list says.
fn item_source(text: &str) -> String {
    let text = text.trim_start();
    let text = ["- ", "* ", "+ "]
        .iter()
        .find_map(|bullet| text.strip_prefix(bullet))
        .unwrap_or(text)
        .trim_start();
    if let Some(open) = text.strip_prefix("[ ] ") {
        format!("To do: {open}")
    } else if let Some(done) = text
        .strip_prefix("[x] ")
        .or_else(|| text.strip_prefix("[X] "))
    {
        format!("Done: {done}")
    } else {
        text.to_owned()
    }
}

// --- read-aloud pieces: `#486 read-aloud-pieces` ------------------------------------------------
//
// The owner, 2026-10-23, of a message whose bullets were "•" lines inside one paragraph: "why is it
// not split up into chunks with the floating controller for me to navigate within it? Shouldn't we
// have a Max chunk size in words? ... splitting along paragraph or list structures is much
// preferable to picking arbitrary breaks inside a paragraph. Having some heuristic to at least
// identify sentence boundaries would be nice."
//
// So a block is read in PIECES, each its own request and its own stop on the controller, by one
// rule the page applies to the text it drew and the server to the text the block was drawn from
// (`readPieces` in web/voice.js; both are held to tests/fixtures/read_aloud_pieces.json):
//
// - a line that begins with a bullet glyph (`READ_BULLETS`) is a piece of its own, and the lines
//   between such lines are one piece together, so a "•" list typed as text reads as a list;
// - a piece of more than `READ_PIECE_WORDS` words is cut at sentence ends (the page's sentence rule,
//   `sentencesOf`), as many whole sentences to a piece as fit;
// - only a single sentence longer than that is cut inside, into equal runs of words.
//
// A code block and a table are said as a short description and are never cut.

/// The most words one piece of a block says, unless one sentence is longer. About twenty seconds of
/// speech: short enough to step through, long enough that a paragraph of three sentences is one.
pub const READ_PIECE_WORDS: usize = 60;

/// The glyphs a line typed as a bullet begins with. Not `-`, `*` or `+`, which Markdown already makes
/// a list of, and not a dash, which begins sentences as often as items.
pub const READ_BULLETS: &[char] = &[
    '•', '◦', '▪', '▫', '‣', '⁃', '●', '○', '■', '□', '▸', '▹', '►',
];

/// The pieces `text` is read in, in order, each a trimmed slice of it. See the section note.
#[must_use]
pub fn read_pieces(text: &str) -> Vec<&str> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    let mut at = 0;
    for line in text.split('\n') {
        let (from, to) = (at, at + line.len());
        at = to + 1;
        // A quote's `>` markers are not drawn: its typed bullets begin lines as the page sees them.
        let drawn = line.trim_start().trim_start_matches('>').trim_start();
        if drawn.starts_with(READ_BULLETS) {
            groups.extend(run.take());
            groups.push((from, to));
        } else {
            run = Some(run.map_or((from, to), |(start, _)| (start, to)));
        }
    }
    groups.extend(run);
    let mut pieces = Vec::new();
    for (from, to) in groups {
        pack_sentences(text, from, to, &mut pieces);
    }
    pieces
        .into_iter()
        .map(|(from, to)| text[from..to].trim())
        .filter(|piece| piece.split_whitespace().next().is_some())
        .collect()
}

/// What a piece says aloud: its words, less the bullet glyph it was typed with.
#[must_use]
pub fn piece_source(piece: &str) -> &str {
    piece
        .trim_start()
        .trim_start_matches(READ_BULLETS)
        .trim_start()
}

/// Cut `text[from..to]` into pieces of whole sentences of at most [`READ_PIECE_WORDS`] words, a
/// longer sentence into equal runs of words, and push their ranges.
fn pack_sentences(text: &str, from: usize, to: usize, pieces: &mut Vec<(usize, usize)>) {
    let words = |a: usize, b: usize| text[a..b].split_whitespace().count();
    if words(from, to) <= READ_PIECE_WORDS {
        pieces.push((from, to));
        return;
    }
    let mut current: Option<(usize, usize, usize)> = None;
    for (start, end) in sentence_ranges(text, from, to) {
        let count = words(start, end);
        if count > READ_PIECE_WORDS {
            pieces.extend(current.take().map(|(a, b, _)| (a, b)));
            even_runs(text, start, end, count, pieces);
            continue;
        }
        current = match current {
            Some((a, _, held)) if held + count <= READ_PIECE_WORDS => Some((a, end, held + count)),
            held => {
                pieces.extend(held.map(|(a, b, _)| (a, b)));
                Some((start, end, count))
            }
        };
    }
    pieces.extend(current.map(|(a, b, _)| (a, b)));
}

/// The sentences of `text[from..to]`: each ends at `.`, `!`, `?` or `…`, with any closing quote or
/// bracket after it, followed by white space or the end. The page's `sentencesOf` rule, on the drawn
/// text; here, on the source, never inside markup the page does not draw as text (see
/// [`inside_markup`]), so no piece holds half a code span or half a link.
fn sentence_ranges(text: &str, from: usize, to: usize) -> Vec<(usize, usize)> {
    let span = &text[from..to];
    let inside = inside_markup(span);
    let mut found = Vec::new();
    let mut start = 0;
    let mut chars = span.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !matches!(c, '.' | '!' | '?' | '…') || inside[at] {
            continue;
        }
        let mut end = at + c.len_utf8();
        while let Some(&(next_at, next)) = chars.peek() {
            if !matches!(
                next,
                '.' | '!' | '?' | '…' | '"' | '\'' | '”' | '’' | ')' | ']'
            ) {
                break;
            }
            end = next_at + next.len_utf8();
            chars.next();
        }
        if span[end..].chars().next().is_none_or(char::is_whitespace) {
            found.push((from + start, from + end));
            start = end;
        }
    }
    if start < span.len() {
        found.push((from + start, to));
    }
    found
}

/// For each byte of `span`, whether it is inside an inline code span, a link's text in `[...]`, or a
/// link's target in the `(...)` after it, as Markdown writes them.
fn inside_markup(span: &str) -> Vec<bool> {
    let mut inside = vec![false; span.len() + 1];
    let (mut code, mut square, mut target, mut opening) = (false, 0usize, 0usize, false);
    let mut chars = span.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        let here = code || square > 0 || target > 0;
        inside[at..at + c.len_utf8()].fill(here);
        match c {
            '`' => code = !code,
            _ if code => {}
            '[' => square += 1,
            ']' if square > 0 => {
                square -= 1;
                opening = chars.peek().is_some_and(|&(_, next)| next == '(');
            }
            '(' if opening => {
                opening = false;
                target = 1;
            }
            '(' if target > 0 => target += 1,
            ')' if target > 0 => target -= 1,
            _ => {}
        }
    }
    inside
}

/// `text[from..to]`, of `count` words, cut into the fewest runs of at most [`READ_PIECE_WORDS`]
/// words, as equal as words allow, the longer runs first.
fn even_runs(text: &str, from: usize, to: usize, count: usize, pieces: &mut Vec<(usize, usize)>) {
    let runs = count.div_ceil(READ_PIECE_WORDS);
    let (size, longer) = (count / runs, count % runs);
    let mut bounds = Vec::new();
    let mut in_word = false;
    let mut seen = 0;
    let mut wanted = size + usize::from(longer > 0);
    for (at, c) in text[from..to].char_indices() {
        let space = c.is_whitespace();
        if !space && !in_word {
            if seen == wanted {
                bounds.push(from + at);
                wanted += size + usize::from(bounds.len() < longer);
            }
            seen += 1;
        }
        in_word = !space;
    }
    let mut start = from;
    for bound in bounds {
        pieces.push((start, bound));
        start = bound;
    }
    pieces.push((start, to));
}

/// `source[from..to]`, each end moved back onto a character boundary and kept inside the text.
fn slice_chars(source: &str, from: usize, to: usize) -> &str {
    let boundary = |mut at: usize| {
        at = at.min(source.len());
        while !source.is_char_boundary(at) {
            at -= 1;
        }
        at
    };
    let (from, to) = (boundary(from), boundary(to));
    if from < to {
        &source[from..to]
    } else {
        ""
    }
}

/// The kinds of the blocks in sanitized `html`, by the section note's rule: what the page finds
/// walking the body it inserted. Only for what [`SANITIZER`] wrote: well-formed and properly
/// nested tags, attribute values in double quotes.
#[must_use]
pub fn html_block_kinds(html: &str) -> Vec<BlockKind> {
    #[derive(Clone, Copy, PartialEq)]
    enum Inside {
        Blocks,
        ListsOnly,
        Nothing,
    }
    let bytes = html.as_bytes();
    let mut kinds = Vec::new();
    let mut open: Vec<(&str, Inside)> = Vec::new();
    let mut at = 0;
    while let Some(found) = html[at..].find('<') {
        let start = at + found + 1;
        let closing = bytes.get(start) == Some(&b'/');
        let name_from = start + usize::from(closing);
        let name_len = bytes[name_from..]
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric())
            .count();
        let name = &html[name_from..name_from + name_len];
        let mut end = name_from + name_len;
        let mut quoted = false;
        while end < bytes.len() && (quoted || bytes[end] != b'>') {
            quoted ^= bytes[end] == b'"';
            end += 1;
        }
        at = (end + 1).min(bytes.len());
        if name.is_empty() {
            continue;
        }
        if closing {
            if let Some(level) = open.iter().rposition(|(tag, _)| *tag == name) {
                open.truncate(level);
            }
            continue;
        }
        if matches!(name, "br" | "hr" | "img" | "input" | "wbr") {
            continue;
        }
        let here = open.last().map_or(Inside::Blocks, |(_, inside)| *inside);
        let inner = match (here, name) {
            (Inside::Blocks | Inside::ListsOnly, "ul" | "ol") | (Inside::Blocks, "div") => {
                Inside::Blocks
            }
            (Inside::Blocks, "li") => {
                kinds.push(BlockKind::Item);
                Inside::ListsOnly
            }
            (Inside::Blocks, _) => {
                let kind = match name {
                    "p" => Some(BlockKind::Paragraph),
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Some(BlockKind::Heading),
                    "blockquote" => Some(BlockKind::Quote),
                    "pre" => Some(BlockKind::Code),
                    "table" => Some(BlockKind::Table),
                    _ => None,
                };
                kinds.extend(kind);
                Inside::Nothing
            }
            _ => Inside::Nothing,
        };
        open.push((name, inner));
    }
    kinds
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

/// comrak's settings, with or without tables, under one single-newline rule. A function rather
/// than a static because they are a handful of booleans.
fn options(tables: bool, rule: SingleNewline) -> Options<'static> {
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
    // Only a blank line starts a new paragraph. Whether a single newline inside one is a line break
    // or a space is the configured rule: see `SingleNewline`.
    options.render.hardbreaks = rule == SingleNewline::Hard;
    // Raw HTML in a message is shown as the text it is. Never passed through: a message is not
    // markup, and the sanitizer below would remove it anyway, along with its content's meaning.
    options.render.escape = true;
    // Classes the stylesheet uses to drop the bullet beside a checkbox.
    options.render.tasklist_classes = true;
    options
}

// --- single newlines: `markdown-tables-render` ------------------------------------------------
//
// Chat's old line rule was that every newline is a line break. The owner revisited it on
// 2026-10-26, of agent posts whose prose broke in the middle of sentences: an agent's Markdown is
// often hard-wrapped (by its writer at a fixed width, or by a terminal it was captured from), and
// standard Markdown joins those lines and lets the screen wrap them. "For now let's just soft wrap
// all single newlines and I'll revisit later." So the rule is a setting, `[render]
// single_newline`, soft by default, with the hard rule kept and tested; and `newline_rule` is the
// one place a later per-message choice (an "is this Markdown?" guess, or the sender) would go.
//
// Under the soft rule a line typed as a bullet would join the line above it, so such lines are
// made Markdown list items first (`soft_rule_lines`): a Slack message's "•" lines, which is
// how Slack's own text writes a list, stay one line each, as items. So do the lines naming the
// files a Slack message carries.

/// What a single newline inside a paragraph is. A blank line starts a new paragraph under both.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum SingleNewline {
    /// A space, as CommonMark has it: the lines of a paragraph are joined and the screen wraps
    /// them. The default since 2026-10-26.
    #[default]
    Soft,
    /// A line break, chat's old rule: every line is drawn as the line it was written on.
    Hard,
}

/// The rule this process renders with: [`SingleNewline::Soft`] until [`set_single_newline`].
static SINGLE_NEWLINE_HARD: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Render every body after this with `rule`, as `[render] single_newline` configures it. Called
/// once at startup; a test that needs the other rule asks for it with [`to_html_as`] instead.
pub fn set_single_newline(rule: SingleNewline) {
    SINGLE_NEWLINE_HARD.store(
        rule == SingleNewline::Hard,
        std::sync::atomic::Ordering::Relaxed,
    );
}

#[cfg(test)]
thread_local! {
    /// A test's own rule, on its own thread, so tests of either rule run side by side.
    static TEST_SINGLE_NEWLINE: std::cell::Cell<Option<SingleNewline>> =
        const { std::cell::Cell::new(None) };
}

/// This thread renders under a test's own rule until the guard is dropped.
#[cfg(test)]
pub(crate) struct SingleNewlineGuard(Option<SingleNewline>);

#[cfg(test)]
impl SingleNewlineGuard {
    pub(crate) fn set(rule: SingleNewline) -> Self {
        Self(TEST_SINGLE_NEWLINE.with(|cell| cell.replace(Some(rule))))
    }
}

#[cfg(test)]
impl Drop for SingleNewlineGuard {
    fn drop(&mut self) {
        TEST_SINGLE_NEWLINE.with(|cell| cell.set(self.0));
    }
}

/// The configured rule. See [`set_single_newline`].
#[must_use]
pub fn single_newline() -> SingleNewline {
    #[cfg(test)]
    if let Some(rule) = TEST_SINGLE_NEWLINE.with(std::cell::Cell::get) {
        return rule;
    }
    if SINGLE_NEWLINE_HARD.load(std::sync::atomic::Ordering::Relaxed) {
        SingleNewline::Hard
    } else {
        SingleNewline::Soft
    }
}

/// The rule one body is drawn with. Today the configured rule for every body; the place for a
/// choice per message — by its sender, or by whether its text reads as Markdown — if the owner
/// wants one later.
fn newline_rule(_text: &str, _markup: Markup) -> SingleNewline {
    single_newline()
}

/// `text` with the lines the soft rule must keep as lines marked so in Markdown, or `None` when
/// it has none outside code and raw HTML:
///
/// - a line that begins with a typed bullet ([`READ_BULLETS`]), a space and some text — in a quote
///   too, after its `>` — is begun with a Markdown bullet instead, keeping its indent, so that it
///   is a list item rather than a line of a paragraph. Slack writes a list in its text as `• item`
///   lines, and a nested one as `    ◦ item`, which CommonMark nests under the item above it once
///   both are Markdown items.
/// - on Discord, a line of prose at the margin straight after such items gets a blank line before
///   it, so it stays a line of its own rather than more of the last item, unless it reads as the
///   rest of the item's sentence ([`continues_item`]) or the blank line would change what the
///   message shows (an emphasis or a link running across it, [`shown_text`]). Google Chat and
///   Slack end a list there already ([`end_lists_at_unindented_lines`]), with the same checks. Not
///   a row of a table (`|`), which a later repair may need right where it is.
/// - a line naming a file a message carries ([`crate::slack::files::FILE_MARK`]) under another
///   such line ends that line with a backslash, Markdown's line break, so each file is still drawn
///   on a line of its own.
fn soft_rule_lines(text: &str, options: &Options, markup: Markup) -> Option<String> {
    use crate::slack::files::FILE_MARK;
    // Where a line's typed bullet is, after any quote markers and indent, and how wide it is.
    let bullet_at = |line: &str| {
        let lead = line.len() - line.trim_start_matches(['>', ' ', '\t']).len();
        let mut rest = line[lead..].chars();
        let glyph = rest.next().filter(|glyph| READ_BULLETS.contains(glyph))?;
        let after = &line[lead + glyph.len_utf8()..];
        (after.starts_with([' ', '\t']) && !after.trim().is_empty())
            .then_some((lead, glyph.len_utf8()))
    };
    let file_line = |line: &str| line.starts_with(FILE_MARK);
    let lines: Vec<&str> = text.split('\n').collect();
    let marked = |index: usize| {
        bullet_at(lines[index]).is_some()
            || (index > 0 && file_line(lines[index]) && file_line(lines[index - 1]))
    };
    if !(0..lines.len()).any(marked) {
        return None;
    }
    let protected = protected_ranges(text, options);
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in &lines {
        starts.push(at);
        at += line.len() + 1;
    }
    let outside = |index: usize, column: usize| {
        !protected
            .iter()
            .any(|range| range.contains(&(starts[index] + column)))
    };
    let separate = !markup.list_ends_at_unindented_line();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut joined: Vec<String> = Vec::with_capacity(lines.len());
    let mut changed = false;
    let mut item: Option<&str> = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some((lead, width)) = bullet_at(line).filter(|&(lead, _)| outside(index, lead)) {
            let converted = format!("{}-{}", &line[..lead], &line[lead + width..]);
            out.push(converted.clone());
            joined.push(converted);
            changed = true;
            item = (lead == 0).then_some(*line);
            continue;
        }
        if let Some(previous) = item.take() {
            if separate
                && line.starts_with(|c: char| !c.is_whitespace() && c != '|' && c != '>')
                && outside(index, 0)
                && !continues_item(previous, line)
            {
                out.push(String::new());
            }
        }
        if index > 0
            && file_line(line)
            && file_line(lines[index - 1])
            && outside(index - 1, 0)
            && !lines[index - 1].trim_end_matches('\r').ends_with('\\')
        {
            for written in [&mut out, &mut joined] {
                if let Some(previous) = written.last_mut() {
                    let end = previous.trim_end_matches('\r').len();
                    previous.insert(end, '\\');
                }
            }
            changed = true;
        }
        out.push((*line).to_owned());
        joined.push((*line).to_owned());
    }
    if !changed {
        return None;
    }
    if out.len() != joined.len() {
        let arena = Arena::new();
        let separated = out.join("\n");
        let together = joined.join("\n");
        let same = shown_text(comrak::parse_document(&arena, &separated, options))
            == shown_text(comrak::parse_document(&arena, &together, options));
        return Some(if same { separated } else { together });
    }
    Some(out.join("\n"))
}

/// Where `text` holds code or raw HTML, by byte: [`code_ranges`], and each block of raw HTML as the
/// whole lines it spans.
fn protected_ranges(text: &str, options: &Options) -> Vec<Range<usize>> {
    let mut ranges = code_ranges(text, options);
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, text, options);
    let lines = line_starts(text);
    let line_start = |line: usize| lines.get(line.wrapping_sub(1)).copied();
    for node in root.descendants() {
        let ast = node.data.borrow();
        if matches!(ast.value, NodeValue::HtmlBlock(_)) {
            ranges
                .extend(line_start(ast.sourcepos.start.line).map(|from| {
                    from..line_start(ast.sourcepos.end.line + 1).unwrap_or(text.len())
                }));
        }
    }
    ranges
}

// --- a table whose line breaks were lost: `markdown-tables-render` ----------------------------
//
// A GFM table needs its header row on a line of its own, and nothing after its last row's closing
// pipe. A message whose line breaks were lost before it got here — Slack's own conversion of an
// agent's Markdown stored one as `…scorecard):| Backend | Two runs |` and `| alpha | 1 | 2 |Most
// gaps are…` — draws as rows of pipes, or worse, as a table that silently drops the prose glued
// to its last row, which GFM reads as an extra cell. Each is repaired, and only where the reading
// is unambiguous:
//
// - the line under it is a delimiter row written with a leading pipe, `|---|:-:|`, of N cells;
// - a glued header: a line that does not already start a table, whose text before its first pipe
//   is prose glued to that pipe (no space between them, not only a list marker, and the pipe
//   outside any code span), and from that pipe on is a row of exactly N cells ending in a pipe.
//   No row under it may have more cells than N, which GFM would drop. The row goes on a line of
//   its own after a blank line;
// - a glued last row: the last row of such a table, or of one that already parses, starting with
//   a pipe, whose (N + 1)th pipe is followed at once, with no space, by text holding no other
//   pipe. That text goes into a paragraph of its own after a blank line.
//
// Code and raw HTML are left alone, a text with a lone carriage return (which comrak counts as a
// line end where this does not) is not repaired, and the result is parsed again: every repaired
// header must now start a table, and the code must be the same code, or nothing is repaired.

/// The byte offsets of `line`'s pipes that are not escaped with a backslash, as GFM splits cells.
fn unescaped_pipes(line: &str) -> Vec<usize> {
    let bytes = line.as_bytes();
    (0..bytes.len())
        .filter(|&at| bytes[at] == b'|' && (at == 0 || bytes[at - 1] != b'\\'))
        .collect()
}

/// The number of cells of a GFM delimiter row written with a leading pipe, such as `|---|:-:|`.
fn delimiter_cells(line: &str) -> Option<usize> {
    let line = line.trim_end_matches([' ', '\t', '\r']);
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let body = line[indent..].strip_prefix('|')?;
    let body = body.strip_suffix('|').unwrap_or(body);
    let cells: Vec<&str> = body.split('|').collect();
    cells
        .iter()
        .all(|cell| {
            let cell = cell.trim_matches([' ', '\t']);
            let cell = cell.strip_prefix(':').unwrap_or(cell);
            let cell = cell.strip_suffix(':').unwrap_or(cell);
            !cell.is_empty() && cell.bytes().all(|byte| byte == b'-')
        })
        .then_some(cells.len())
}

/// Whether `line` starts, after at most three spaces, with a pipe: a row of a table written with
/// leading pipes.
fn starts_row(line: &str) -> bool {
    let indent = line.len() - line.trim_start_matches(' ').len();
    indent <= 3 && line[indent..].starts_with('|')
}

/// Whether `prefix`, the text before a glued header row, is only a list marker, a typed bullet or
/// quote markers: the row then starts the item or the quote, and is not glued to anything.
fn only_marker(prefix: &str) -> bool {
    let prefix = prefix.trim_start_matches(['>', ' ', '\t']).trim_end();
    prefix.is_empty()
        || line_marker(prefix).is_some_and(|marker| marker.empty)
        || (prefix.chars().count() == 1 && prefix.starts_with(READ_BULLETS))
}

/// What a parse of `text` says about its lines: the 1-based lines a code block or a block of raw
/// HTML holds, the lines a table starts on, and the code it shows.
fn table_facts(text: &str, options: &Options) -> (HashSet<usize>, HashSet<usize>, Vec<String>) {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, text, options);
    let (mut code, mut tables, mut literals) = (HashSet::new(), HashSet::new(), Vec::new());
    for node in root.descendants() {
        let ast = node.data.borrow();
        match &ast.value {
            NodeValue::CodeBlock(block) => {
                code.extend(ast.sourcepos.start.line..=ast.sourcepos.end.line);
                literals.push(block.literal.clone());
            }
            NodeValue::HtmlBlock(block) => {
                code.extend(ast.sourcepos.start.line..=ast.sourcepos.end.line);
                literals.push(block.literal.clone());
            }
            NodeValue::Code(span) => literals.push(span.literal.clone()),
            NodeValue::Table(_) => {
                tables.insert(ast.sourcepos.start.line);
            }
            _ => {}
        }
    }
    (code, tables, literals)
}

/// `text` with each table whose line breaks were lost put back on its lines, or `None` when there
/// is none to repair. See the section note.
fn repair_glued_tables(text: &str, options: &Options) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let delimiters: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, line)| delimiter_cells(line).map(|cells| (index, cells)))
        .collect();
    if delimiters.is_empty() || text.replace("\r\n", "").contains('\r') {
        return None;
    }
    let (code, tables, literals) = table_facts(text, options);
    let in_code = |index: usize| code.contains(&(index + 1));
    let spans = code_ranges(text, options);
    let mut starts = Vec::with_capacity(lines.len());
    let mut at = 0;
    for line in &lines {
        starts.push(at);
        at += line.len() + 1;
    }
    // The rows under a delimiter: the lines after it that start with a pipe, up to the next
    // delimiter row.
    let rows_after = |delimiter: usize| {
        (delimiter + 1..lines.len())
            .take_while(|&index| {
                starts_row(lines[index])
                    && !in_code(index)
                    && delimiter_cells(lines[index]).is_none()
            })
            .collect::<Vec<_>>()
    };
    // Per line: the text that replaces it, as one or more lines.
    let mut replaced: HashMap<usize, Vec<String>> = HashMap::new();
    let mut headers = Vec::new();
    for (delimiter, cells) in delimiters {
        let header = delimiter - 1;
        if in_code(delimiter) || in_code(header) || replaced.contains_key(&header) {
            continue;
        }
        let rows = rows_after(delimiter);
        if !tables.contains(&(header + 1)) {
            let line = lines[header];
            let pipes = unescaped_pipes(line);
            let Some(&first) = pipes.first() else {
                continue;
            };
            let prefix = &line[..first];
            let row = line[first..].trim_end_matches([' ', '\t', '\r']);
            let glued = prefix.ends_with(|c: char| !c.is_whitespace()) && !only_marker(prefix);
            // In a code span, closed or (to CommonMark, which then shows the backtick) not.
            let in_span = prefix.matches('`').count() % 2 == 1
                || spans
                    .iter()
                    .any(|range| range.contains(&(starts[header] + first)));
            let row_cells = unescaped_pipes(row).len().saturating_sub(1);
            let wider = rows
                .iter()
                .any(|&index| unescaped_pipes(lines[index]).len() > cells + 1);
            if !glued || in_span || !row.ends_with('|') || row_cells != cells || wider {
                continue;
            }
            replaced.insert(
                header,
                vec![prefix.trim_end().to_owned(), String::new(), row.to_owned()],
            );
            headers.push(header);
        }
        let Some(&last) = rows.last() else {
            continue;
        };
        let line = lines[last];
        let pipes = unescaped_pipes(line);
        if pipes.len() != cells + 1 {
            continue;
        }
        let close = pipes[cells];
        let after = &line[close + 1..];
        if after.starts_with(|c: char| !c.is_whitespace()) && !replaced.contains_key(&last) {
            replaced.insert(
                last,
                vec![line[..=close].to_owned(), String::new(), after.to_owned()],
            );
        }
    }
    if replaced.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(lines.len() + 2 * replaced.len());
    let mut header_lines = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match replaced.get(&index) {
            Some(parts) => {
                if headers.contains(&index) {
                    header_lines.push(out.len() + parts.len());
                }
                out.extend(parts.iter().map(String::as_str));
            }
            None => out.push(line),
        }
    }
    let repaired = out.join("\n");
    let (_, repaired_tables, repaired_literals) = table_facts(&repaired, options);
    (repaired_literals == literals
        && header_lines
            .iter()
            .all(|line| repaired_tables.contains(line)))
    .then_some(repaired)
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

/// The class a mention chip carries. The page styles it. The only other classes a `span` may keep
/// are [`RESTATED_CLASS`] and the highlighter's [`highlight::CLASSES`].
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
            (
                "a",
                HashSet::from(["href", "title", crate::message_links::ATTRIBUTE]),
            ),
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
        // `rh-src` marks the words a message's heading restates (`#242 rich-headers`, see
        // `mark_restated`). Message text cannot write it: raw HTML in the source is escaped by
        // comrak before the sanitizer sees anything, so the only markup that reaches it is markup
        // this module made.
        .allowed_classes(HashMap::from([
            (
                "span",
                [MENTION_CLASS, RESTATED_CLASS]
                    .into_iter()
                    .chain(highlight::CLASSES)
                    .collect(),
            ),
            ("p", HashSet::from([RESTATED_CLASS])),
            ("ul", HashSet::from(["contains-task-list"])),
            ("ol", HashSet::from(["contains-task-list"])),
            ("li", HashSet::from(["task-list-item"])),
            ("input", HashSet::from(["task-list-item-checkbox"])),
        ]))
        // A URL is a sink. `javascript:` and `data:` execute, and a relative address resolves
        // against this origin and can be made to look like somewhere else entirely. Such a link
        // keeps its text and loses its address, so the reader still sees what was written. `slack`
        // reaches here only on a link to a Slack message [`adjust`] kept, which opens Slack's app
        // and nothing else (`#543 in-app-message-links`); every other one is shown as text there.
        .url_schemes(HashSet::from(["http", "https", "slack"]))
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(Some("noopener noreferrer nofollow"))
        .strip_comments(true);
    builder
});

/// The changes made to the parsed tree before it is written. See the module note, step 3.
///
/// Two walks, because the second reads what the first made: an image is a link by the time links
/// are checked, and a block of raw HTML is text by the time mentions are looked for.
fn adjust<'a>(
    arena: &'a Arena<'a>,
    root: &'a AstNode<'a>,
    source: &str,
    inserted: &[usize],
    markup: Markup,
    mentions: &[MentionName],
) {
    let lines = line_starts(source);
    nest_lists(
        arena,
        root,
        &Lines {
            source,
            starts: &lines,
            inserted,
        },
        markup,
    );
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
            NodeValue::Link(link) if !web_address(&link.url) && !slack_app_link(&link.url) => {
                let url = link.url.clone();
                drop(ast);
                unwrap_link(arena, node, url);
            }
            NodeValue::Link(_) => {
                drop(ast);
                label_link(node);
            }
            NodeValue::Text(text) => {
                let pieces = mention_pieces(text, mentions);
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

/// Whether a link's address is one of Slack's app links to a message or a conversation, which a
/// tap may follow: into the app here when the page serves the channel, into Slack's own app when it
/// does not. `#543 in-app-message-links`. Any other `slack:` address is shown as text.
fn slack_app_link(url: &str) -> bool {
    url.get(..6)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("slack:"))
        && crate::message_links::recognize(url).is_some()
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

/// One piece of a text node: words, or a chip and the text it shows.
enum Piece {
    Text(String),
    Chip(String),
}

/// A text node's text, cut at every mention. A user mention `mentions` names shows `@` and the
/// name; any other shows its id with `@` or `#`.
fn mention_pieces(text: &str, mentions: &[MentionName]) -> Vec<Piece> {
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
                let named = chip.strip_prefix('@').and_then(|id| {
                    mentions
                        .iter()
                        .find(|mention| mention.id == id)
                        .map(|mention| format!("@{}", mention.name))
                });
                pieces.push(Piece::Chip(named.unwrap_or(chip)));
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
/// A chip is a raw node: its text is the mention's id with `@` or `#`, which
/// [`crate::discord::markup::mention_at`] admits only as ASCII letters and digits, or `@` and the
/// name of the person it names, which is anybody's choice and is escaped here. The sanitizer still
/// reads it, keeping the `span` and its one allowed class and nothing else.
fn replace_with<'a>(arena: &'a Arena<'a>, node: &'a AstNode<'a>, pieces: Vec<Piece>) {
    for piece in pieces {
        let value = match piece {
            Piece::Text(text) => NodeValue::Text(text.into()),
            Piece::Chip(chip) => {
                let mut raw = format!("<span class=\"{MENTION_CLASS}\">");
                escape_text(&mut raw, &chip);
                raw.push_str("</span>");
                NodeValue::Raw(raw)
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

// --- lists as a chat client draws them: `#237 nested-lists` -----------------------------------

/// How many columns `text` spans, as CommonMark counts them: a tab reaches the next multiple of
/// four.
fn columns(text: &str) -> usize {
    text.chars().fold(0, |column, character| {
        if character == '\t' {
            column + 4 - column % 4
        } else {
            column + 1
        }
    })
}

/// A list marker at the start of a line's text, read the way CommonMark reads one.
#[derive(Clone, Copy, Debug)]
struct LineMarker {
    /// The number before an ordered marker's delimiter; `None` for a bullet.
    number: Option<u64>,
    /// Whether nothing follows the marker on its line.
    empty: bool,
}

impl LineMarker {
    /// Whether this item may start a new list straight after a line of prose, which it would
    /// otherwise continue: in CommonMark only a bullet with text, or a numbered item at `1`.
    fn may_interrupt_prose(self) -> bool {
        !self.empty && self.number.is_none_or(|number| number == 1)
    }
}

/// The list marker `text` starts with, if it starts with one: `-`, `*` or `+`, or up to nine digits
/// and `.` or `)`, followed by a space, a tab or the end of the line — and not a rule, which `---`
/// and `* * *` are although they start like bullets.
fn line_marker(text: &str) -> Option<LineMarker> {
    let bytes = text.as_bytes();
    let (number, length) = match *bytes.first()? {
        b'-' | b'*' | b'+' => (None, 1),
        b'0'..=b'9' => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            (text[..digits].parse().ok(), digits + 1)
        }
        _ => return None,
    };
    let rest = &text[length..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) || is_rule(text) {
        return None;
    }
    Some(LineMarker {
        number,
        empty: rest.trim_matches([' ', '\t']).is_empty(),
    })
}

/// Whether `text` is a thematic break: three or more of one of `-`, `*` and `_`, and nothing else
/// but spaces and tabs.
fn is_rule(text: &str) -> bool {
    let mut marks = text
        .chars()
        .filter(|character| !matches!(character, ' ' | '\t'));
    let Some(first) = marks.next() else {
        return false;
    };
    matches!(first, '-' | '*' | '_')
        && marks.clone().all(|mark| mark == first)
        && marks.count() >= 2
}

/// The text with a blank line put before each line a chat service draws as the end of a list, the
/// lines that are those blank lines, and the tree parsed from it. See
/// [`end_lists_at_unindented_lines`].
struct Split<'a> {
    source: String,
    /// The 1-based lines of `source` this module put in, in order.
    inserted: Vec<usize>,
    root: &'a AstNode<'a>,
}

/// Put a blank line before each line a chat service draws as the end of a list and CommonMark
/// reads as more of its last item — prose at the left margin, straight after a line of the list —
/// and parse the text again; or `None`, when there is no such line or the new text would read as
/// anything more than that. See [`Markup::list_ends_at_unindented_line`].
///
/// The lines are found in the tree `root` parsed from `source`, so code, raw HTML and quotes are
/// whatever CommonMark says they are: a candidate is a LAZY line, one at the margin inside the
/// paragraph of a list item, which no fenced or indented code block, raw HTML or table holds
/// ([`paragraph_break`]). No line is changed or moved; one is only given a blank line before it.
///
/// That blank line may change nothing else, and the new tree is checked for it: every block that
/// is not a paragraph or a list — every item, code block, quote, heading, rule, table and block of
/// raw HTML — must start on the line it started on, after the lines put in, and the text, code and
/// raw HTML of the whole message must be the same characters ([`shown_text`]). Otherwise nothing
/// is put in at all. That is what keeps a list's next marker a marker, the code an item holds the
/// same code and a pair of backticks a code span, whatever shape is missed above.
fn end_lists_at_unindented_lines<'a>(
    arena: &'a Arena<'a>,
    root: &'a AstNode<'a>,
    source: &str,
    options: &Options,
) -> Option<Split<'a>> {
    let starts = line_starts(source);
    let lines = Lines {
        source,
        starts: &starts,
        inserted: &[],
    };
    let mut breaks: Vec<usize> = root
        .descendants()
        .filter_map(|node| paragraph_break(node, &lines))
        .collect();
    if breaks.is_empty() {
        return None;
    }
    breaks.sort_unstable();
    let mut text = String::with_capacity(source.len() + breaks.len());
    let mut copied = 0;
    for &line in &breaks {
        let at = starts[line - 1];
        text.push_str(&source[copied..at]);
        text.push('\n');
        copied = at;
    }
    text.push_str(&source[copied..]);
    let split = comrak::parse_document(arena, &text, options);
    let moved = |line: usize| line + breaks.partition_point(|&at| at <= line);
    if block_starts(root, moved) != block_starts(split, |line| line)
        || shown_text(root) != shown_text(split)
    {
        return None;
    }
    Some(Split {
        source: text,
        inserted: breaks
            .iter()
            .enumerate()
            .map(|(k, line)| line + k)
            .collect(),
        root: split,
    })
}

/// Each block under `root` that is not a paragraph or a list, as its kind and the line it starts
/// on, read through `line`.
fn block_starts<'a>(
    root: &'a AstNode<'a>,
    line: impl Fn(usize) -> usize,
) -> Vec<(std::mem::Discriminant<NodeValue>, usize)> {
    root.descendants()
        .filter_map(|node| {
            let ast = node.data.borrow();
            let counted = ast.value.block()
                && !matches!(
                    ast.value,
                    NodeValue::Document | NodeValue::Paragraph | NodeValue::List(_)
                );
            counted.then(|| {
                (
                    std::mem::discriminant(&ast.value),
                    line(ast.sourcepos.start.line),
                )
            })
        })
        .collect()
}

/// The characters a tree shows, in order and without whitespace: its text, its code and its raw
/// HTML. A marker read as text, a fence read as indented code, or backticks or asterisks that pair
/// up differently once a paragraph is split, change it; where one paragraph ends and the next
/// begins does not.
fn shown_text<'a>(root: &'a AstNode<'a>) -> String {
    let mut shown = String::new();
    for node in root.descendants() {
        match &node.data.borrow().value {
            NodeValue::Text(text) => shown.push_str(text),
            NodeValue::Code(code) => shown.push_str(&code.literal),
            NodeValue::CodeBlock(code) => shown.push_str(&code.literal),
            NodeValue::HtmlBlock(html) => shown.push_str(&html.literal),
            NodeValue::HtmlInline(html) => shown.push_str(html),
            _ => {}
        }
    }
    shown.retain(|character| !character.is_whitespace());
    shown
}

/// The line of `node` that a chat service starts a new paragraph at, when `node` is the paragraph
/// of a list item and has one: the first line at the left margin — a lazy line, to CommonMark —
/// that reads as a line of its own ([`continues_item`]) and that no inline element runs on to.
///
/// Not in a quote, whose every line starts with its marker, so a blank line before one would end
/// it; and not when the line after the paragraph would join it, as the next marker of a numbered
/// list would ([`would_join_paragraph`]): there the line at the margin is more of the item it sits
/// in the middle of.
fn paragraph_break<'a>(node: &'a AstNode<'a>, lines: &Lines<'_>) -> Option<usize> {
    let (first, last) = {
        let ast = node.data.borrow();
        if !matches!(ast.value, NodeValue::Paragraph) {
            return None;
        }
        (ast.sourcepos.start.line, ast.sourcepos.end.line)
    };
    if last <= first {
        return None;
    }
    let mut in_item = false;
    for container in node.ancestors().skip(1) {
        match container.data.borrow().value {
            NodeValue::Item(_) | NodeValue::TaskItem(_) => in_item = true,
            NodeValue::List(_) | NodeValue::Document => {}
            _ => return None,
        }
    }
    if !in_item || lines.line(last + 1).is_some_and(would_join_paragraph) {
        return None;
    }
    let crossed = crossed_lines(node, first, last);
    (first + 1..=last).find(|&line| {
        let (Some(previous), Some(text)) = (lines.line(line - 1), lines.line(line)) else {
            return false;
        };
        text.starts_with(|character: char| !matches!(character, ' ' | '\t'))
            && line_marker(text).is_none()
            && !continues_item(previous, text)
            && !crossed[line - first]
    })
}

/// For each line of a paragraph from `first` to `last`, whether an inline element runs on to it
/// from the line before: a code span, emphasis, a link or raw HTML written across a line break.
/// Splitting the paragraph there would leave the backticks or asterisks as text on both sides.
fn crossed_lines<'a>(paragraph: &'a AstNode<'a>, first: usize, last: usize) -> Vec<bool> {
    let mut crossed = vec![false; last - first + 1];
    for node in paragraph.descendants().skip(1) {
        let ast = node.data.borrow();
        if matches!(
            ast.value,
            NodeValue::Text(_) | NodeValue::SoftBreak | NodeValue::LineBreak
        ) {
            continue;
        }
        let (from, to) = (ast.sourcepos.start.line, ast.sourcepos.end.line);
        for line in from.max(first) + 1..=to.min(last) {
            crossed[line - first] = true;
        }
    }
    crossed
}

/// Whether `line`, straight after a paragraph at the left margin, would be read as more of it: a
/// line indented four columns or more, or a list marker that may not interrupt a paragraph, such
/// as the `2.` of a numbered list that carries on after the line at the margin.
fn would_join_paragraph(line: &str) -> bool {
    let rest = line.trim_start_matches([' ', '\t']);
    let indent = columns(&line[..line.len() - rest.len()]);
    !rest.is_empty()
        && (indent >= 4 || line_marker(rest).is_some_and(|marker| !marker.may_interrupt_prose()))
}

/// How long, in characters, a line of an item may be and still be taken to end where its writer
/// ended it when it stops without ending a sentence. Editors and agents that wrap a long line do it
/// at seventy to a hundred columns, or more; a short item without a full stop, `- 5 pm`, is a line
/// that ended.
const WRAP_COLUMNS: usize = 60;

/// Whether `line`, at the margin straight after `previous`, a line of an item, is more of that
/// item's text rather than a line of its own: it starts in lower case, carrying a sentence on, or
/// `previous` is as long as a wrapped line and stops without ending its sentence — unless `line`
/// ends in a colon, the heading of what comes after it.
fn continues_item(previous: &str, line: &str) -> bool {
    const OPENERS: [char; 7] = ['*', '_', '~', '"', '\'', '(', '['];
    if line
        .trim_start_matches(OPENERS)
        .chars()
        .next()
        .is_some_and(char::is_lowercase)
    {
        return true;
    }
    if unclosed(line).ends_with(':') {
        return false;
    }
    !unclosed(previous).ends_with(['.', '!', '?', ':', '\u{2026}'])
        && previous.trim_end().chars().count() >= WRAP_COLUMNS
}

/// `text` without the spaces, closing brackets, quotes and emphasis marks after its last word, so
/// that `(4061).` and `**Done.**` both end in a full stop.
fn unclosed(text: &str) -> &str {
    text.trim_end_matches([
        ' ', '\t', ')', ']', '"', '\'', '*', '_', '`', '~', '\u{201d}', '\u{2019}',
    ])
}

/// The source a tree was parsed from, and where each of its lines begins.
struct Lines<'s> {
    source: &'s str,
    starts: &'s [usize],
    /// The blank lines [`end_lists_at_unindented_lines`] put in, which the writer did not write.
    inserted: &'s [usize],
}

impl Lines<'_> {
    /// The text of 1-based `line`, without its line ending.
    fn line(&self, line: usize) -> Option<&str> {
        let start = *self.starts.get(line.checked_sub(1)?)?;
        let end = self.starts.get(line).copied().unwrap_or(self.source.len());
        Some(self.source.get(start..end)?.trim_end_matches(['\n', '\r']))
    }
}

/// What a list item's marker says, read back from the source at the item's own position.
#[derive(Clone, Copy, Debug)]
struct Marker {
    /// The marker's column, counted from the margin of the innermost quote it is in on its line
    /// ([`indent_inside_quotes`]), or from the start of the line, so that two items in one
    /// container compare however each line writes that container.
    indent: usize,
    /// The number an ordered item was written with.
    number: Option<u64>,
}

/// The [`Marker`] of a list item.
fn marker(item: &AstNode<'_>, lines: &Lines<'_>) -> Option<Marker> {
    let start = item.data.borrow().sourcepos.start;
    let line = lines.line(start.line)?;
    let at = start.column.checked_sub(1)?;
    let (before, rest) = (line.get(..at)?, line.get(at..)?);
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    Some(Marker {
        indent: indent_inside_quotes(before),
        number: rest[..digits].parse().ok(),
    })
}

/// How far in `before`, the text in front of a marker on its line, puts the marker: counted from
/// just inside the last `>` in it and the one space a quote marker takes with it, as CommonMark
/// counts a line in a quote. So `>- a` and `> - b` are two items of one list, as they are to
/// CommonMark, and not the second a step of the first. A tab after the `>` gives up one column of
/// its width to the marker, as CommonMark has it.
fn indent_inside_quotes(before: &str) -> usize {
    let all = columns(before);
    match before.rfind('>') {
        None => all,
        Some(at) => (all - columns(&before[..=at])).saturating_sub(1),
    }
}

/// The list data of a list node.
fn list_of(node: &AstNode<'_>) -> Option<NodeList> {
    match node.data.borrow().value {
        NodeValue::List(list) => Some(list),
        _ => None,
    }
}

/// Nest each list the writer indented under an item in that item, where CommonMark reads it as
/// the item's sibling. `#237 nested-lists`.
///
/// CommonMark nests a line only when it is indented as far as the item's TEXT: three columns
/// under `1. `, four under `10. `, two under `- `. People and agents writing chat indent a level
/// by two spaces, or one, whatever the marker above it — Discord's own guide says to indent a
/// list "by adding 2 spaces" — so `1. Goal\n  - step` reached the page as a numbered list and a
/// bulleted list level with it, and `1. Goal\n  1. step\n  2. step\n2. Goal` as ONE list numbered
/// one to four. Here, an item or a list whose marker is further in than the item before it goes
/// under that item:
///
/// - within one list, an item further in than the one before it — except a numbered item that
///   counts on from the one before it (`8.` then ` 9.`, a column of right-aligned numbers);
/// - a list, a fenced code block or a quote straight after a list, further in than its last item;
/// - and in Google Chat, a bulleted list on the line straight after a numbered one, whatever its
///   indent ([`Markup::bullets_under_numbered_line_nest`]), and after those steps each heading
///   line with more bullets straight under it ([`more_steps_after`]).
///
/// On the parsed tree rather than the text, so no line is ever read as something else: a code
/// block moves with the item it is in, untouched, and every list keeps the numbers CommonMark gave
/// it. A list that arrives where one of its kind already ends the item joins it when it carries
/// its numbering on, so an item's steps stay one list.
///
/// The price of not rewriting the text: a marker CommonMark has already read as part of an item's
/// text stays that text. That is a marker four or more columns past its container's margin under
/// an item whose own text starts further in still — `    1. c` under `  1. b` — or one that may
/// not interrupt the item's paragraph, `  2. b` under `- a`. A week of the messages this was
/// written for held neither shape.
fn nest_lists<'a>(
    arena: &'a Arena<'a>,
    container: &'a AstNode<'a>,
    lines: &Lines<'_>,
    markup: Markup,
) {
    let mut child = container.first_child();
    while let Some(node) = child {
        if list_of(node).is_some() {
            absorb_following_lists(node, lines, markup);
            nest_items(arena, node, lines);
        }
        child = node.next_sibling();
        if !node.data.borrow().value.contains_inlines() {
            nest_lists(arena, node, lines, markup);
        }
    }
}

/// Move into `list` each list straight after it that belongs under its last item, and each fenced
/// code block or quote the writer indented under that item — in Google Chat, each heading line
/// with more of that item's steps under it as well — and, once something has gone under it, the
/// list after that carries `list` on, so that `1. Goal`, what is under it and `2. Goal`, with no
/// blank line between them, are one numbered list again.
fn absorb_following_lists<'a>(list: &'a AstNode<'a>, lines: &Lines<'_>, markup: Markup) {
    let ordered = |list: NodeList| list.list_type == ListType::Ordered;
    let mut absorbed = false;
    while let Some(next) = list.next_sibling() {
        if markup.bullets_under_numbered_line_nest() {
            if let (Some(item), Some(more)) =
                (list.last_child(), more_steps_after(list, next, lines))
            {
                item.append(next);
                item.append(more);
                // A loose list draws each paragraph of an item as a paragraph, so the heading
                // stands clear of the bullets above it, as after the blank line it stands for.
                if let NodeValue::List(data) = &mut list.data.borrow_mut().value {
                    data.tight = false;
                }
                absorbed = true;
                continue;
            }
        }
        let next_list = list_of(next);
        // Where `next` starts: a list's first marker, or a fence's or a quote's first character.
        let start = match &next.data.borrow().value {
            NodeValue::List(_) => next.first_child(),
            NodeValue::CodeBlock(code) if code.fenced => Some(next),
            NodeValue::BlockQuote => Some(next),
            _ => None,
        };
        let (Some(last), Some(start)) = (list.last_child(), start) else {
            break;
        };
        let (Some(above), Some(below)) = (marker(last, lines), marker(start, lines)) else {
            break;
        };
        let adjacent = !blank_line_before(next, lines);
        if below.indent > above.indent
            || (markup.bullets_under_numbered_line_nest()
                && list_of(list).is_some_and(ordered)
                && next_list.is_some_and(|next| !ordered(next))
                && adjacent)
        {
            let item = innermost_open_item(last, below.indent, lines);
            if next_list.is_some() {
                attach_list(item, next);
            } else {
                item.append(next);
            }
            absorbed = true;
        } else if absorbed && adjacent && carries_on(list, next) {
            for item in next.children().collect::<Vec<_>>() {
                list.append(item);
            }
            next.detach();
            mark_task_list(list);
        } else {
            break;
        }
    }
}

/// Whether a blank line, or the start of the text, comes straight before `node`'s first line. A
/// quote's `>` alone is a blank line inside the quote. A blank line this module put in was not
/// written, so the line it is read through to is the one before it.
fn blank_line_before(node: &AstNode<'_>, lines: &Lines<'_>) -> bool {
    let mut before = node.data.borrow().sourcepos.start.line.saturating_sub(1);
    while before > 0 && lines.inserted.binary_search(&before).is_ok() {
        before -= 1;
    }
    lines
        .line(before)
        .is_none_or(|text| text.chars().all(|c| matches!(c, ' ' | '\t' | '>')))
}

/// In Google Chat, the bulleted list after `heading` when that is more steps of `list`'s last
/// numbered item: `heading` is a paragraph straight after the steps already under that item, and
/// the list comes straight after `heading` — `In review:` and the bullets under it, in the outline
/// of `#237 nested-lists`. Google Chat draws a numbered line as text, and every bullet of such an
/// outline at one indent below the text; once both lists are in the item, so does the page, where
/// with only the first in it the second was drawn a level further out than the first.
fn more_steps_after<'a>(
    list: &'a AstNode<'a>,
    heading: &'a AstNode<'a>,
    lines: &Lines<'_>,
) -> Option<&'a AstNode<'a>> {
    let of_kind = |node: &AstNode<'_>, kind: ListType| {
        list_of(node).is_some_and(|data| data.list_type == kind)
    };
    let steps_so_far = list
        .last_child()
        .and_then(|item| item.last_child())
        .is_some_and(|steps| of_kind(steps, ListType::Bullet));
    let is_heading = matches!(heading.data.borrow().value, NodeValue::Paragraph)
        && !blank_line_before(heading, lines);
    let more = heading
        .next_sibling()
        .filter(|more| of_kind(more, ListType::Bullet) && !blank_line_before(more, lines))?;
    (of_kind(list, ListType::Ordered) && steps_so_far && is_heading).then_some(more)
}

/// The item a list or an item whose marker is at `indent` belongs under: `item`, or the last item
/// of the list that ends it, and so on down, for as long as that item's marker is further out than
/// `indent`.
fn innermost_open_item<'a>(
    item: &'a AstNode<'a>,
    indent: usize,
    lines: &Lines<'_>,
) -> &'a AstNode<'a> {
    let mut target = item;
    while let Some(inner) = target
        .last_child()
        .filter(|node| list_of(node).is_some())
        .and_then(|list| list.last_child())
    {
        if !marker(inner, lines).is_some_and(|inner| inner.indent < indent) {
            break;
        }
        target = inner;
    }
    target
}

/// Put `list` at the end of `item`: as more of the list already ending it, when it carries that
/// list on, or as a list of its own.
fn attach_list<'a>(item: &'a AstNode<'a>, list: &'a AstNode<'a>) {
    if let Some(trailing) = item
        .last_child()
        .filter(|trailing| carries_on(trailing, list))
    {
        for moved in list.children().collect::<Vec<_>>() {
            trailing.append(moved);
        }
        list.detach();
        mark_task_list(trailing);
    } else {
        item.append(list);
    }
}

/// Whether `list` after `earlier` would carry it on: both bulleted, or both numbered with one
/// delimiter and `list` starting at the number after `earlier`'s last, so joining renumbers
/// nothing.
fn carries_on<'a>(earlier: &'a AstNode<'a>, list: &AstNode<'_>) -> bool {
    let (Some(a), Some(b)) = (list_of(earlier), list_of(list)) else {
        return false;
    };
    match (a.list_type, b.list_type) {
        (ListType::Bullet, ListType::Bullet) => true,
        (ListType::Ordered, ListType::Ordered) => {
            a.delimiter == b.delimiter && b.start == a.start + earlier.children().count()
        }
        _ => false,
    }
}

/// Move each item of `list` that is further in than the item before it under that item.
fn nest_items<'a>(arena: &'a Arena<'a>, list: &'a AstNode<'a>, lines: &Lines<'_>) {
    let Some(data) = list_of(list) else {
        return;
    };
    let mut previous: Option<(&AstNode<'_>, Marker)> = None;
    for item in list.children().collect::<Vec<_>>() {
        let Some(mark) = marker(item, lines) else {
            previous = None;
            continue;
        };
        if let Some((above, above_mark)) = previous {
            if mark.indent > above_mark.indent && !counts_on(above, above_mark, mark, lines) {
                adopt(
                    arena,
                    innermost_open_item(above, mark.indent, lines),
                    item,
                    data,
                    mark,
                );
                continue;
            }
        }
        previous = Some((item, mark));
    }
    mark_task_list(list);
}

/// Whether a numbered item further in than the item above it is that list's next item all the
/// same: it counts on from the item above, and not from a list of steps already under that item
/// at the step's own indent.
fn counts_on(above: &AstNode<'_>, above_mark: Marker, mark: Marker, lines: &Lines<'_>) -> bool {
    let next_after = |marker: Marker| marker.number.and_then(|number| number.checked_add(1));
    if mark.number.is_none() || mark.number != next_after(above_mark) {
        return false;
    }
    let step = above
        .last_child()
        .filter(|node| list_of(node).is_some_and(|list| list.list_type == ListType::Ordered))
        .and_then(|steps| steps.last_child())
        .and_then(|step| marker(step, lines));
    !step.is_some_and(|step| next_after(step) == mark.number && mark.indent >= step.indent)
}

/// Move `item` to the end of `target`: into the list already ending it when that is a list of
/// `like`'s kind, or into a new one like it, starting at the item's own number.
fn adopt<'a>(
    arena: &'a Arena<'a>,
    target: &'a AstNode<'a>,
    item: &'a AstNode<'a>,
    like: NodeList,
    mark: Marker,
) {
    let same_kind =
        |node: &&AstNode<'_>| list_of(node).is_some_and(|list| list.list_type == like.list_type);
    if let Some(trailing) = target.last_child().filter(same_kind) {
        trailing.append(item);
        mark_task_list(trailing);
        return;
    }
    let mut data = like;
    data.start = mark
        .number
        .and_then(|number| usize::try_from(number).ok())
        .unwrap_or(1);
    let sublist = arena.alloc(NodeValue::List(data).into());
    sublist.data.borrow_mut().sourcepos = item.data.borrow().sourcepos;
    target.append(sublist);
    sublist.append(item);
    mark_task_list(sublist);
}

/// Keep a list's task flag true to its items, so the stylesheet drops the bullets only of a list
/// that has checkboxes in it.
fn mark_task_list<'a>(list: &'a AstNode<'a>) {
    let tasks = list
        .children()
        .any(|item| matches!(item.data.borrow().value, NodeValue::TaskItem(_)));
    if let NodeValue::List(data) = &mut list.data.borrow_mut().value {
        data.is_task_list = tasks;
    }
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
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
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
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
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
    fn a_named_mention_is_drawn_as_the_name_escaped_and_any_other_by_its_id() {
        // `#318 slack-mention-names`.
        let named = |id: &str, name: &str| crate::model::MentionName {
            id: id.to_owned(),
            name: name.to_owned(),
        };
        let mentions = [
            named("U0ADA0001", "Ada Lovelace"),
            named("U0EVIL001", "<b>bold</b> & co"),
        ];
        assert_eq!(
            super::to_html_named(
                "hi <@U0ADA0001>, <@U0EVIL001> and <@U0GHOST01> in <#C0123>",
                Markup::Slack,
                &mentions
            ),
            "<p>hi <span class=\"mention\">@Ada Lovelace</span>, <span class=\"mention\">@&lt;b&gt;bold\
             &lt;/b&gt; &amp; co</span> and <span class=\"mention\">@U0GHOST01</span> in \
             <span class=\"mention\">#C0123</span></p>\n"
        );
        // The negative control: with no names, every mention is its id, as before.
        assert_eq!(
            to_html("hi <@U0ADA0001>", Markup::Slack),
            "<p>hi <span class=\"mention\">@U0ADA0001</span></p>\n"
        );
        // A name inside code is not a mention at all.
        assert_eq!(
            super::to_html_named("`<@U0ADA0001>`", Markup::Slack, &mentions),
            "<p><code>&lt;@U0ADA0001&gt;</code></p>\n"
        );
        // A heading still finds the words it restates when the line names somebody.
        let (html, heading) = super::headed_body(
            "[build-bot] <@U0ADA0001> please look",
            Markup::Slack,
            true,
            &|_| true,
            &mentions,
        );
        assert_eq!(
            html,
            "<p><span class=\"rh-src\">[build-bot] </span><span class=\"mention\">@Ada \
             Lovelace</span> please look</p>\n"
        );
        assert!(heading.is_some(), "{html}");
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
        // A paragraph may keep one class, `rh-src`, which only `mark_restated` writes (`#242
        // rich-headers`); any other class it arrives with is emptied, as a span's is.
        assert_eq!(
            cleaned,
            format!(
                "<p class=\"\">a</p>\
                 <a {LINK}>j</a><a {LINK}>d</a><a {LINK}>r</a>\
                 <a href=\"https://ok.test/\" {LINK}>ok</a><span class=\"mention\">@1</span>\
                 <input disabled=\"\">bfiveu"
            )
        );
    }

    /// The highlighter's classes, `#531 server-side-syntax-highlight`: exactly its list, only on a
    /// `span`, and nothing it might arrive with beside them.
    #[test]
    fn the_sanitizer_keeps_exactly_the_highlighters_classes_on_a_span() {
        let cleaned = SANITIZER
            .clean(
                "<pre class=\"hl-k\"><code class=\"language-rust hl-s\">\
                 <span class=\"hl-k hl-evil hl- hl-kk\" style=\"color:red\" data-x=\"1\">fn</span>\
                 <span class=\"hl-gi\">+</span><span class=\"hl-zz\">z</span></code></pre>",
            )
            .to_string();
        assert_eq!(
            cleaned,
            "<pre><code><span class=\"hl-k\">fn</span><span class=\"hl-gi\">+</span>\
             <span class=\"\">z</span></code></pre>"
        );
        for class in super::highlight::CLASSES {
            let span = format!("<span class=\"{class}\">x</span>");
            assert_eq!(SANITIZER.clean(&span).to_string(), span);
        }
        // A message cannot write one: its markup is text.
        assert_eq!(
            html("<span class=\"hl-k\">fn</span>"),
            "<p>&lt;span class=\"hl-k\"&gt;fn&lt;/span&gt;</p>\n"
        );
    }

    /// `#531 server-side-syntax-highlight`: a body is rendered once and then answered from the
    /// cache, by its whole key, and a full generation makes room by dropping the oldest.
    #[test]
    fn the_render_cache_answers_by_the_whole_key_and_drops_the_oldest() {
        use crate::model::MentionName;
        const S: super::SingleNewline = super::SingleNewline::Soft;
        let cache = super::RenderCache::default();
        let ada = [MentionName {
            id: "U1".into(),
            name: "Ada".into(),
        }];
        cache.put("<@U1> hi", Markup::Discord, S, &ada, "ada");
        assert_eq!(
            cache.get("<@U1> hi", Markup::Discord, S, &ada),
            Some("ada".into())
        );
        assert_eq!(cache.get("<@U1> hi", Markup::Discord, S, &[]), None);
        assert_eq!(cache.get("<@U1> hi", Markup::Slack, S, &ada), None);
        assert_eq!(cache.get("<@U1> hi!", Markup::Discord, S, &ada), None);
        // `markdown-tables-render`: the single-newline rule is part of the key.
        assert_eq!(
            cache.get(
                "<@U1> hi",
                Markup::Discord,
                super::SingleNewline::Hard,
                &ada
            ),
            None
        );

        // One body larger than a generation fills it at once and moves it to the old one, where
        // it is still found, and is brought back.
        let big = "x".repeat(super::RENDERED_GENERATION_BYTES);
        cache.put(&big, Markup::Discord, S, &[], "big");
        {
            let generations = cache.generations.lock().unwrap();
            assert!(generations.young.is_empty());
            assert_eq!(generations.old.values().map(Vec::len).sum::<usize>(), 2);
        }
        assert_eq!(
            cache.get("<@U1> hi", Markup::Discord, S, &ada),
            Some("ada".into())
        );
        assert_eq!(cache.generations.lock().unwrap().young.len(), 1);
        // The next full generation drops what was not read again.
        cache.put(&format!("{big}y"), Markup::Discord, S, &[], "bigger");
        assert_eq!(cache.get(&big, Markup::Discord, S, &[]), None);
        assert_eq!(
            cache.get("<@U1> hi", Markup::Discord, S, &ada),
            Some("ada".into())
        );

        // Through the module's own cache, a body renders the same the second time.
        let text = "```rust\nfn cached() {}\n```";
        assert_eq!(html(text), html(text));
    }

    #[test]
    fn google_chat_and_slack_read_one_asterisk_as_bold() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
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
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
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
    fn no_markup_labels_an_address_that_opens_another_repository() {
        // Labels read from these addresses' segments would name `widgets` or `octo`, but a browser
        // opens a page elsewhere: it resolves dot segments, and an organisation's discussion lives
        // in a repository of the organisation's choosing. Each stays as written, with no title.
        for href in [
            format!("{REPO}/blob/main/../../../../evil/repo/pull/5"),
            format!("{REPO}/blob/main/../../../../evil/repo/blob/main/README.md"),
            format!("{REPO}/releases/tag/../../../../evil/payload"),
            format!("{REPO}/blob/%2e%2e/%2e%2e/%2e%2e/evil/repo/blob/main/x.rs"),
            format!("{REPO}/tree/.."),
            format!("{REPO}/blob/main/.."),
            "https://github.com/orgs/octo/discussions/3".to_owned(),
        ] {
            let link = format!("<p>{}</p>\n", anchor(&href, &href));
            // A bare address's closing dots are punctuation to the autolinker, so an address that
            // ends in them is only ever all of a link in angle brackets.
            let bare = (!href.ends_with('.')).then(|| href.clone());
            for markup in [Markup::Discord, Markup::GoogleChat, Markup::Slack] {
                for text in bare.iter().cloned().chain([format!("<{href}>")]) {
                    assert_eq!(to_html(&text, markup), link, "{markup:?} {text:?}");
                }
            }
            for markup in [Markup::GoogleChat, Markup::Slack] {
                assert_eq!(
                    to_html(&format!("<{href}|{href}>"), markup),
                    link,
                    "{markup:?} {href}"
                );
            }
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

    // --- lists as a chat client draws them: `#237 nested-lists` ------------------------------

    const EVERY_MARKUP: [Markup; 3] = [Markup::Discord, Markup::GoogleChat, Markup::Slack];

    /// `text` rendered the same in every markup, and that rendering.
    fn in_every_markup(text: &str) -> String {
        let html = to_html(text, Markup::Discord);
        for markup in EVERY_MARKUP {
            assert_eq!(to_html(text, markup), html, "{markup:?} {text:?}");
        }
        html
    }

    #[test]
    fn steps_two_spaces_under_a_numbered_item_are_its_steps_in_every_markup() {
        // CommonMark nests only at the item's text, three columns in; this was a numbered list of
        // one, a bulleted list level with it, and a second numbered list.
        assert_eq!(
            in_every_markup("1. Goal\n  - step one\n  - step two\n2. Next goal"),
            "<ol>\n<li>Goal\n<ul>\n<li>step one</li>\n<li>step two</li>\n</ul>\n</li>\n\
             <li>Next goal</li>\n</ol>\n"
        );
        // ...and this was ONE list numbered one to four.
        assert_eq!(
            in_every_markup("1. Goal\n  1. step one\n  2. step two\n2. Next goal"),
            "<ol>\n<li>Goal\n<ol>\n<li>step one</li>\n<li>step two</li>\n</ol>\n</li>\n\
             <li>Next goal</li>\n</ol>\n"
        );
        // A blank line before the next goal is kept as the gap it is.
        assert_eq!(
            in_every_markup("1. Goal\n  - step\n\n2. Next goal"),
            "<ol>\n<li>Goal\n<ul>\n<li>step</li>\n</ul>\n</li>\n</ol>\n\
             <ol start=\"2\">\n<li>Next goal</li>\n</ol>\n"
        );
    }

    #[test]
    fn a_tab_or_a_single_space_is_a_level_too() {
        assert_eq!(
            in_every_markup("- a\n\t- b"),
            "<ul>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n</ul>\n"
        );
        assert_eq!(
            in_every_markup("1. a\n\t- b\n2. c"),
            "<ol>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n<li>c</li>\n</ol>\n"
        );
        assert_eq!(
            in_every_markup("- a\n - b\n  - c\n- d"),
            "<ul>\n<li>a\n<ul>\n<li>b\n<ul>\n<li>c</li>\n</ul>\n</li>\n</ul>\n</li>\n\
             <li>d</li>\n</ul>\n"
        );
    }

    #[test]
    fn three_levels_at_two_spaces_a_level_mixed_numbered_and_bulleted() {
        assert_eq!(
            in_every_markup("1. a\n  - b\n    - c\n      - d\n2. e"),
            "<ol>\n<li>a\n<ul>\n<li>b\n<ul>\n<li>c\n<ul>\n<li>d</li>\n</ul>\n</li>\n</ul>\n\
             </li>\n</ul>\n</li>\n<li>e</li>\n</ol>\n"
        );
        assert_eq!(
            in_every_markup("- a\n  1. b\n    - c\n- d"),
            "<ul>\n<li>a\n<ol>\n<li>b\n<ul>\n<li>c</li>\n</ul>\n</li>\n</ol>\n</li>\n\
             <li>d</li>\n</ul>\n"
        );
    }

    #[test]
    fn two_digit_items_keep_their_numbers_and_their_steps() {
        // Under `10. ` the text is four columns in, so three spaces were not enough.
        assert_eq!(
            in_every_markup("9. nine\n  - its step\n10. ten\n   - its step\n11. eleven"),
            "<ol start=\"9\">\n<li>nine\n<ul>\n<li>its step</li>\n</ul>\n</li>\n\
             <li>ten\n<ul>\n<li>its step</li>\n</ul>\n</li>\n<li>eleven</li>\n</ol>\n"
        );
    }

    #[test]
    fn numbers_that_count_on_stay_in_their_list_however_they_are_aligned() {
        // Right-aligned numbers are one list, not each one a step of the one above.
        assert_eq!(
            in_every_markup("8. eight\n 9. nine\n10. ten"),
            "<ol start=\"8\">\n<li>eight</li>\n<li>nine</li>\n<li>ten</li>\n</ol>\n"
        );
        assert_eq!(
            in_every_markup("1. one\n  2. two"),
            "<ol>\n<li>one</li>\n<li>two</li>\n</ol>\n"
        );
    }

    #[test]
    fn code_moves_with_its_item_and_is_never_read_as_anything_else() {
        assert_eq!(
            in_every_markup("1. Run it:\n  ```\n  - not a list\n  ```\n  - then this\n2. Next"),
            "<ol>\n<li>Run it:\n<pre><code>- not a list\n</code></pre>\n<ul>\n\
             <li>then this</li>\n</ul>\n</li>\n<li>Next</li>\n</ol>\n"
        );
        // An indented code block inside a step goes where the step goes, and stays code.
        let html = in_every_markup("1. Goal\n  - step\n\n        cargo test\n2. Next");
        assert!(
            html.starts_with("<ol>\n<li>Goal\n<ul>\n<li>\n<p>step</p>\n<pre><code>cargo test\n"),
            "{html}"
        );
        // Nothing inside a fence is moved or separated, in any markup.
        assert_eq!(
            in_every_markup("- a\n```\n- b\nText\n  - c\n```"),
            "<ul>\n<li>a</li>\n</ul>\n<pre><code>- b\nText\n  - c\n</code></pre>\n"
        );
    }

    #[test]
    fn a_list_in_a_quote_nests_as_written() {
        assert_eq!(
            in_every_markup("> 1. a\n>   - b"),
            "<blockquote>\n<ol>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n</ol>\n</blockquote>\n"
        );
    }

    #[test]
    fn a_nested_task_list_keeps_its_boxes_and_its_bullets_keep_theirs() {
        assert_eq!(
            in_every_markup("1. Plan\n  - [ ] write\n  - [x] test\n2. Ship"),
            "<ol>\n<li>Plan\n<ul class=\"contains-task-list\">\n\
             <li class=\"task-list-item\"><input type=\"checkbox\" \
             class=\"task-list-item-checkbox\" disabled=\"\"> write</li>\n\
             <li class=\"task-list-item\"><input type=\"checkbox\" \
             class=\"task-list-item-checkbox\" checked=\"\" disabled=\"\"> test</li>\n\
             </ul>\n</li>\n<li>Ship</li>\n</ol>\n"
        );
    }

    #[test]
    fn a_line_at_the_margin_after_a_list_is_a_new_paragraph_in_google_chat_and_slack() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        // The example a received message is given in Google Chat's own API guide: a list, then a
        // sentence, drawn as a list and then a sentence.
        let text = "I can meet there at:\n- Noon\n- 3 pm\n- 5 pm\nWhat time works for you?";
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html(text, markup),
                "<p>I can meet there at:</p>\n<ul>\n<li>Noon</li>\n<li>3 pm</li>\n\
                 <li>5 pm</li>\n</ul>\n<p>What time works for you?</p>\n",
                "{markup:?}"
            );
            // Under a nested item too, and with Windows line endings.
            assert_eq!(
                to_html("- a\n  - b\nAfter", markup),
                "<ul>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n</ul>\n<p>After</p>\n",
                "{markup:?}"
            );
            assert_eq!(
                to_html("- a\r\nAfter", markup),
                "<ul>\n<li>a</li>\n</ul>\n<p>After</p>\n",
                "{markup:?}"
            );
        }
        // Discord keeps CommonMark's lazy line, its client being undocumented either way.
        assert_eq!(
            html(text),
            "<p>I can meet there at:</p>\n<ul>\n<li>Noon</li>\n<li>3 pm</li>\n\
             <li>5 pm<br>\nWhat time works for you?</li>\n</ul>\n"
        );
    }

    #[test]
    fn only_prose_at_the_margin_ends_a_list() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        for text in [
            // An indented line is more of the item, as its writer indented it.
            "- a\n  and more of a",
            "1. a\n  more of a",
            // A marker is the next item, or the first of a new list.
            "- a\n2. b",
            "- a\n- b",
            // After a blank line CommonMark ends the list itself.
            "- a\n\nAfter",
            // No list at all: a line that only looks like an item is prose to CommonMark.
            "Text\n2. not an item\nmore text",
            "plain\nlines",
        ] {
            in_every_markup(text);
        }
        assert_eq!(
            in_every_markup("- a\n2. b"),
            "<ul>\n<li>a</li>\n</ul>\n<ol start=\"2\">\n<li>b</li>\n</ol>\n"
        );
        assert_eq!(
            in_every_markup("Text\n2. not an item\nmore text"),
            "<p>Text<br>\n2. not an item<br>\nmore text</p>\n"
        );
    }

    #[test]
    fn a_hard_wrapped_item_stays_one_item_in_every_markup() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        // The rest of a sentence an editor wrapped to the margin is more of the item, and the
        // numbered list goes on. Set apart, `2.` and `3.` could not start a list after the
        // paragraph and were drawn as its text.
        assert_eq!(
            in_every_markup("1. first item that wraps\nonto a second line\n2. second\n3. third"),
            "<ol>\n<li>first item that wraps<br>\nonto a second line</li>\n<li>second</li>\n\
             <li>third</li>\n</ol>\n"
        );
        // A code span across the wrap stays one code span, not two backticks as text.
        assert_eq!(
            in_every_markup("- run `make\ncheck` now\n- next"),
            "<ul>\n<li>run <code>make check</code> now</li>\n<li>next</li>\n</ul>\n"
        );
        // A line in lower case carries the sentence above it on, however short that line is.
        assert_eq!(
            in_every_markup("- the first half,\nand the second half.\n- next"),
            "<ul>\n<li>the first half,<br>\nand the second half.</li>\n<li>next</li>\n</ul>\n"
        );
        // A line as long as editors wrap that stops mid-sentence was wrapped, whatever the next
        // line starts with.
        in_every_markup(
            "- the cache now keys every build by its target and its toolchain, so\n\
             CI never shares one between them\n- next",
        );
    }

    #[test]
    fn a_line_of_its_own_is_set_apart_only_where_nothing_else_changes() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        // Each of these lines reads as a line of its own, and stays more of its item.
        for text in [
            // The numbered list carries on after it, and `2.` cannot interrupt a paragraph.
            "1. First step.\nThen check it.\n2. Second step.",
            "- a\n  1. First.\nThen.\n  2. Second.",
            // A link, a code span or emphasis runs on to it from the line above.
            "- Read [the guide.\nAll of it](https://example.test/) first.\n- next",
            "- Run `make check.\nThen` again.",
            "- This is **done.\nReally** done.",
            // The line after it would be read as more of it: here a tag, which starts a block
            // after a list but not after a paragraph. Caught by checking the new tree, which
            // keeps every block but paragraphs and lists where it was.
            "- a.\nB.\n<custom-tag>",
            // A fenced block in the item would be indented code at the margin, its fences code.
            "- a.\nB.\n\n    ```\n    x\n    ```",
            // The backticks on it would pair up differently in a paragraph of its own: CommonMark's
            // reader leaves `` ``b`` `` as text after the lone backtick and `` ``a`` `` above it,
            // and the characters a message shows are never changed by setting a line apart.
            "- Use ` and ``a``.\nThen ``b`` too.",
        ] {
            in_every_markup(text);
        }
        // A line that ends in a colon heads what follows it, after however long a line.
        let text = "- 4127 (changes requested in review, see the thread for the details)\n\
                    In review, each with fixes in flight:\n- 4130";
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html(text, markup),
                "<ul>\n<li>4127 (changes requested in review, see the thread for the details)</li>\n\
                 </ul>\n<p>In review, each with fixes in flight:</p>\n<ul>\n<li>4130</li>\n</ul>\n",
                "{markup:?}"
            );
        }
        // A line kept in its numbered item costs no other line its own paragraph: the check on
        // the new tree is the last resort, and it refuses every line of the message at once.
        let text = "1. First step.\nThen check it.\n2. Second step.\n\n- a\nIn review:\n- b";
        for markup in [Markup::GoogleChat, Markup::Slack] {
            assert_eq!(
                to_html(text, markup),
                "<ol>\n<li>First step.<br>\nThen check it.</li>\n<li>Second step.</li>\n</ol>\n\
                 <ul>\n<li>a</li>\n</ul>\n<p>In review:</p>\n<ul>\n<li>b</li>\n</ul>\n",
                "{markup:?}"
            );
        }
    }

    #[test]
    fn no_line_inside_code_is_set_apart_or_moved() {
        for text in [
            // Four columns in, ``` is indented code, and the fence the next ``` opens holds the
            // rest of the message.
            "    ```\n- a\n```\n- b\nfoo\n```",
            // A fence in an item ends with the item at a line further out than the item's text,
            // and the ``` after it opens another.
            "- a\n  ```\nx\n```\n- b\nfoo\n```",
            "- a\n```\n- b\nfoo\n```",
        ] {
            let html = in_every_markup(text);
            assert!(
                html.contains("<pre><code>- b\nfoo\n</code></pre>"),
                "{text:?}: {html}"
            );
        }
    }

    #[test]
    fn items_in_a_quote_compare_from_inside_the_quote() {
        // A quote's `>` takes one space with it, so these are two items of one list, as
        // CommonMark reads them, and not the second a step of the first.
        assert_eq!(
            in_every_markup(">- a\n> - b"),
            "<blockquote>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>\n</blockquote>\n"
        );
        // Further in than that is a step, however the quote is written.
        assert_eq!(
            in_every_markup(">- a\n>  - b"),
            "<blockquote>\n<ul>\n<li>a\n<ul>\n<li>b</li>\n</ul>\n</li>\n</ul>\n</blockquote>\n"
        );
    }

    #[test]
    fn google_chat_draws_bullets_straight_under_a_numbered_line_as_its_steps() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        // The shape of the message in `#237 nested-lists`, with neutral words: numbered goals,
        // each with its bullets at the margin on the next line, a heading line after them, and
        // the goals apart by blank lines. Google Chat has no numbered lists, so `1.` is a line of
        // text there and the bullets a list indented under it. The heading line and the bullets
        // under it are more of the first goal: Google Chat draws all its bullets at one indent,
        // and so does the page once both lists are in the goal.
        let text = "Goals:\n\n\
                    1. Faster builds. Most of the work went here:\n\
                    - cache keyed by target\n\
                    - warm machines kept warm\n\
                    Still open:\n\
                    - the flaky test\n\
                    \n\
                    2. Better docs: no work.";
        assert_eq!(
            to_html(text, Markup::GoogleChat),
            "<p>Goals:</p>\n<ol>\n<li>\n<p>Faster builds. Most of the work went here:</p>\n<ul>\n\
             <li>cache keyed by target</li>\n<li>warm machines kept warm</li>\n</ul>\n\
             <p>Still open:</p>\n<ul>\n<li>the flaky test</li>\n</ul>\n</li>\n</ol>\n\
             <ol start=\"2\">\n<li>Better docs: no work.</li>\n</ol>\n"
        );
        // Each further heading and its bullets join too, and a goal after them with no blank line
        // carries the list on. A line with no bullets after it heads no steps, and stays at the
        // margin.
        assert_eq!(
            to_html(
                "1. a\n- x\nDone:\n- y\nOpen:\n- z\n2. b\n- w\nThanks.",
                Markup::GoogleChat
            ),
            "<ol>\n<li>\n<p>a</p>\n<ul>\n<li>x</li>\n</ul>\n<p>Done:</p>\n<ul>\n<li>y</li>\n</ul>\n\
             <p>Open:</p>\n<ul>\n<li>z</li>\n</ul>\n</li>\n<li>\n<p>b</p>\n<ul>\n<li>w</li>\n</ul>\n\
             </li>\n</ol>\n<p>Thanks.</p>\n"
        );
        // A blank line the writer put before the heading keeps it, and its bullets, apart.
        assert_eq!(
            to_html("1. a\n- x\n\nOpen:\n- z", Markup::GoogleChat),
            "<ol>\n<li>a\n<ul>\n<li>x</li>\n</ul>\n</li>\n</ol>\n<p>Open:</p>\n<ul>\n\
             <li>z</li>\n</ul>\n"
        );
        // Slack draws both as lines at one margin, so only the heading line is set apart.
        assert_eq!(
            to_html(text, Markup::Slack),
            "<p>Goals:</p>\n<ol>\n<li>Faster builds. Most of the work went here:</li>\n</ol>\n\
             <ul>\n<li>cache keyed by target</li>\n<li>warm machines kept warm</li>\n</ul>\n\
             <p>Still open:</p>\n<ul>\n<li>the flaky test</li>\n</ul>\n\
             <ol start=\"2\">\n<li>Better docs: no work.</li>\n</ol>\n"
        );
        // Discord draws both as lists, at one margin, and keeps CommonMark's lazy line.
        assert_eq!(
            to_html(text, Markup::Discord),
            "<p>Goals:</p>\n<ol>\n<li>Faster builds. Most of the work went here:</li>\n</ol>\n\
             <ul>\n<li>cache keyed by target</li>\n<li>warm machines kept warm<br>\nStill open:</li>\n\
             <li>the flaky test</li>\n</ul>\n\
             <ol start=\"2\">\n<li>Better docs: no work.</li>\n</ol>\n"
        );
        // With no blank lines, the goals are one list, each with its steps.
        assert_eq!(
            to_html("1. a\n- x\n2. b\n- y\n3. c", Markup::GoogleChat),
            "<ol>\n<li>a\n<ul>\n<li>x</li>\n</ul>\n</li>\n<li>b\n<ul>\n<li>y</li>\n</ul>\n</li>\n\
             <li>c</li>\n</ol>\n"
        );
        // A blank line between the numbered line and the bullets keeps them apart.
        assert_eq!(
            to_html("1. a\n\n- x", Markup::GoogleChat),
            "<ol>\n<li>a</li>\n</ol>\n<ul>\n<li>x</li>\n</ul>\n"
        );
    }

    #[test]
    fn nesting_that_would_go_past_the_depth_bound_is_drawn_as_text() {
        // Every line further in than the one before, one column and then two: CommonMark nests
        // every other line, the page every line, so the parsed tree is inside the bound and the
        // nested one is not.
        let stairs = |steps: usize| -> String {
            (0..steps)
                .map(|step| format!("{}- step {step}\n", " ".repeat(step / 2 * 3 + step % 2)))
                .collect()
        };
        let arena = comrak::Arena::new();
        let parsed = comrak::parse_document(
            &arena,
            &stairs(24),
            &super::options(true, super::SingleNewline::Hard),
        );
        assert!(
            !super::deeper_than(parsed, MAX_DEPTH),
            "the parse alone is within the bound"
        );
        assert_eq!(body_html(&stairs(24), Markup::Discord), "");
        assert_eq!(
            to_html(&stairs(6), Markup::Discord).matches("<ul>").count(),
            6
        );
    }

    // ---- `#242 rich-headers`: the words a heading restates, marked --------------------------

    fn headed(text: &str, markup: Markup) -> (String, Option<crate::model::Heading>) {
        super::headed_body(text, markup, true, &|_| true, &[])
    }

    /// `html` with the restated words unmarked: what the page draws with rich headers off.
    fn unmarked(html: &str) -> String {
        let open = r#"<span class="rh-src">"#;
        let mut out = html.replacen(r#"<p class="rh-src">"#, "<p>", 1);
        if let Some(at) = out.find(open) {
            out.replace_range(at..at + open.len(), "");
            let close = out[at..].find("</span>").expect("the mark closes") + at;
            out.replace_range(close..close + "</span>".len(), "");
        }
        out
    }

    #[test]
    fn a_rendered_body_marks_the_words_its_heading_restates_and_changes_nothing_else() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        for (text, markup, expected) in [
            // The rest of the line goes on: the words and the space after them.
            (
                "[build-bot] **Status, 9:05 AM ET.** All `green`.\nSecond line",
                Markup::Discord,
                "<p><span class=\"rh-src\">[build-bot] <strong>Status, 9:05 AM ET.</strong> \
                 </span>All <code>green</code>.<br>\nSecond line</p>\n",
            ),
            // The line is the whole paragraph: the paragraph is marked.
            (
                "[build-bot] Status, 9:05 AM ET:\n- one\n- two",
                Markup::Discord,
                "<p class=\"rh-src\">[build-bot] Status, 9:05 AM ET:</p>\n\
                 <ul>\n<li>one</li>\n<li>two</li>\n</ul>\n",
            ),
            // The line is used up and the paragraph goes on: the line break is marked with it.
            (
                "[build-bot] Status:\n**bold** next",
                Markup::Discord,
                "<p><span class=\"rh-src\">[build-bot] Status:<br>\n</span>\
                 <strong>bold</strong> next</p>\n",
            ),
            // The same over CRLF, whose carriage return the cut counts.
            (
                "[build-bot] Status, 9:05 AM ET:\r\n**bold** next",
                Markup::Discord,
                "<p><span class=\"rh-src\">[build-bot] Status, 9:05 AM ET:<br>\n</span>\
                 <strong>bold</strong> next</p>\n",
            ),
            // A tag alone: only the tag is marked.
            (
                "[build-bot] Fixed the `parser` crash: details",
                Markup::Discord,
                "<p><span class=\"rh-src\">[build-bot] </span>Fixed the <code>parser</code> \
                 crash: details</p>\n",
            ),
            // A link after the words: the sanitizer's second pass leaves it as the first wrote it.
            (
                "[build-bot] Status: see https://example.com/run",
                Markup::Discord,
                "<p><span class=\"rh-src\">[build-bot] Status: </span>see <a \
                 href=\"https://example.com/run\" target=\"_blank\" \
                 rel=\"noopener noreferrer nofollow\">https://example.com/run</a></p>\n",
            ),
            // One asterisk is bold where the provider says so, and is unwrapped from the title.
            (
                "(ops2) *Midday summary, 15:05Z.* Nothing is stuck.",
                Markup::GoogleChat,
                "<p><span class=\"rh-src\">(ops2) <strong>Midday summary, 15:05Z.</strong> \
                 </span>Nothing is stuck.</p>\n",
            ),
        ] {
            let (html, heading) = headed(text, markup);
            assert!(heading.is_some(), "{text:?}");
            assert_eq!(html, expected, "{text:?}");
            assert_eq!(unmarked(&html), to_html(text, markup), "{text:?}");
        }
    }

    #[test]
    fn a_plain_body_stays_empty_and_carries_its_heading() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        let text = "[build-bot] Status, 9:05 AM ET. All green.\nNothing else.";
        let (html, heading) = headed(text, Markup::Discord);
        assert_eq!(html, "");
        let heading = heading.expect("a heading");
        assert_eq!(heading.tag.as_deref(), Some("build-bot"));
        assert_eq!(heading.time.as_deref(), Some("9:05 AM ET"));
        assert_eq!(heading.title.as_deref(), Some("Status"));
        assert_eq!(heading.cut, 32);
        assert_eq!(&text[..32], "[build-bot] Status, 9:05 AM ET. ");
    }

    #[test]
    fn a_body_that_does_not_begin_with_the_words_has_no_heading() {
        // The tag is a link here, because a reference definition later in the message names it:
        // the words alone render as text, the body does not begin with them, and the message is
        // drawn as it always was.
        let text = "[build-bot] Status: body\n\n[build-bot]: https://example.com";
        let (html, heading) = headed(text, Markup::Discord);
        assert_eq!(heading, None);
        assert_eq!(html, to_html(text, Markup::Discord));
        assert!(!html.contains("rh-src"));
        // No heading at all: exactly `body_html`.
        for text in [
            "plain words",
            "**bold** words",
            "Backup check, 10:52 PM ET. All good.",
        ] {
            assert_eq!(
                headed(text, Markup::Discord),
                (body_html(text, Markup::Discord), None)
            );
        }
    }

    #[test]
    fn message_text_cannot_write_the_mark() {
        for text in [
            "[build-bot] Status: <span class=\"rh-src\">hidden</span> and **more**",
            "<p class=\"rh-src\">hidden</p>\n\n**more**",
            "plain <span class=\"rh-src\">hidden</span> **more**",
        ] {
            let (html, _) = headed(text, Markup::Discord);
            let marks = html.matches("<span class=\"rh-src\">").count()
                + html.matches("<p class=\"rh-src\">").count();
            let expected = usize::from(text.starts_with('['));
            assert_eq!(marks, expected, "{text:?}: {html}");
        }
    }
}

#[cfg(test)]
mod search_text_tests {
    use super::{Markup, SearchText};
    use crate::cache::search_text::build;
    use crate::cache::SearchTextSource as _;

    #[test]
    fn the_index_keeps_link_labels_addresses_and_words_emphasis_split() {
        let pull = "https://github.com/octo/widgets/pull/12";
        let body = format!("Fixed **wid**gets in {pull} and [the notes](https://example.test/notes?a=1&b=2) _today_");
        let drawn = SearchText::drawn(&body, Markup::Discord, false);
        assert_eq!(
            drawn.text, "Fixed widgets in widgets#12 and the notes today\n",
            "{drawn:?}"
        );
        assert_eq!(
            drawn.hrefs,
            [pull, "https://example.test/notes?a=1&b=2"],
            "addresses are decoded, in order"
        );
        let text = build("Ada", &body, &drawn);
        for needle in [
            "widgets",
            "widgets#12",
            "the notes today",
            pull,
            "https://example.test/notes?a=1&b=2",
            "**wid**gets",
        ] {
            assert!(text.contains(needle), "{needle:?} missing from {text:?}");
        }
    }

    #[test]
    fn each_provider_is_drawn_in_its_own_markup_and_a_plain_body_is_its_text() {
        let config = crate::testing::config();
        let source = SearchText::from_config(&config);
        let key = &config.providers[0].key;
        let mut message = crate::cache::fixtures::message("1", "u", "a *b* <&> c", 5);
        let discord = source.rendered(key, &message);
        assert_eq!(discord.text, "a b <&> c\n");
        assert!(discord.hrefs.is_empty());
        assert_eq!(
            SearchText::drawn("a *b* c <https://x.test/|label>", Markup::Slack, false),
            crate::cache::Rendered {
                text: "a b c label\n".to_owned(),
                hrefs: vec!["https://x.test/".to_owned()],
            }
        );
        message.content = "just words".to_owned();
        assert_eq!(source.rendered(key, &message).text, "just words");
        assert_eq!(
            source
                .rendered("a-provider-nobody-configured", &message)
                .text,
            "just words"
        );
        assert_eq!(source.version(), SearchText::VERSION);
    }

    /// Bodies of every shape the renderer treats differently, in every markup.
    const CORPUS: &[(&str, bool)] = &[
        ("plain words, nothing else", false),
        ("**bold**, _italic_, *star* and ~~struck~~", false),
        (
            "a bare link https://github.com/octo/widgets/issues/7 mid-sentence",
            false,
        ),
        (
            "[a label](https://example.test/a?b=1&c=<2>) and <https://example.test/x|angled>",
            false,
        ),
        ("`code with **stars**` and\n```\nblock <b>&amp;\n```", false),
        ("- one\n- two\n  - nested\n1. Goal\n- step", false),
        ("> quoted\n\n| a | b |\n|---|---|\n| 1 | 2 |", false),
        ("<@123456789> said <#987654321> & left", false),
        ("[build-bot] Status: green. More words follow here.", true),
        ("Release notes 10:52 PM ET\nsecond line", false),
        ("__init__.py and snake_case_name", false),
        ("emoji 🎉 and non-breaking\u{a0}space", false),
    ];

    /// Each `SearchText::VERSION` there has ever been, and the digest of the corpus under it. A
    /// change to what the index is given changes the digest: bump the version (a cache built under
    /// the old one then rebuilds its index text) and add a line, rather than editing one.
    const PINNED: &[(&str, &str)] = &[
        (
            "render-1",
            "09987ed5048b1a31f6d567320deaf9fb276392766933c7a85a23eaef6bfe331c",
        ),
        (
            "render-2",
            "30ddda09e4cbb6fdbe5b174699e7015aebc3474454a1871f1db9f5be4189eb34",
        ),
    ];

    fn corpus_digest() -> String {
        let mut all = String::new();
        for markup in [Markup::Discord, Markup::GoogleChat, Markup::Slack] {
            for (body, bot) in CORPUS {
                let drawn = SearchText::drawn(body, markup, *bot);
                all.push_str(&drawn.text);
                all.push('\u{0}');
                all.push_str(&drawn.hrefs.join("\u{1}"));
                all.push('\u{2}');
            }
        }
        let digest = ring::digest::digest(&ring::digest::SHA256, all.as_bytes());
        digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn what_the_index_is_given_changes_only_with_its_version() {
        let digest = corpus_digest();
        let versions: std::collections::HashSet<&str> = PINNED.iter().map(|(v, _)| *v).collect();
        assert_eq!(versions.len(), PINNED.len(), "a version is pinned twice");
        let pinned = PINNED
            .iter()
            .find(|(version, _)| *version == SearchText::VERSION)
            .map(|(_, digest)| *digest);
        assert_eq!(
            pinned,
            Some(digest.as_str()),
            "the index text of the corpus is now {digest} under {}: if the rendering changed on \
             purpose, bump SearchText::VERSION and pin the new digest under it",
            SearchText::VERSION
        );
    }
}

#[cfg(test)]
mod read_aloud_blocks {
    //! `#376 read-aloud-blocks`: the blocks a body is read in are the blocks the page draws.

    use super::*;

    /// The message tests/read_aloud_blocks_browser.py walks and the page suite draws, its rendering
    /// and its blocks, shared by all three.
    const FIXTURE: &str = include_str!("../tests/fixtures/read_aloud_blocks.json");

    fn fixture(key: &str) -> serde_json::Value {
        let fixture: serde_json::Value =
            serde_json::from_str(FIXTURE).expect("the fixture is JSON");
        fixture[key].clone()
    }

    fn kinds(blocks: &[SourceBlock]) -> Vec<BlockKind> {
        blocks.iter().map(|block| block.kind).collect()
    }

    #[test]
    fn a_long_message_is_read_in_the_blocks_it_is_drawn_as() {
        let text = fixture("content");
        let text = text.as_str().expect("content");
        let html = fixture("html");
        assert_eq!(
            body_html(text, Markup::Discord),
            html.as_str().expect("html"),
            "the browser walk and the page suite draw a body this server does not render"
        );
        let blocks = source_blocks(text, Markup::Discord, &[]);
        assert_eq!(
            serde_json::to_value(kinds(&blocks)).expect("kinds"),
            fixture("blocks")
        );
        assert_eq!(
            html_block_kinds(html.as_str().expect("html")),
            kinds(&blocks)
        );
        let sources: Vec<&str> = blocks.iter().map(|block| block.source.trim()).collect();
        assert_eq!(
            sources,
            [
                "Where the overnight run got to. Three things finished. One is still going.",
                // An item's own words, without the bullet and without the list nested in it.
                "the retry budget landed",
                "the cache rewrite is a draft",
                "waiting on review",
                "> nothing failed outright",
                "```\nqueued 41m\n```",
                "That is everything.",
            ]
        );
    }

    #[test]
    fn a_plain_body_is_its_paragraphs_as_the_page_draws_them_from_the_text() {
        // Pins line breaks as `<br>`: the hard single-newline rule.
        let _rule = super::SingleNewlineGuard::set(super::SingleNewline::Hard);
        let text = "one\ntwo\n\n  \nthree";
        assert_eq!(body_html(text, Markup::Discord), "");
        let blocks = source_blocks(text, Markup::Discord, &[]);
        assert_eq!(
            blocks
                .iter()
                .map(|block| block.source.as_str())
                .collect::<Vec<_>>(),
            ["one\ntwo", "three"]
        );
        assert_eq!(
            html_block_kinds(&plain_html(text)),
            [BlockKind::Paragraph, BlockKind::Paragraph]
        );
    }

    #[test]
    fn headings_numbered_items_tables_and_tasks_are_blocks_and_a_rule_is_not() {
        use BlockKind::{Heading, Item, Paragraph, Table};
        let text = "# Title\nbody line\n\n1. first\n2. second\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [ ] open task\n- [x] done task\n\n---\n\nend";
        let blocks = source_blocks(text, Markup::Discord, &[]);
        assert_eq!(
            kinds(&blocks),
            [Heading, Paragraph, Item, Item, Table, Item, Item, Paragraph]
        );
        assert_eq!(
            blocks[2].source.trim(),
            "1. first",
            "a number is part of what a list says"
        );
        assert_eq!(blocks[5].source.trim(), "To do: open task");
        assert_eq!(blocks[6].source.trim(), "Done: done task");
        assert_eq!(
            html_block_kinds(&to_html(text, Markup::Discord)),
            kinds(&blocks)
        );
    }

    #[test]
    fn an_items_words_after_its_nested_list_are_its_own() {
        let text = "- a\n  - b\n\n  tail of a\n- c";
        let blocks = source_blocks(text, Markup::Discord, &[]);
        assert_eq!(
            kinds(&blocks),
            [BlockKind::Item, BlockKind::Item, BlockKind::Item]
        );
        assert!(
            blocks[0].source.contains('a') && blocks[0].source.contains("tail of a"),
            "{blocks:?}"
        );
        assert!(!blocks[0].source.contains('b'), "{blocks:?}");
        assert_eq!(blocks[1].source.trim(), "b");
        assert_eq!(blocks[2].source.trim(), "c");
        assert_eq!(
            html_block_kinds(&to_html(text, Markup::Discord)),
            kinds(&blocks)
        );
    }

    #[test]
    fn a_google_chat_outline_reads_the_lines_moved_into_an_item() {
        // `#237 nested-lists`: a line the service shows under an item is nested into it.
        let text = "1. First step\nIn review:\n- one\n- two\n2. Second step";
        let blocks = source_blocks(text, Markup::GoogleChat, &[]);
        let all: String = blocks.iter().map(|block| block.source.clone()).collect();
        for words in ["First step", "In review", "one", "two", "Second step"] {
            assert!(all.contains(words), "{words:?} is never read: {blocks:?}");
        }
        assert_eq!(
            html_block_kinds(&to_html(text, Markup::GoogleChat)),
            kinds(&blocks)
        );
    }

    #[test]
    fn a_loose_item_is_one_block_with_all_its_paragraphs() {
        let text = "- a\n\n  more of a\n- b";
        let blocks = source_blocks(text, Markup::Discord, &[]);
        assert_eq!(kinds(&blocks), [BlockKind::Item, BlockKind::Item]);
        assert!(blocks[0].source.contains("more of a"), "{blocks:?}");
    }

    #[test]
    fn a_body_past_a_rendering_bound_is_read_as_its_plain_paragraphs() {
        let text = format!("{}\n\n- x", "w".repeat(MAX_RENDERED_BYTES));
        let blocks = source_blocks(&text, Markup::Discord, &[]);
        assert_eq!(kinds(&blocks), [BlockKind::Paragraph, BlockKind::Paragraph]);
        assert_eq!(blocks[1].source, "- x");
    }

    /// `#486 read-aloud-pieces`: the cases web/voice.js `readPieces` is held to as well.
    #[test]
    fn a_block_is_read_in_the_pieces_the_page_cuts_it_into() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/read_aloud_pieces.json"))
                .expect("the fixture is JSON");
        for case in fixture["cases"].as_array().expect("cases") {
            let text = case["text"].as_str().expect("text");
            let wanted: Vec<&str> = case["pieces"]
                .as_array()
                .expect("pieces")
                .iter()
                .map(|piece| piece.as_str().expect("a piece"))
                .collect();
            assert_eq!(read_pieces(text), wanted, "{}", case["name"]);
        }
    }

    #[test]
    fn no_piece_is_longer_than_the_most_words_unless_it_is_one_sentence_cut_evenly() {
        let sentence = |words: usize| format!("{}.", vec!["w"; words].join(" "));
        let text = [sentence(30), sentence(31), sentence(59), sentence(1)].join(" ");
        let pieces = read_pieces(&text);
        let counts: Vec<usize> = pieces
            .iter()
            .map(|p| p.split_whitespace().count())
            .collect();
        assert_eq!(counts, [30, 31, 60], "{pieces:?}");
        let counts: Vec<usize> = read_pieces(&vec!["w"; 121].join(" "))
            .iter()
            .map(|p| p.split_whitespace().count())
            .collect();
        assert_eq!(counts, [41, 40, 40]);
        assert!(read_pieces("  \n \n").is_empty());
    }

    /// From review: the source holds markup the page draws as something else, and a piece never
    /// ends inside it; a quote's typed bullets are pieces, as the page sees them.
    #[test]
    fn a_piece_never_ends_inside_a_code_span_or_a_link() {
        let long = |w: &str| vec![w; 35].join(" ");
        let text = format!(
            "{} see [Mr. Smith](https://example.com/a. b) and `x. y` then. {}.",
            long("a"),
            long("b")
        );
        let pieces = read_pieces(&text);
        assert_eq!(pieces.len(), 2, "{pieces:?}");
        assert!(pieces[0].ends_with("then."), "{pieces:?}");
        assert_eq!(read_pieces("> • a\n> • b").len(), 2);
    }

    #[test]
    fn a_piece_says_its_words_without_the_bullet_it_was_typed_with() {
        assert_eq!(piece_source("  • First load"), "First load");
        assert_eq!(piece_source("▸ x"), "x");
        assert_eq!(piece_source("No bullet"), "No bullet");
    }

    #[test]
    fn the_html_walk_follows_the_page_rule() {
        use BlockKind::{Code, Item, Paragraph, Quote, Table};
        // Nothing inside a quote, a pre or a table is another block; an item's nested lists are.
        let html = "<blockquote>\n<p>a</p>\n<ul>\n<li>in a quote</li>\n</ul>\n</blockquote>\n\
                    <ul>\n<li>\n<p>loose</p>\n<pre><code>x</code></pre>\n<ol>\n<li>nested</li>\n</ol>\n</li>\n</ul>\n\
                    <table>\n<tbody>\n<tr>\n<td><p>cell</p></td>\n</tr>\n</tbody>\n</table>\n\
                    <hr>\n<pre><code>&lt;p&gt;not a tag</code></pre>\n<p><a href=\"https://example.com/?a=1>2\" title=\"x\">link</a></p>\n";
        assert_eq!(
            html_block_kinds(html),
            [Quote, Item, Item, Table, Code, Paragraph]
        );
    }
}

#[cfg(test)]
mod single_newline_tests {
    //! `markdown-tables-render`: the soft single-newline rule, the `•` lines it makes items, and
    //! the hard rule kept beside it.

    use super::{
        body_html, headed_body, source_blocks, to_html, to_html_as, Markup, SingleNewline,
        SingleNewlineGuard,
    };

    fn soft(text: &str, markup: Markup) -> String {
        to_html_as(text, markup, SingleNewline::Soft)
    }

    #[test]
    fn soft_is_the_default_and_joins_the_lines_of_a_paragraph() {
        assert_eq!(super::single_newline(), SingleNewline::Soft);
        assert_eq!(
            to_html("one\ntwo\n\nthree", Markup::Discord),
            "<p>one\ntwo</p>\n<p>three</p>\n"
        );
        assert_eq!(
            to_html_as("one\ntwo\n\nthree", Markup::Discord, SingleNewline::Hard),
            "<p>one<br>\ntwo</p>\n<p>three</p>\n"
        );
    }

    #[test]
    fn a_hard_wrapped_paragraph_is_sent_as_html_and_a_one_line_message_still_is_not() {
        let wrapped = "Most gaps are named refusals, and the two runs agreed on two hundred\n\
                       more cells, but the check that they were really intercepted failed.";
        assert_eq!(
            body_html(wrapped, Markup::Slack),
            format!("<p>{wrapped}</p>\n"),
            "joined, so the screen wraps it"
        );
        assert_eq!(
            body_html("Sounds good, I'll take a look", Markup::Slack),
            ""
        );
        let _rule = SingleNewlineGuard::set(SingleNewline::Hard);
        assert_eq!(
            body_html(wrapped, Markup::Slack),
            "",
            "the page draws its lines itself"
        );
    }

    #[test]
    fn typed_bullet_lines_stay_lines_as_list_items() {
        let text = "Published:\n• one\n• two\n    ◦ two, part a\n• three\nAfter the list.";
        assert_eq!(
            soft(text, Markup::Slack),
            "<p>Published:</p>\n<ul>\n<li>one</li>\n<li>two\n<ul>\n<li>two, part a</li>\n</ul>\n\
             </li>\n<li>three</li>\n</ul>\n<p>After the list.</p>\n"
        );
        // Not a bullet: a glyph with no space after it, a glyph inside a line, and code.
        assert_eq!(
            soft("•not an item\nsay • here", Markup::Slack),
            "<p>•not an item\nsay • here</p>\n"
        );
        assert_eq!(
            soft("```\n• kept as written\n```", Markup::Slack),
            "<pre><code>• kept as written\n</code></pre>\n"
        );
        // The hard rule leaves them as the lines they are.
        assert_eq!(
            to_html_as("• one\n• two", Markup::Slack, SingleNewline::Hard),
            "<p>• one<br>\n• two</p>\n"
        );
    }

    #[test]
    fn an_agents_title_line_still_ends_where_it_was_written() {
        let _rule = SingleNewlineGuard::set(SingleNewline::Soft);
        let text = "[build-bot] Backup check, 10:52 PM\nAll green.\nNothing else.";
        let (html, heading) = headed_body(text, Markup::Discord, true, &|_| false, &[]);
        assert!(heading.is_some());
        assert_eq!(
            html,
            "<p><span class=\"rh-src\">[build-bot] Backup check, 10:52 PM<br>\n</span>All green.\n\
             Nothing else.</p>\n"
        );
    }

    #[test]
    fn read_aloud_blocks_follow_the_soft_rule() {
        let _rule = SingleNewlineGuard::set(SingleNewline::Soft);
        let blocks = source_blocks("one\ntwo\n\n• a\n• b", Markup::Slack, &[]);
        let sources: Vec<&str> = blocks.iter().map(|block| block.source.as_str()).collect();
        assert_eq!(sources, ["one\ntwo", "a", "b"]);
    }
}

#[cfg(test)]
mod glued_table_tests {
    //! `markdown-tables-render`: a table whose line breaks were lost before it got here.

    use super::{to_html_as, Markup, SingleNewline};

    /// A message as Slack stored it, names and addresses scrubbed: its header row glued to the
    /// line before it, and the paragraph after it glued to its last row.
    const STORED: &str = include_str!("../tests/fixtures/slack_glued_table.md");

    #[test]
    fn the_stored_message_draws_its_table_under_either_rule() {
        for rule in [SingleNewline::Soft, SingleNewline::Hard] {
            let html = to_html_as(STORED, Markup::Slack, rule);
            assert_eq!(html.matches("<table>").count(), 1, "{rule:?}: {html}");
            assert_eq!(html.matches("<tr>").count(), 6, "{rule:?}");
            assert!(html.contains("<th>Backend</th>\n<th align=\"right\">Two runs identical</th>"));
            assert!(html.contains("<td>in-guest-trap</td>\n<td align=\"right\">302</td>"));
            assert!(
                html.contains("</table>\n<p>Most LiteInst and in-guest-trap gaps are named"),
                "{rule:?}: the prose glued to the last row is a paragraph after the table"
            );
            assert!(
                !html.contains("| Backend"),
                "{rule:?}: no row is left as pipes"
            );
            assert!(
                html.contains("scorecard</a>):"),
                "{rule:?}: the line the header was glued to keeps its words"
            );
        }
    }

    /// Drawn exactly as the text would be with no repair at all.
    fn unrepaired(text: &str) {
        let options = super::options(true, SingleNewline::Soft);
        assert_eq!(
            super::repair_glued_tables(text, &options),
            None,
            "{text:?} was repaired"
        );
    }

    #[test]
    fn only_an_unambiguous_table_is_repaired() {
        // A table already on its lines.
        unrepaired("| a | b |\n|---|---|\n| 1 | 2 |");
        // Cell counts that do not match the delimiter row.
        unrepaired("Intro:| a | b |\n|---|---|---|\n| 1 | 2 | 3 |");
        // A delimiter row without its leading pipe.
        unrepaired("Intro: a | b\n---|---\n1 | 2");
        // A header row that does not end in a pipe.
        unrepaired("Intro:| a | b\n|---|---|\n| 1 | 2 |");
        // The pipes are in a code span the text before them opened.
        unrepaired("Use `x| a | b |\n|---|---|");
        // Code.
        unrepaired("```\nIntro:| a | b |\n|---|---|\n| 1 | 2 |tail\n```");
        // Text after the last row's closing pipe that is not glued to it, or holds a pipe.
        unrepaired("| a | b |\n|---|---|\n| 1 | 2 | more");
        unrepaired("| a | b |\n|---|---|\n| 1 | 2 |x | y");
        // Not a delimiter row at all.
        unrepaired("Intro:| a | b |\n|---|--x|");
    }

    #[test]
    fn a_glued_header_and_a_glued_last_row_are_each_repaired() {
        let options = super::options(true, SingleNewline::Soft);
        assert_eq!(
            super::repair_glued_tables("Results:| a | b |\n|---|:-:|\n| 1 | 2 |", &options)
                .as_deref(),
            Some("Results:\n\n| a | b |\n|---|:-:|\n| 1 | 2 |")
        );
        // A table that parses without repair would drop the glued prose as an extra cell.
        assert_eq!(
            super::repair_glued_tables("| a | b |\n|---|---|\n| 1 | 2 |The end.", &options)
                .as_deref(),
            Some("| a | b |\n|---|---|\n| 1 | 2 |\n\nThe end.")
        );
        assert_eq!(
            to_html_as(
                "| a | b |\n|---|---|\n| 1 | 2 |The end.",
                Markup::Discord,
                SingleNewline::Soft
            ),
            "<table>\n<thead>\n<tr>\n<th>a</th>\n<th>b</th>\n</tr>\n</thead>\n<tbody>\n<tr>\n\
             <td>1</td>\n<td>2</td>\n</tr>\n</tbody>\n</table>\n<p>The end.</p>\n"
        );
    }
}

#[cfg(test)]
mod repair_review_tests {
    //! `markdown-tables-render`: the shapes an adversarial review of the repairs found, each kept
    //! as the writer wrote it, or repaired only where nothing written is lost.

    use super::{repair_glued_tables, soft_rule_lines, to_html_as, Markup, SingleNewline};

    fn options() -> comrak::Options<'static> {
        super::options(true, SingleNewline::Soft)
    }

    fn soft(text: &str, markup: Markup) -> String {
        to_html_as(text, markup, SingleNewline::Soft)
    }

    #[test]
    fn a_table_that_starts_an_item_or_already_draws_is_not_moved() {
        for text in [
            "- | a | b |\n  |---|---|\n  | 1 | 2 |",
            "- | a | b |\n|---|---|\n| 1 | 2 |",
            "> | a | b |\n|---|---|",
        ] {
            assert_eq!(repair_glued_tables(text, &options()), None, "{text:?}");
        }
        assert!(!soft("• | a | b |\n|---|---|\n| 1 | 2 |", Markup::Slack).contains("<li></li>"));
    }

    #[test]
    fn no_repair_may_lose_a_cell_or_split_code() {
        for text in [
            // A space before the first pipe: a header of three cells, not prose and two.
            "Name | Value | Notes |\n|---|---|\n| alice | 1 | ok |",
            // A row under it wider than the delimiter row: GFM would drop its last cell.
            "Name:| Value | Notes |\n|---|---|\n| alice | 1 | ok |",
            // The first pipe is inside a double-backtick code span.
            "Run ``x|y``:| a | b |\n|---|---|---|",
            // A lone carriage return, which comrak counts as a line end, before a code block.
            "a\rb\n```\n| a | b |\n|---|---|\n| 1 | 2 |tail\n```",
            // Text glued to a row in the middle of a table, not its last.
            "| a | b |\n|---|---|\n| 1 | 2 |.\n| 3 | 4 |",
        ] {
            assert_eq!(repair_glued_tables(text, &options()), None, "{text:?}");
        }
        // A table whose header has no leading pipe still keeps the prose glued to its last row.
        assert_eq!(
            repair_glued_tables("a | b\n|---|---|\n| 1 | 2 |tail", &options()).as_deref(),
            Some("a | b\n|---|---|\n| 1 | 2 |\n\ntail")
        );
    }

    #[test]
    fn a_line_after_typed_bullets_is_set_apart_only_where_nothing_else_changes() {
        // Emphasis and a link running across the line stay what they were.
        let bold = soft("• **Important: the build\nis broken**", Markup::Discord);
        assert!(
            bold.contains("<strong>Important: the build\nis broken</strong>"),
            "{bold}"
        );
        let link = soft(
            "• See [the design\ndoc](https://example.org/) here",
            Markup::Discord,
        );
        assert!(link.contains("<a href=\"https://example.org/\""), "{link}");
        // The rest of an item's sentence stays in the item.
        assert_eq!(
            soft("• a long item\nthat continues", Markup::Discord),
            "<ul>\n<li>a long item\nthat continues</li>\n</ul>\n"
        );
        // A line of its own is set apart.
        assert_eq!(
            soft("• one\n• two\nKnown limits: none.", Markup::Discord),
            "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n<p>Known limits: none.</p>\n"
        );
    }

    #[test]
    fn typed_bullets_in_a_quote_are_its_items_and_in_raw_html_are_left_alone() {
        assert_eq!(
            soft("> • one\n> • two", Markup::Slack),
            "<blockquote>\n<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n</blockquote>\n"
        );
        assert_eq!(
            soft_rule_lines("<div>\n• a\n</div>", &options(), Markup::Slack),
            None
        );
        assert_eq!(
            soft_rule_lines("text\n• \nmore", &options(), Markup::Slack),
            None
        );
    }

    #[test]
    fn a_google_chat_outline_of_typed_bullets_draws_as_one_of_dashes() {
        assert_eq!(
            soft("1. Goal\n• a\nIn review:\n• b", Markup::GoogleChat),
            soft("1. Goal\n- a\nIn review:\n- b", Markup::GoogleChat)
        );
    }
}

/// `#543 in-app-message-links`: a link to a chat message is marked with what it names, so the page
/// can keep the tap in the app. The shapes are `message_links`'s and tested there; here, that the
/// mark reaches the page on every way of writing a link, and nowhere else.
#[cfg(test)]
mod message_link_tests {
    use super::{headed_body, to_html, Markup};

    const LINK: &str = r#"target="_blank" rel="noopener noreferrer nofollow""#;
    const PERMALINK: &str = "https://acme.slack.com/archives/C0123ABCDE/p1700000000000100";

    /// What a link to [`PERMALINK`] is marked with.
    fn mark() -> String {
        crate::message_links::recognize(PERMALINK)
            .expect("a permalink")
            .attribute()
    }

    #[test]
    fn a_bare_a_named_and_an_angle_permalink_are_each_marked_and_keep_their_address() {
        let mark = mark();
        let bare = format!(r#"<a href="{PERMALINK}" data-msg="{mark}" {LINK}>{PERMALINK}</a>"#);
        let named = format!(r#"<a href="{PERMALINK}" data-msg="{mark}" {LINK}>the plan</a>"#);
        assert_eq!(
            to_html(&format!("see {PERMALINK} above"), Markup::Discord),
            format!("<p>see {bare} above</p>\n")
        );
        assert_eq!(
            to_html(&format!("see <{PERMALINK}>"), Markup::Discord),
            format!("<p>see {bare}</p>\n")
        );
        assert_eq!(
            to_html(&format!("see [the plan]({PERMALINK})"), Markup::Discord),
            format!("<p>see {named}</p>\n")
        );
        assert_eq!(
            to_html(&format!("see <{PERMALINK}|the plan>"), Markup::Slack),
            format!("<p>see {named}</p>\n")
        );
    }

    #[test]
    fn a_reply_link_is_marked_with_its_thread_and_its_query_kept() {
        let reply = format!("{PERMALINK}?thread_ts=1699999999.000200&cid=C0123ABCDE");
        let mark = crate::message_links::recognize(&reply)
            .expect("a reply")
            .attribute();
        assert!(mark.ends_with(":1699999999.000200"), "{mark}");
        let escaped = reply.replace('&', "&amp;");
        assert_eq!(
            to_html(&format!("[earlier]({reply})"), Markup::Discord),
            format!("<p><a href=\"{escaped}\" data-msg=\"{mark}\" {LINK}>earlier</a></p>\n")
        );
    }

    #[test]
    fn a_permalink_in_code_is_text_and_any_other_link_is_unmarked() {
        for text in [
            format!("`{PERMALINK}`"),
            format!("```\n{PERMALINK}\n```"),
            format!("    {PERMALINK}"),
        ] {
            let html = to_html(&text, Markup::Slack);
            assert!(
                !html.contains("<a ") && !html.contains("data-msg"),
                "{html}"
            );
        }
        let other = to_html(
            "https://example.com/archives/C0123ABCDE/p1700000000000100",
            Markup::Slack,
        );
        assert!(
            other.contains("<a ") && !other.contains("data-msg"),
            "{other}"
        );
    }

    #[test]
    fn a_slack_app_link_to_a_message_stays_a_link_and_any_other_slack_address_is_text() {
        let app = "slack://channel?team=T0123ABCD&id=C0123ABCDE&message=1700000000.000100";
        let mark = crate::message_links::recognize(app)
            .expect("an app link")
            .attribute();
        assert_eq!(
            to_html(&format!("[in Slack]({app})"), Markup::Discord),
            format!(
                "<p><a href=\"{}\" data-msg=\"{mark}\" {LINK}>in Slack</a></p>\n",
                app.replace('&', "&amp;")
            )
        );
        assert_eq!(
            to_html(
                "[x](slack://user?team=T0123ABCD&id=U0123ABCD)",
                Markup::Discord
            ),
            "<p>x (slack://user?team=T0123ABCD&amp;id=U0123ABCD)</p>\n"
        );
    }

    #[test]
    fn the_mark_survives_a_heading_being_marked_in_the_same_body() {
        // `mark_restated` sanitizes the marked body again and keeps it only if nothing changed.
        let text = format!("(plan) Status: done\nsee {PERMALINK}");
        let (html, heading) = headed_body(&text, Markup::Discord, true, &|_| true, &[]);
        assert!(heading.is_some(), "{html}");
        assert!(html.contains(&format!("data-msg=\"{}\"", mark())), "{html}");
    }
}
