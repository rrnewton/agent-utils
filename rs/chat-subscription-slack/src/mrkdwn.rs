//! Convert standard Markdown to Slack mrkdwn without the vendor conversion.
//!
//! The `converted` outbound path uses this module: the helper parses Markdown
//! itself (comrak, with the table, strikethrough, task-list and autolink
//! extensions) and renders Slack mrkdwn that preserves the source layout:
//! paragraphs keep their blank lines, headings become bold lines, lists keep
//! bullets/numbers and indentation as text, tables become an aligned
//! monospace code block, and task boxes become `☐`/`☑`. The vendor
//! `--slack-mrkdwn` body keeps every line break, so what is rendered here is
//! what Slack stores.
//!
//! Headings (`#551 headings-real-headings`). A part of a message that holds a
//! heading of levels 1 to 3, of at most [`MAX_HEADER_CHARS`] characters
//! (counted long: UTF-16 units of its escaped text) and with no link, is
//! also given Block Kit blocks ([`ConvertedPart::blocks`]): each such heading
//! a `header` block, which Slack's own client draws as a heading and its
//! history returns as one, and the mrkdwn between them `section` blocks. The
//! mrkdwn above stays the message text, which Slack shows in notifications and
//! the client's stored text returns (with its newlines as spaces). A deeper or
//! longer heading, or one with a link, stays a bold line; a heading never ends
//! a part that has more after it; and a part whose blocks would pass [`MAX_BLOCKS`] is posted as its
//! mrkdwn alone. Slack's header block has no level, so every header is one
//! heading level to a reader.

use comrak::nodes::{AstNode, NodeValue, TableAlignment};
use comrak::{Arena, Options};
use serde_json::{json, Value};

/// The vendor `--slack-mrkdwn` byte cap for one message.
pub const MAX_MRKDWN_BYTES: usize = 4_000;

/// Slack's cap on a header block's plain text, in characters.
pub const MAX_HEADER_CHARS: usize = 150;

/// Slack's cap on a section block's mrkdwn text, in characters.
pub const MAX_SECTION_CHARS: usize = 3_000;

/// The vendor client's cap on the Block Kit blocks of one message.
pub const MAX_BLOCKS: usize = 50;

/// The deepest heading posted as a header block; a deeper one stays a bold line.
pub const MAX_HEADER_LEVEL: u8 = 3;

/// One message of a converted reply: its mrkdwn text, and the Block Kit blocks
/// that say the same with real headings, when it holds a heading that can be one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConvertedPart {
    /// The mrkdwn body, at most the split limit in bytes; with blocks, the
    /// notification fallback and the text Slack's history returns.
    pub text: String,
    /// The blocks as a JSON array, or `None` to post `text` alone.
    pub blocks: Option<String>,
}

/// One blank-line separated block of converted mrkdwn, with the plain text of
/// the header block it may be posted as when it is a heading that can be one.
struct Unit {
    text: String,
    header: Option<String>,
}

fn options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options
}

/// Escape one plain-text fragment for Slack mrkdwn (`&`, `<`, `>`).
#[must_use]
pub fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_url(url: &str) -> String {
    url.replace('&', "&amp;")
        .replace('|', "%7C")
        .replace('>', "%3E")
        .replace('<', "%3C")
        .replace(' ', "%20")
}

/// Convert Markdown to one mrkdwn string. Blocks are separated by a blank
/// line; the result has no trailing blank line.
#[must_use]
pub fn convert_markdown(markdown: &str) -> String {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, markdown, &options());
    let mut blocks = Vec::new();
    for child in root.children() {
        let block = render_block(child, 0);
        let block = block.trim_matches('\n').to_owned();
        if !block.trim().is_empty() {
            blocks.push(block);
        }
    }
    blocks.join("\n\n")
}

/// Convert Markdown and split the result into parts of at most `limit` bytes,
/// preferring block boundaries, then line boundaries (see
/// [`crate::outbound::split_body`]).
#[must_use]
pub fn convert_and_split(markdown: &str, limit: usize) -> Vec<String> {
    convert_and_split_parts(markdown, limit)
        .into_iter()
        .map(|part| part.text)
        .collect()
}

