//! Terminal rendering for `agentctl tail`: styled spans, width-aware wrapping, and a small
//! line-oriented Markdown renderer for the agent's prose.
//!
//! Everything is built as [`Line`]s of styled [`Span`]s and only turned into text at the end,
//! so the same layout prints with ANSI colors (pretty) or without (plain). Text is sanitised
//! when a span is made, before any escape sequence of ours is added, so nothing an agent or a
//! sender wrote can reach the terminal as a control sequence.
//!
//! The Markdown renderer keeps the author's line breaks (it never joins lines into one
//! paragraph) and soft-wraps each line to the width with a hanging indent. It knows headings,
//! bullet and numbered lists, block quotes, fenced code (boxed, never wrapped into prose),
//! tables (columns aligned when they fit), rules, and inline code, emphasis and links. In
//! plain mode the inline markers and heading hashes stay, since no style replaces them.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// ANSI foreground colors used by the renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    /// 31
    Red,
    /// 32
    Green,
    /// 33
    Yellow,
    /// 34
    Blue,
    /// 35
    Magenta,
    /// 36
    Cyan,
    /// 90
    Gray,
    /// 91
    BrightRed,
    /// 93
    BrightYellow,
    /// 97
    White,
}

impl Color {
    fn code(self) -> u8 {
        match self {
            Self::Red => 31,
            Self::Green => 32,
            Self::Yellow => 33,
            Self::Blue => 34,
            Self::Magenta => 35,
            Self::Cyan => 36,
            Self::Gray => 90,
            Self::BrightRed => 91,
            Self::BrightYellow => 93,
            Self::White => 97,
        }
    }
}

/// How a span is drawn in pretty mode. Plain mode ignores it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Style {
    /// Foreground color.
    pub fg: Option<Color>,
    /// Bold.
    pub bold: bool,
    /// Dim (faint).
    pub dim: bool,
    /// Italic.
    pub italic: bool,
    /// Underlined.
    pub underline: bool,
}

impl Style {
    /// No styling.
    pub const PLAIN: Self = Self {
        fg: None,
        bold: false,
        dim: false,
        italic: false,
        underline: false,
    };

    /// Only a foreground color.
    #[must_use]
    pub const fn fg(color: Color) -> Self {
        Self {
            fg: Some(color),
            ..Self::PLAIN
        }
    }

    /// Dim, no color.
    #[must_use]
    pub const fn dim() -> Self {
        Self {
            dim: true,
            ..Self::PLAIN
        }
    }

    /// This style, bold.
    #[must_use]
    pub const fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    /// This style, dim.
    #[must_use]
    pub const fn dimmed(self) -> Self {
        Self { dim: true, ..self }
    }

    /// This style, italic.
    #[must_use]
    pub const fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }

    /// This style, underlined.
    #[must_use]
    pub const fn underlined(self) -> Self {
        Self {
            underline: true,
            ..self
        }
    }

    fn sgr(self) -> String {
        let mut codes = Vec::new();
        if self.bold {
            codes.push("1".to_owned());
        }
        if self.dim {
            codes.push("2".to_owned());
        }
        if self.italic {
            codes.push("3".to_owned());
        }
        if self.underline {
            codes.push("4".to_owned());
        }
        if let Some(color) = self.fg {
            codes.push(color.code().to_string());
        }
        codes.join(";")
    }
}

/// A run of text in one style. Its text holds no control characters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span {
    /// The style.
    pub style: Style,
    /// The sanitised text.
    pub text: String,
}

impl Span {
    /// A span of `text` (sanitised) in `style`.
    #[must_use]
    pub fn new(style: Style, text: &str) -> Self {
        Self {
            style,
            text: sanitize(text),
        }
    }
}

/// One output line.
pub type Line = Vec<Span>;

