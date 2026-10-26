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

use comrak::nodes::{AstNode, NodeValue, TableAlignment};
use comrak::{Arena, Options};

/// The vendor `--slack-mrkdwn` byte cap for one message.
pub const MAX_MRKDWN_BYTES: usize = 4_000;

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
    let limit = limit.clamp(1, MAX_MRKDWN_BYTES);
    let converted = convert_markdown(markdown);
    if converted.is_empty() {
        return vec![String::new()];
    }
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    for block in split_blocks(&converted) {
        let candidate = if current.is_empty() {
            block.to_owned()
        } else {
            format!("{current}\n\n{block}")
        };
        if candidate.len() <= limit {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        if block.len() <= limit {
            current = block;
        } else {
            let mut chunks = split_oversize_block(&block, limit);
            let last = chunks.pop().unwrap_or_default();
            parts.extend(chunks);
            current = last;
        }
    }
    if !current.is_empty() || parts.is_empty() {
        parts.push(current);
    }
    parts
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
        NodeValue::Heading(heading) => {
            let mut inner = String::new();
            for child in node.children() {
                plain_inline(child, &mut inner);
            }
            let inner = inner.trim();
            if inner.is_empty() {
                String::new()
            } else {
                let _ = heading.level;
                format!("*{inner}*")
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
}