/// [`convert_and_split`], each part with the Block Kit blocks that post its
/// headings as headings, when it has one that can be (see the module notes).
#[must_use]
pub fn convert_and_split_parts(markdown: &str, limit: usize) -> Vec<ConvertedPart> {
    let limit = limit.clamp(1, MAX_MRKDWN_BYTES);
    let units = convert_units(markdown);
    let mut parts: Vec<Vec<Unit>> = Vec::new();
    let mut current: Vec<Unit> = Vec::new();
    let mut current_bytes = 0;
    for unit in units {
        let joined = if current.is_empty() {
            unit.text.len()
        } else {
            current_bytes + 2 + unit.text.len()
        };
        if joined <= limit {
            current_bytes = joined;
            current.push(unit);
            continue;
        }
        if !current.is_empty() {
            // A heading never ends a part with more after it: it heads nothing there, so it
            // opens the next part with the block it heads.
            let carried = (current.len() > 1
                && current.last().is_some_and(|last| last.header.is_some()))
            .then(|| current.pop())
            .flatten();
            parts.push(std::mem::take(&mut current));
            if let Some(heading) = carried {
                let joined = heading.text.len() + 2 + unit.text.len();
                current.push(heading);
                if joined <= limit {
                    current_bytes = joined;
                    current.push(unit);
                    continue;
                }
                parts.push(std::mem::take(&mut current));
            }
        }
        if unit.text.len() <= limit {
            current_bytes = unit.text.len();
            current.push(unit);
        } else {
            let mut chunks = split_oversize_block(&unit.text, limit);
            let last = chunks.pop().unwrap_or_default();
            parts.extend(
                chunks
                    .into_iter()
                    .map(|text| vec![Unit { text, header: None }]),
            );
            current_bytes = last.len();
            current.push(Unit {
                text: last,
                header: None,
            });
        }
    }
    if !current.is_empty() || parts.is_empty() {
        parts.push(current);
    }
    parts
        .into_iter()
        .map(|units| ConvertedPart {
            text: units
                .iter()
                .map(|unit| unit.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
            blocks: blocks_for(&units),
        })
        .collect()
}

/// The converted document as [`convert_markdown`] writes it, one unit per
/// blank-line separated block, each top-level heading that can be a header
/// block carrying its plain text.
fn convert_units(markdown: &str) -> Vec<Unit> {
    let arena = Arena::new();
    let root = comrak::parse_document(&arena, markdown, &options());
    let mut units = Vec::new();
    for child in root.children() {
        let block = render_block(child, 0);
        let block = block.trim_matches('\n');
        if block.trim().is_empty() {
            continue;
        }
        // A heading with a link stays a bold line, where the link can still be followed: a
        // header block is plain text.
        let header = match &child.data.borrow().value {
            NodeValue::Heading(heading)
                if heading.level <= MAX_HEADER_LEVEL && !has_link(child) =>
            {
                let plain = heading_plain(child);
                (slack_length(&escape_text(&plain)) <= MAX_HEADER_CHARS).then_some(plain)
            }
            _ => None,
        };
        if header.is_some() {
            units.push(Unit {
                text: block.to_owned(),
                header,
            });
        } else {
            units.extend(
                split_blocks(block)
                    .into_iter()
                    .map(|text| Unit { text, header: None }),
            );
        }
    }
    units
}

/// A length as Slack may count it against a block's limit, erring long: UTF-16 code units, the
/// way a JavaScript string counts, over the text as escaped for the wire.
fn slack_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Whether `node` holds a link or an autolink anywhere inside it.
fn has_link<'a>(node: &'a AstNode<'a>) -> bool {
    node.descendants()
        .any(|inner| matches!(inner.data.borrow().value, NodeValue::Link(_)))
}

/// A heading's words as plain text, its inline markup dropped.
fn heading_plain<'a>(node: &'a AstNode<'a>) -> String {
    let mut inner = String::new();
    for child in node.children() {
        plain_inline(child, &mut inner);
    }
    inner.trim().to_owned()
}