/// `text` safe for a terminal: tabs become spaces, and control and bidirectional-override
/// characters (escape sequences in a sender's text or the model's output) become U+FFFD.
#[must_use]
pub fn sanitize(text: &str) -> String {
    if !text.chars().any(unsafe_char) {
        return text.to_owned();
    }
    text.chars()
        .map(|c| match c {
            '\t' => ' ',
            c if unsafe_char(c) => '\u{fffd}',
            c => c,
        })
        .collect()
}

fn unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// Display width of `text` in terminal columns.
#[must_use]
pub fn width_of(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// Display width of a line.
#[must_use]
pub fn line_width(line: &[Span]) -> usize {
    line.iter().map(|span| width_of(&span.text)).sum()
}

/// A line as text: with ANSI styling when `color`, else bare. Trailing blanks are dropped.
#[must_use]
pub fn emit(line: &[Span], color: bool) -> String {
    let last = line
        .iter()
        .rposition(|span| !span.text.trim_end().is_empty());
    // Runs of one style, with spaces between two runs of the same style joined into them.
    let mut runs: Vec<(Style, String)> = Vec::new();
    for (index, span) in line.iter().enumerate() {
        let text = match last {
            Some(last) if index == last => span.text.trim_end(),
            Some(last) if index > last => "",
            None => "",
            _ => span.text.as_str(),
        };
        if text.is_empty() {
            continue;
        }
        // Spaces show no color, so they join the run before them unless it is underlined.
        let blank = text.trim().is_empty();
        match runs.last_mut() {
            Some((style, run)) if *style == span.style || (blank && !style.underline) => {
                run.push_str(text);
            }
            _ => runs.push((span.style, text.to_owned())),
        }
    }
    let mut out = String::new();
    for (style, text) in runs {
        let sgr = style.sgr();
        if color && !sgr.is_empty() {
            out.push_str("\u{1b}[");
            out.push_str(&sgr);
            out.push('m');
            out.push_str(&text);
            out.push_str("\u{1b}[0m");
        } else {
            out.push_str(&text);
        }
    }
    out
}

/// `text` cut to `width` columns, with `…` when cut.
#[must_use]
pub fn truncate(text: &str, width: usize) -> String {
    if width_of(text) <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > width {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

/// One unbreakable word, possibly in several styles (`**bold**,` is bold then plain).
type Word = Vec<Span>;

/// The words of `spans`, each with the number of spaces before it (kept between words on one
/// line, and before the first word, so indentation and aligned columns survive; dropped where a
/// line breaks).
fn words_of(spans: &[Span]) -> Vec<(usize, Word)> {
    let mut words = Vec::new();
    let mut word: Word = Vec::new();
    let mut gap = 0;
    for span in spans {
        for c in span.text.chars() {
            if c == ' ' {
                if !word.is_empty() {
                    words.push((gap, std::mem::take(&mut word)));
                    gap = 0;
                }
                gap += 1;
                continue;
            }
            match word.last_mut() {
                Some(last) if last.style == span.style => last.text.push(c),
                _ => word.push(Span {
                    style: span.style,
                    text: c.to_string(),
                }),
            }
        }
    }
    if !word.is_empty() {
        words.push((gap, word));
    }
    words
}

/// Split a word wider than `room` into pieces of at most `room` columns.
fn split_word(word: Word, room: usize) -> Vec<Word> {
    let room = room.max(1);
    let mut pieces = Vec::new();
    let mut piece: Word = Vec::new();
    let mut used = 0;
    for span in word {
        let mut text = String::new();
        for c in span.text.chars() {
            let w = c.width().unwrap_or(0);
            if used + w > room && used > 0 {
                if !text.is_empty() {
                    piece.push(Span {
                        style: span.style,
                        text: std::mem::take(&mut text),
                    });
                }
                pieces.push(std::mem::take(&mut piece));
                used = 0;
            }
            text.push(c);
            used += w;
        }
        if !text.is_empty() {
            piece.push(Span {
                style: span.style,
                text,
            });
        }
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
}

/// `spans` soft-wrapped to `width` columns: the first line starts with `first`, the rest with
/// `rest`. Words wider than a whole line are split. Always at least one line.
#[must_use]
pub fn wrap(spans: &[Span], width: usize, first: &[Span], rest: &[Span]) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut line: Line = first.to_vec();
    let mut used = line_width(first);
    let mut fresh = true;
    let mut at_start = true;
    let mut queue: std::collections::VecDeque<(usize, Word)> = words_of(spans).into();
    while let Some((gap, mut word)) = queue.pop_front() {
        let mut word_width: usize = line_width(&word);
        // Spaces before a word are kept, except where a wrapped line begins.
        let space = if fresh && !at_start { 0 } else { gap };
        if used + space + word_width > width {
            if !fresh {
                lines.push(std::mem::replace(&mut line, rest.to_vec()));
                used = line_width(rest);
                fresh = true;
                at_start = false;
                queue.push_front((gap, word));
                continue;
            }
            let room = width.saturating_sub(used + space).max(8);
            if word_width > room {
                let mut pieces = split_word(word, room).into_iter();
                word = pieces.next().unwrap_or_default();
                word_width = line_width(&word);
                for piece in pieces.rev() {
                    queue.push_front((0, piece));
                }
            }
        }
        if space > 0 {
            line.push(Span {
                style: Style::PLAIN,
                text: " ".repeat(space),
            });
        }
        used += space + word_width;
        line.extend(word);
        fresh = false;
        at_start = false;
    }
    lines.push(line);
    lines
}

/// `spans` cut to `room` columns, the last piece ending in `…` when cut.
#[must_use]
pub fn fit(spans: &[Span], room: usize) -> Line {
    let mut line = Vec::new();
    let mut used = 0;
    for span in spans {
        let w = width_of(&span.text);
        if used + w <= room {
            used += w;
            line.push(span.clone());
        } else {
            line.push(Span {
                style: span.style,
                text: truncate(&span.text, room.saturating_sub(used)),
            });
            break;
        }
    }
    line
}

/// Styles of the Markdown renderer.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// Ordinary prose.
    pub text: Style,
    /// Headings.
    pub heading: Style,
    /// Inline code and code blocks.
    pub code: Style,
    /// Box, bullets, quote bars, rules and table borders.
    pub frame: Style,
    /// Link targets.
    pub link: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::PLAIN,
            heading: Style::fg(Color::White).bold(),
            code: Style::fg(Color::Cyan),
            frame: Style::fg(Color::Gray),
            link: Style::fg(Color::Blue).underlined(),
        }
    }
}