/// The Block Kit blocks for one part's units: each header unit a `header`
/// block and the mrkdwn between them `section` blocks, each at most
/// [`MAX_SECTION_CHARS`]. `None` for a part with no header unit, so it posts as
/// before, or one whose blocks would pass [`MAX_BLOCKS`].
fn blocks_for(units: &[Unit]) -> Option<String> {
    if !units.iter().any(|unit| unit.header.is_some()) {
        return None;
    }
    let mut blocks: Vec<Value> = Vec::new();
    let mut section = String::new();
    let flush = |section: &mut String, blocks: &mut Vec<Value>| {
        if !section.is_empty() {
            let text = std::mem::take(section);
            blocks.push(json!({"type": "section", "text": {"type": "mrkdwn", "text": text}}));
        }
    };
    for unit in units {
        if let Some(plain) = &unit.header {
            flush(&mut section, &mut blocks);
            blocks.push(json!({
                "type": "header",
                "text": {"type": "plain_text", "text": plain, "emoji": true}
            }));
            continue;
        }
        let pieces = if slack_length(&unit.text) <= MAX_SECTION_CHARS {
            vec![unit.text.clone()]
        } else {
            // Byte-bounded, so each piece is within the UTF-16 cap too.
            split_oversize_block(&unit.text, MAX_SECTION_CHARS)
        };
        for piece in pieces {
            if !section.is_empty()
                && slack_length(&section) + 2 + slack_length(&piece) > MAX_SECTION_CHARS
            {
                flush(&mut section, &mut blocks);
            }
            if !section.is_empty() {
                section.push_str("\n\n");
            }
            section.push_str(&piece);
        }
    }
    flush(&mut section, &mut blocks);
    (blocks.len() <= MAX_BLOCKS).then(|| Value::Array(blocks).to_string())
}

/// Blocks of a converted document: blank-line separated, except that a
/// blank line inside a code fence does not end the block.
fn split_blocks(converted: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in converted.split('\n') {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if line.is_empty() && !in_fence {
            if !current.is_empty() {
                blocks.push(current.join("\n"));
                current = Vec::new();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }
    blocks
}

/// Split one block that alone exceeds `limit`. A fenced block is split at
/// line boundaries and every part is re-fenced, so no part opens or leaves
/// a fence dangling; any other block splits at line boundaries.
fn split_oversize_block(block: &str, limit: usize) -> Vec<String> {
    let lines: Vec<&str> = block.lines().collect();
    let fenced = lines.len() >= 2 && lines[0].trim_start().starts_with("```");
    if !fenced {
        return crate::outbound::split_body(block, limit);
    }
    let inner = &lines[1..lines.len()
        - usize::from(
            lines
                .last()
                .is_some_and(|line| line.trim_start().starts_with("```")),
        )];
    let budget = limit.saturating_sub(8).max(1);
    let mut parts = Vec::new();
    let mut current = String::new();
    for line in inner {
        let candidate = if current.is_empty() {
            (*line).to_owned()
        } else {
            format!("{current}\n{line}")
        };
        if candidate.len() <= budget {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            parts.push(format!("```\n{current}\n```"));
            current = String::new();
        }
        if line.len() <= budget {
            current = (*line).to_owned();
        } else {
            for chunk in crate::outbound::split_body(line, budget) {
                parts.push(format!("```\n{chunk}\n```"));
            }
        }
    }
    if !current.is_empty() || parts.is_empty() {
        parts.push(format!("```\n{current}\n```"));
    }
    parts
}

fn render_block<'a>(node: &'a AstNode<'a>, depth: usize) -> String {
    match &node.data.borrow().value {
        NodeValue::Paragraph => {
            let mut out = String::new();
            for child in node.children() {
                render_inline(child, &mut out);
            }
            out
        }
        NodeValue::Heading(_) => {
            let inner = heading_plain(node);
            if inner.is_empty() {
                String::new()
            } else {
                format!("*{}*", escape_text(&inner))
            }
        }
        NodeValue::BlockQuote | NodeValue::MultilineBlockQuote(_) => {
            let mut inner = Vec::new();
            for child in node.children() {
                let block = render_block(child, depth);
                let block = block.trim_matches('\n').to_owned();
                if !block.is_empty() {
                    inner.push(block);
                }
            }
            inner
                .join("\n\n")
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        ">".to_owned()
                    } else {
                        format!("> {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        NodeValue::List(list) => render_list(node, list, depth),
        NodeValue::CodeBlock(code) => {
            let literal = code.literal.trim_end_matches('\n');
            format!("```\n{literal}\n```")
        }
        NodeValue::Table(table) => render_table(node, &table.alignments),
        NodeValue::ThematicBreak => "---".to_owned(),
        NodeValue::HtmlBlock(html) => escape_text(html.literal.trim()),
        NodeValue::FootnoteDefinition(_) => String::new(),
        _ => {
            // Containers and any other block: render children as blocks.
            let mut blocks = Vec::new();
            for child in node.children() {
                let block = render_block(child, depth);
                let block = block.trim_matches('\n').to_owned();
                if !block.trim().is_empty() {
                    blocks.push(block);
                }
            }
            blocks.join("\n\n")
        }
    }
}

fn render_list<'a>(node: &'a AstNode<'a>, list: &comrak::nodes::NodeList, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut lines: Vec<String> = Vec::new();
    let mut number = list.start;
    for item in node.children() {
        let (marker, task) = match &item.data.borrow().value {
            NodeValue::TaskItem(task) => {
                let box_mark = if task.symbol.is_some() { "☑" } else { "☐" };
                let bullet = if list.list_type == comrak::nodes::ListType::Bullet {
                    format!("• {box_mark}")
                } else {
                    let n = number;
                    number += 1;
                    format!("{n}. {box_mark}")
                };
                (bullet, true)
            }
            _ => {
                if list.list_type == comrak::nodes::ListType::Bullet {
                    ("•".to_owned(), false)
                } else {
                    let n = number;
                    number += 1;
                    (format!("{n}."), false)
                }
            }
        };
        let _ = task;
        let mut item_blocks: Vec<String> = Vec::new();
        for child in item.children() {
            if matches!(child.data.borrow().value, NodeValue::List(_)) {
                item_blocks.push(render_block(child, depth + 1));
            } else {
                let block = render_block(child, depth + 1);
                let block = block.trim_matches('\n').to_owned();
                if !block.is_empty() {
                    item_blocks.push(block);
                }
            }
        }
        let mut item_lines = item_blocks
            .join("\n")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if item_lines.is_empty() {
            item_lines.push(String::new());
        }
        for (index, line) in item_lines.iter().enumerate() {
            if index == 0 {
                lines.push(format!("{indent}{marker} {line}"));
            } else if line.starts_with(&"  ".repeat(depth + 1)) {
                lines.push(line.clone());
            } else if line.trim().is_empty() {
                lines.push(String::new());
            } else {
                lines.push(format!("{indent}  {line}"));
            }
        }
    }
    lines.join("\n")
}

fn plain_inline<'a>(node: &'a AstNode<'a>, out: &mut String) {
    match &node.data.borrow().value {
        NodeValue::Text(text) => out.push_str(text),
        NodeValue::Code(code) => out.push_str(&code.literal),
        NodeValue::SoftBreak | NodeValue::LineBreak => out.push(' '),
        NodeValue::Link(link) | NodeValue::Image(link) => {
            let mut label = String::new();
            for child in node.children() {
                plain_inline(child, &mut label);
            }
            out.push_str(label.trim());
            let url = link.url.trim();
            if !url.is_empty() && url != label.trim() {
                out.push_str(&format!(" ({url})"));
            }
        }
        NodeValue::HtmlInline(html) => out.push_str(html),
        _ => {
            for child in node.children() {
                plain_inline(child, out);
            }
        }
    }
}

fn render_table<'a>(node: &'a AstNode<'a>, alignments: &[TableAlignment]) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in node.children() {
        let mut cells = Vec::new();
        for cell in row.children() {
            let mut text = String::new();
            for child in cell.children() {
                plain_inline(child, &mut text);
            }
            cells.push(text.trim().replace('\n', " "));
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    if rows.is_empty() {
        return String::new();
    }
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0_usize; columns];
    for row in &rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let render_row = |row: &Vec<String>| -> String {
        let mut line = String::new();
        for (index, &width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            let padded = match alignments
                .get(index)
                .copied()
                .unwrap_or(TableAlignment::None)
            {
                TableAlignment::Right => format!("{cell:>width$}"),
                TableAlignment::Center => {
                    let total = width.saturating_sub(cell.chars().count());
                    let left = total / 2;
                    format!("{}{cell}{}", " ".repeat(left), " ".repeat(total - left))
                }
                _ => format!("{cell:<width$}"),
            };
            if index > 0 {
                line.push_str("  ");
            }
            line.push_str(&padded);
        }
        line.trim_end().to_owned()
    };
    let mut out = String::from("```\n");
    for row in &rows {
        out.push_str(&render_row(row));
        out.push('\n');
    }
    out.push_str("```");
    out
}