/// Inline Markdown of one line as spans. In plain mode the text is kept verbatim.
#[must_use]
pub fn inline(text: &str, base: Style, theme: &Theme, pretty: bool) -> Vec<Span> {
    if !pretty {
        return vec![Span::new(base, text)];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    let flush = |literal: &mut String, spans: &mut Vec<Span>| {
        if !literal.is_empty() {
            spans.push(Span::new(base, literal));
            literal.clear();
        }
    };
    let find = |from: usize, pattern: &[char]| -> Option<usize> {
        (from..chars.len().saturating_sub(pattern.len() - 1))
            .find(|&at| chars[at..at + pattern.len()] == *pattern)
    };
    while i < chars.len() {
        let c = chars[i];
        let previous_alnum = i > 0 && chars[i - 1].is_alphanumeric();
        if c == '`' {
            let run = chars[i..].iter().take_while(|&&c| c == '`').count();
            let fence = vec!['`'; run];
            if let Some(end) = find(i + run, &fence) {
                flush(&mut literal, &mut spans);
                let inner: String = chars[i + run..end].iter().collect();
                let inner = if inner.starts_with(' ') && inner.ends_with(' ') && inner.len() > 2 {
                    inner[1..inner.len() - 1].to_owned()
                } else {
                    inner
                };
                spans.push(Span::new(theme.code, &inner));
                i = end + run;
                continue;
            }
        }
        if (c == '*' || c == '_') && chars.get(i + 1) == Some(&c) && !(c == '_' && previous_alnum) {
            let marker = [c, c];
            if let Some(end) = find(i + 2, &marker).filter(|&end| end > i + 2) {
                flush(&mut literal, &mut spans);
                let inner: String = chars[i + 2..end].iter().collect();
                spans.extend(inline(&inner, base.bold(), theme, true));
                i = end + 2;
                continue;
            }
        }
        if (c == '*' || c == '_')
            && !(c == '_' && previous_alnum)
            && chars
                .get(i + 1)
                .is_some_and(|next| !next.is_whitespace() && *next != c)
        {
            let end = (i + 1..chars.len()).find(|&at| {
                chars[at] == c
                    && !chars[at - 1].is_whitespace()
                    && chars.get(at + 1) != Some(&c)
                    && !(c == '_' && chars.get(at + 1).is_some_and(|next| next.is_alphanumeric()))
            });
            if let Some(end) = end {
                flush(&mut literal, &mut spans);
                let inner: String = chars[i + 1..end].iter().collect();
                spans.extend(inline(&inner, base.italic(), theme, true));
                i = end + 1;
                continue;
            }
        }
        if c == '[' {
            if let Some(close) = (i + 1..chars.len()).find(|&at| chars[at] == ']') {
                if chars.get(close + 1) == Some(&'(') {
                    if let Some(end) = (close + 2..chars.len()).find(|&at| chars[at] == ')') {
                        flush(&mut literal, &mut spans);
                        let label: String = chars[i + 1..close].iter().collect();
                        let target: String = chars[close + 2..end].iter().collect();
                        spans.extend(inline(&label, base.underlined(), theme, true));
                        if target != label {
                            spans.push(Span::new(base, " "));
                            spans.push(Span::new(theme.link, &format!("({target})")));
                        }
                        i = end + 1;
                        continue;
                    }
                }
            }
        }
        literal.push(c);
        i += 1;
    }
    flush(&mut literal, &mut spans);
    spans
}

fn leading_columns(line: &str) -> usize {
    line.chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
}

/// `(indent, marker, rest)` of a list item line.
fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = leading_columns(line);
    let trimmed = line.trim_start();
    let mut chars = trimmed.char_indices();
    let (_, first) = chars.next()?;
    if matches!(first, '-' | '*' | '+') {
        let rest = trimmed[1..].strip_prefix(' ')?;
        return Some((indent, first.to_string(), rest.trim_start()));
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 9 {
        return None;
    }
    let after = &trimmed[digits..];
    let punctuation = after.chars().next()?;
    if !matches!(punctuation, '.' | ')') {
        return None;
    }
    let rest = after[1..].strip_prefix(' ')?;
    Some((indent, trimmed[..=digits].to_owned(), rest.trim_start()))
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if leading_columns(line) > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if rest.is_empty() {
        return Some((level, ""));
    }
    rest.strip_prefix(' ')
        .map(|text| (level, text.trim().trim_end_matches('#').trim_end()))
}

fn is_rule(line: &str) -> bool {
    let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    compact.len() >= 3
        && (compact.chars().all(|c| c == '-')
            || compact.chars().all(|c| c == '*')
            || compact.chars().all(|c| c == '_'))
}

fn fence(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if leading_columns(line) > 3 {
        return None;
    }
    for marker in ['`', '~'] {
        let run = trimmed.chars().take_while(|c| *c == marker).count();
        if run >= 3 {
            let info = trimmed[run..].trim();
            if marker == '`' && info.contains('`') {
                return None;
            }
            return Some((marker.to_string().repeat(run), info.to_owned()));
        }
    }
    None
}

fn table_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    let inner = trimmed.strip_prefix('|').unwrap_or(trimmed);
    let inner = inner.strip_suffix('|').unwrap_or(inner);
    inner
        .split('|')
        .map(|cell| cell.trim().to_owned())
        .collect()
}

fn is_table_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':')) && cell.contains('-')
        })
}

/// Where Markdown output goes: the total width, and the prefixes of the first and later lines.
pub struct Frame<'a> {
    /// Total columns, prefixes included.
    pub width: usize,
    /// Prefix of the very first output line.
    pub first: &'a [Span],
    /// Prefix of every later line.
    pub rest: &'a [Span],
}

struct Out<'a> {
    frame: &'a Frame<'a>,
    lines: Vec<Line>,
}

impl Out<'_> {
    fn prefix(&self) -> Line {
        if self.lines.is_empty() {
            self.frame.first.to_vec()
        } else {
            self.frame.rest.to_vec()
        }
    }

    /// Wrap `spans` after a lead (`lead` on the first line, `hang` on its continuations).
    fn wrapped(&mut self, spans: &[Span], lead: &[Span], hang: &[Span]) {
        let mut first = self.prefix();
        first.extend_from_slice(lead);
        let mut rest = self.frame.rest.to_vec();
        rest.extend_from_slice(hang);
        let wrapped = wrap(spans, self.frame.width, &first, &rest);
        self.lines.extend(wrapped);
    }

    /// One line, cut to the width, never wrapped.
    fn exact(&mut self, mut spans: Line) {
        let mut line = self.prefix();
        let room = self.frame.width.saturating_sub(line_width(&line));
        let mut used = 0;
        for span in spans.drain(..) {
            let w = width_of(&span.text);
            if used + w <= room {
                used += w;
                line.push(span);
            } else {
                let text = truncate(&span.text, room.saturating_sub(used));
                line.push(Span {
                    style: span.style,
                    text,
                });
                break;
            }
        }
        self.lines.push(line);
    }
}