fn render_inline<'a>(node: &'a AstNode<'a>, out: &mut String) {
    match &node.data.borrow().value {
        NodeValue::Text(text) => {
            // A literal asterisk in source text is not valid emphasis (the
            // parser would have made it one), so keep it visible without
            // letting Slack re-read it as a bold delimiter.
            out.push_str(&escape_text(text).replace('*', "\u{2217}"));
        }
        NodeValue::SoftBreak => out.push(' '),
        NodeValue::LineBreak => out.push('\n'),
        NodeValue::Code(code) => {
            out.push('`');
            out.push_str(&code.literal);
            out.push('`');
        }
        NodeValue::Strong => {
            out.push('*');
            for child in node.children() {
                render_inline(child, out);
            }
            out.push('*');
        }
        NodeValue::Emph => {
            out.push('_');
            for child in node.children() {
                render_inline(child, out);
            }
            out.push('_');
        }
        NodeValue::Strikethrough => {
            out.push('~');
            for child in node.children() {
                render_inline(child, out);
            }
            out.push('~');
        }
        NodeValue::Link(link) => {
            let mut label = String::new();
            for child in node.children() {
                plain_inline(child, &mut label);
            }
            let label = label
                .trim()
                .replace(['|', '<', '>', '\n', '\r'], " ")
                .replace("  ", " ");
            let url = escape_url(link.url.trim());
            if url.is_empty() {
                out.push_str(&escape_text(&label));
            } else if label.trim().is_empty() {
                out.push_str(&format!("<{url}>"));
            } else {
                out.push_str(&format!("<{url}|{}>", escape_text(label.trim())));
            }
        }
        NodeValue::Image(link) => {
            let mut label = String::new();
            for child in node.children() {
                plain_inline(child, &mut label);
            }
            let url = escape_url(link.url.trim());
            let label = escape_text(label.trim());
            if url.is_empty() {
                out.push_str(&label);
            } else if label.is_empty() {
                out.push_str(&format!("<{url}>"));
            } else {
                out.push_str(&format!("<{url}|{label}>"));
            }
        }
        NodeValue::HtmlInline(html) => {
            // Raw HTML is shown as escaped text, never as Slack markup.
            out.push_str(&escape_text(html));
        }
        NodeValue::FootnoteReference(reference) => {
            out.push_str(&format!("[^{}]", reference.name));
        }
        _ => {
            for child in node.children() {
                render_inline(child, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_blank_lines_and_headings() {
        let out = convert_markdown("# Title\n\nFirst para.\n\nSecond para.\n");
        assert_eq!(out, "*Title*\n\nFirst para.\n\nSecond para.");
    }

    #[test]
    fn converts_inline_formatting_and_links() {
        let out = convert_markdown(
            "**bold** *italic* ~~gone~~ `code` [docs](https://example.com/x) & <tag>",
        );
        assert_eq!(
            out,
            "*bold* _italic_ ~gone~ `code` <https://example.com/x|docs> &amp; &lt;tag&gt;"
        );
    }

    #[test]
    fn converts_lists_tasks_quotes_and_code() {
        let md = "- one\n- two\n  - nested\n\n1. first\n2. second\n\n- [ ] todo\n- [x] done\n\n> quoted\n\n```rust\nfn main() {}\n```\n";
        let out = convert_markdown(md);
        assert!(out.contains("• one\n• two\n  • nested"), "{out}");
        assert!(out.contains("1. first\n2. second"), "{out}");
        assert!(out.contains("• ☐ todo"), "{out}");
        assert!(out.contains("• ☑ done"), "{out}");
        assert!(out.contains("> quoted"), "{out}");
        assert!(out.contains("```\nfn main() {}\n```"), "{out}");
    }

    #[test]
    fn converts_table_to_aligned_code_block() {
        let md = "| Host | p50 |\n|---|---:|\n| alpha | 12 |\n| beta | 340 |\n";
        let out = convert_markdown(md);
        assert!(out.starts_with("```\n"), "{out}");
        assert!(out.contains("Host"), "{out}");
        let lines: Vec<&str> = out.lines().collect();
        // Right-aligned numeric column: header and values share a right edge.
        assert_eq!(
            lines[1].find("p50").map(|p| p + 3),
            lines[3].find("340").map(|p| p + 3),
            "{out}"
        );
        assert!(out.ends_with("```"), "{out}");
    }

    #[test]
    fn nested_bullet_table_stays_separate() {
        let md = "Summary line.\n\n- item\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\nAfter.\n";
        let out = convert_markdown(md);
        assert!(out.contains("Summary line.\n\n• item\n\n```"), "{out}");
        assert!(out.ends_with("After."), "{out}");
    }

    #[test]
    fn heading_and_link_label_carry_no_nested_markers() {
        assert_eq!(
            convert_markdown("# **Bold** and `code` title"),
            "*Bold and code title*"
        );
        assert_eq!(
            convert_markdown("[la|bel](https://example.com/?a=1&b=2)"),
            "<https://example.com/?a=1&amp;b=2|la bel>"
        );
        assert_eq!(convert_markdown("2 * 3"), "2 \u{2217} 3");
    }

    #[test]
    fn table_cells_keep_link_urls() {
        let out = convert_markdown(
            "| Item | Note |\n|---|---|\n| [docs](https://example.com) | **bold** |\n",
        );
        assert!(out.contains("docs (https://example.com)"), "{out}");
        assert!(out.contains("bold"), "{out}");
    }

    #[test]
    fn oversize_table_and_code_split_into_refenced_parts() {
        let rows = (0..300)
            .map(|i| format!("| host{i} | {i} |"))
            .collect::<Vec<_>>()
            .join("\n");
        let md = format!(
            "| Host | p50 |\n|---|---|\n{rows}\n\n```\n{}\n```\n",
            "x = 1;\n\n".repeat(600)
        );
        let parts = convert_and_split(&md, 4_000);
        assert!(parts.len() > 1, "{parts:?}");
        for part in &parts {
            assert!(part.len() <= 4_000, "{} bytes", part.len());
            let fences = part.matches("```").count();
            assert_eq!(fences % 2, 0, "dangling fence in {part:?}");
        }
    }

    #[test]
    fn split_respects_cap_and_block_boundaries() {
        let md = (0..200)
            .map(|i| format!("Paragraph {i} with some text to fill space."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let parts = convert_and_split(&md, 4_000);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.len() <= 4_000));
        assert_eq!(parts.join("\n\n"), convert_markdown(&md));
    }

    fn parsed_blocks(part: &ConvertedPart) -> Vec<Value> {
        let blocks = part.blocks.as_deref().expect("blocks");
        serde_json::from_str::<Vec<Value>>(blocks).expect("blocks JSON")
    }

    #[test]
    fn headings_become_header_blocks_beside_unchanged_mrkdwn() {
        // `#551 headings-real-headings`.
        let md = "# Weekly report\n\nStatus is **green** & fine.\n\n## Numbers\n\n1. one\n2. two\n\n### Small & `plain`\n\nEnd.";
        let parts = convert_and_split_parts(md, 4_000);
        assert_eq!(parts.len(), 1);
        // The text is exactly what the mrkdwn-only path posts.
        assert_eq!(parts[0].text, convert_markdown(md));
        assert_eq!(convert_and_split(md, 4_000), vec![parts[0].text.clone()]);
        let blocks = parsed_blocks(&parts[0]);
        let kinds: Vec<&str> = blocks.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            ["header", "section", "header", "section", "header", "section"]
        );
        assert_eq!(
            blocks[0]["text"],
            json!({"type": "plain_text", "text": "Weekly report", "emoji": true})
        );
        assert_eq!(blocks[1]["text"]["type"], "mrkdwn");
        assert_eq!(blocks[1]["text"]["text"], "Status is *green* &amp; fine.");
        assert_eq!(blocks[3]["text"]["text"], "1. one\n2. two");
        // A header is plain text: entities are mrkdwn's, so it keeps the character itself.
        assert_eq!(blocks[4]["text"]["text"], "Small & plain");
        assert!(
            parts[0].text.contains("*Small &amp; plain*"),
            "{}",
            parts[0].text
        );
    }

    #[test]
    fn deep_long_or_absent_headings_post_mrkdwn_alone_or_as_bold_lines() {
        // No heading at all: the post is unchanged, with no blocks.
        let plain = convert_and_split_parts("Just a paragraph.\n\n- a list", 4_000);
        assert_eq!(plain[0].blocks, None);
        // Level 4 and beyond stay bold lines, so a part with only those has no blocks either.
        let deep = convert_and_split_parts("#### Fourth\n\ntext", 4_000);
        assert_eq!(deep[0].blocks, None);
        assert_eq!(deep[0].text, "*Fourth*\n\ntext");
        // A heading past Slack's header cap stays a bold line inside a section.
        let long = "x".repeat(MAX_HEADER_CHARS + 1);
        let parts = convert_and_split_parts(&format!("# Short\n\n# {long}\n\nbody"), 4_000);
        let blocks = parsed_blocks(&parts[0]);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "header");
        assert_eq!(blocks[1]["text"]["text"], format!("*{long}*\n\nbody"));
    }

    #[test]
    fn a_heading_with_a_link_or_past_the_counted_cap_stays_a_bold_line() {
        let linked = convert_and_split_parts("# See [docs](https://example.com/a)\n\nbody", 4_000);
        assert_eq!(linked[0].blocks, None);
        // 146 characters, but `&&` is 10 on the wire: over 150.
        let heading = format!("{}&&", "x".repeat(MAX_HEADER_CHARS - 4));
        let parts = convert_and_split_parts(&format!("# {heading}\n\nbody"), 4_000);
        assert_eq!(parts[0].blocks, None);
        // An astral emoji is two UTF-16 units.
        let heading = format!("{}🚀", "x".repeat(MAX_HEADER_CHARS - 1));
        assert_eq!(
            convert_and_split_parts(&format!("# {heading}\n\nbody"), 4_000)[0].blocks,
            None
        );
    }

    #[test]
    fn a_heading_never_ends_a_part_with_more_after_it() {
        let para = "word ".repeat(150).trim_end().to_owned(); // 749 bytes
        let md = format!("{para}\n\n# Next\n\n{para}");
        let parts = convert_and_split_parts(&md, 800);
        assert_eq!(parts.len(), 2, "{parts:?}");
        assert_eq!(parts[0].text, para);
        assert!(parts[1].text.starts_with("*Next*\n\n"), "{}", parts[1].text);
        assert!(parts[1].blocks.is_some());
        assert_eq!(
            convert_and_split(&md, 800),
            vec![parts[0].text.clone(), parts[1].text.clone()]
        );
    }

    #[test]
    fn blocks_respect_section_and_block_caps() {
        // A section never passes Slack's 3,000 characters: long mrkdwn is split across sections.
        let para = "word ".repeat(400).trim_end().to_owned();
        let md = format!("# Head\n\n{para}\n\n{para}");
        let parts = convert_and_split_parts(&md, 4_000);
        for part in &parts {
            if part.blocks.is_some() {
                for block in parsed_blocks(part) {
                    if block["type"] == "section" {
                        let text = block["text"]["text"].as_str().unwrap();
                        assert!(text.chars().count() <= MAX_SECTION_CHARS);
                    }
                }
            }
        }
        // More than 50 blocks: that part posts its mrkdwn alone.
        let many: String = (0..30)
            .map(|n| format!("## H{n}\n\nline {n}\n\n"))
            .collect();
        let parts = convert_and_split_parts(&many, 4_000);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].blocks, None);
        assert_eq!(parts[0].text, convert_markdown(&many));
    }
}