fn spaces(count: usize) -> Span {
    Span::new(Style::PLAIN, &" ".repeat(count))
}

/// Render Markdown `text` into `frame`. Always returns at least one line.
#[must_use]
pub fn markdown(text: &str, frame: &Frame<'_>, theme: &Theme, pretty: bool) -> Vec<Line> {
    let mut out = Out {
        frame,
        lines: Vec::new(),
    };
    let source: Vec<&str> = text.trim_matches('\n').split('\n').collect();
    let mut index = 0;
    let mut blank = false;
    while index < source.len() {
        let raw = source[index].trim_end_matches('\r');
        index += 1;
        if raw.trim().is_empty() {
            if !blank && !out.lines.is_empty() {
                let line = out.prefix();
                out.lines.push(line);
            }
            blank = true;
            continue;
        }
        blank = false;
        if let Some((marker, info)) = fence(raw) {
            let mut head = vec![Span::new(theme.frame, "╭─")];
            if !info.is_empty() {
                head.push(Span::new(theme.frame, &format!(" {info}")));
            }
            out.exact(head);
            let gutter = Span::new(theme.frame, "│ ");
            while index < source.len() {
                let code = source[index].trim_end_matches('\r');
                index += 1;
                if code.trim_start().starts_with(&marker)
                    && code
                        .trim()
                        .chars()
                        .all(|c| c == marker.chars().next().unwrap_or('`'))
                {
                    break;
                }
                let expanded = code.replace('\t', "    ");
                let body = vec![Span::new(theme.code, &expanded)];
                // Code keeps its spacing: hard-split long lines instead of reflowing words.
                let mut first = out.prefix();
                first.push(gutter.clone());
                let room = frame.width.saturating_sub(line_width(&first)).max(8);
                let mut pieces = split_word(body, room);
                if pieces.is_empty() {
                    pieces.push(Vec::new());
                }
                for (number, piece) in pieces.into_iter().enumerate() {
                    let mut line = if number == 0 {
                        first.clone()
                    } else {
                        let mut line = frame.rest.to_vec();
                        line.push(Span::new(theme.frame, "│ "));
                        line
                    };
                    line.extend(piece);
                    out.lines.push(line);
                }
            }
            out.exact(vec![Span::new(theme.frame, "╰─")]);
            continue;
        }
        if let Some((level, title)) = heading(raw) {
            let style = if level <= 2 {
                theme.heading.underlined()
            } else {
                theme.heading
            };
            let spans = if pretty {
                inline(title, style, theme, true)
            } else {
                vec![Span::new(style, raw.trim())]
            };
            out.wrapped(&spans, &[], &[]);
            continue;
        }
        if is_rule(raw) {
            let used = line_width(&out.prefix());
            let rule = "─".repeat(frame.width.saturating_sub(used).clamp(3, 80));
            out.exact(vec![Span::new(theme.frame, &rule)]);
            continue;
        }
        if raw.trim_start().starts_with('|') {
            let mut rows = vec![table_cells(raw)];
            while index < source.len() && source[index].trim_start().starts_with('|') {
                rows.push(table_cells(source[index]));
                index += 1;
            }
            table(&mut out, &rows, theme, pretty);
            continue;
        }
        if let Some((indent, marker, rest)) = list_item(raw) {
            let glyph = if marker.len() == 1 && !marker.starts_with(|c: char| c.is_ascii_digit()) {
                match indent / 2 {
                    0 => "•",
                    1 => "◦",
                    _ => "▪",
                }
                .to_owned()
            } else {
                marker
            };
            let lead = vec![spaces(indent), Span::new(theme.frame, &glyph), spaces(1)];
            let hang = vec![spaces(indent + width_of(&glyph) + 1)];
            let spans = inline(rest, theme.text, theme, pretty);
            out.wrapped(&spans, &lead, &hang);
            continue;
        }
        if let Some(quoted) = raw.trim_start().strip_prefix('>') {
            let quoted = quoted.strip_prefix(' ').unwrap_or(quoted);
            let bar = vec![Span::new(theme.frame, "▎ ")];
            let spans = inline(quoted, theme.text.italic(), theme, pretty);
            out.wrapped(&spans, &bar, &bar);
            continue;
        }
        let indent = leading_columns(raw).min(frame.width / 2);
        let lead = if indent > 0 {
            vec![spaces(indent)]
        } else {
            Vec::new()
        };
        let spans = inline(raw.trim_start(), theme.text, theme, pretty);
        out.wrapped(&spans, &lead, &lead);
    }
    if out.lines.is_empty() {
        out.lines.push(frame.first.to_vec());
    }
    out.lines
}

fn table(out: &mut Out<'_>, rows: &[Vec<String>], theme: &Theme, pretty: bool) {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let cell_spans = |cell: &str| inline(cell, theme.text, theme, pretty);
    let mut widths = vec![0usize; columns];
    for row in rows.iter().filter(|row| !is_table_separator(row)) {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(line_width(&cell_spans(cell)));
        }
    }
    let total: usize = widths.iter().sum::<usize>() + 3 * columns.saturating_sub(1);
    let room = out.frame.width.saturating_sub(line_width(&out.prefix()));
    if total > room {
        // Too wide to align: keep the rows as written, each cut to the width.
        for row in rows {
            let text = format!("| {} |", row.join(" | "));
            out.exact(vec![Span::new(theme.text, &text)]);
        }
        return;
    }
    for (number, row) in rows.iter().enumerate() {
        let mut line: Line = Vec::new();
        if is_table_separator(row) {
            let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            line.push(Span::new(theme.frame, &rule.join("─┼─")));
            out.exact(line);
            continue;
        }
        let header = number == 0 && rows.get(1).is_some_and(|next| is_table_separator(next));
        for (column, width) in widths.iter().enumerate() {
            if column > 0 {
                line.push(Span::new(theme.frame, " │ "));
            }
            let cell = row.get(column).map_or("", String::as_str);
            let mut spans = cell_spans(cell);
            if header {
                for span in &mut spans {
                    span.style = span.style.bold();
                }
            }
            let used = line_width(&spans);
            line.extend(spans);
            line.push(spaces(width.saturating_sub(used)));
        }
        out.exact(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|line| emit(line, false)).collect()
    }

    fn render(text: &str, width: usize, pretty: bool) -> Vec<String> {
        let first = [Span::new(Style::PLAIN, "> ")];
        let rest = [Span::new(Style::PLAIN, "  ")];
        let frame = Frame {
            width,
            first: &first,
            rest: &rest,
        };
        plain(&markdown(text, &frame, &Theme::default(), pretty))
    }

    #[test]
    fn prose_keeps_line_breaks_and_wraps_to_the_width() {
        let lines = render("one two three four five six seven\nnext line", 16, true);
        assert_eq!(
            lines,
            vec![
                "> one two three",
                "  four five six",
                "  seven",
                "  next line"
            ]
        );
        assert!(lines.iter().all(|line| width_of(line) <= 16));
    }

    #[test]
    fn lists_hang_and_nest() {
        let lines = render(
            "- alpha beta gamma delta\n  - inner item here\n1. first one",
            18,
            true,
        );
        assert_eq!(
            lines,
            vec![
                "> • alpha beta",
                "    gamma delta",
                "    ◦ inner item",
                "      here",
                "  1. first one",
            ]
        );
    }

    #[test]
    fn code_blocks_are_boxed_and_never_reflowed() {
        let lines = render("```rust\nfn  main() {}\n```\nafter", 30, true);
        assert_eq!(
            lines,
            vec!["> ╭─ rust", "  │ fn  main() {}", "  ╰─", "  after"]
        );
        let long = render(&format!("```\n{}\n```", "x".repeat(20)), 12, true);
        assert_eq!(long[1], "  │ xxxxxxxx");
        assert_eq!(long[2], "  │ xxxxxxxx");
    }

    #[test]
    fn headings_and_inline_markers_depend_on_mode() {
        assert_eq!(
            render(
                "## Status\n**bold** and `code` and [link](http://x)",
                60,
                true
            ),
            vec!["> Status", "  bold and code and link (http://x)"]
        );
        assert_eq!(
            render("## Status\n**bold** and `code`", 60, false),
            vec!["> ## Status", "  **bold** and `code`"]
        );
    }

    #[test]
    fn snake_case_and_lone_stars_stay_literal() {
        assert_eq!(
            render("use snake_case_name and 2 * 3 * 4", 60, true),
            vec!["> use snake_case_name and 2 * 3 * 4"]
        );
    }

    #[test]
    fn tables_align_when_they_fit() {
        assert_eq!(
            render("| a | bb |\n|---|---|\n| ccc | d |", 40, true),
            vec!["> a   │ bb", "  ────┼───", "  ccc │ d"]
        );
        let narrow = render("| aaaaaaaaaa | bbbbbbbbbb |\n|---|---|", 12, true);
        assert_eq!(narrow[0], "> | aaaaaaa…");
    }

    #[test]
    fn overlong_words_are_split_and_blank_runs_collapse() {
        let lines = render(&format!("{}\n\n\n\nend", "y".repeat(30)), 12, true);
        assert_eq!(
            lines,
            vec!["> yyyyyyyyyy", "  yyyyyyyyyy", "  yyyyyyyyyy", "", "  end"]
        );
    }

    #[test]
    fn emit_adds_sgr_only_when_colored() {
        let line = vec![
            Span::new(Style::fg(Color::Green).bold(), "ok"),
            Span::new(Style::PLAIN, " done "),
        ];
        assert_eq!(emit(&line, false), "ok done");
        assert_eq!(emit(&line, true), "\u{1b}[1;32mok\u{1b}[0m done");
        assert_eq!(
            Span::new(Style::PLAIN, "a\u{1b}[31mb").text,
            "a\u{fffd}[31mb"
        );
    }
}
