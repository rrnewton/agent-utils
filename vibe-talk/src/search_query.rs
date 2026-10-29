//! The search language, and the one rule for whether a message matches it.
//!
//! `#521 server-side-search` asks for one language giving exactly the same results on the page and
//! on the server. The page has a twin of this module in `web/src/core/search/`
//! (`searchQueryParse`, `searchQueryMatches`, `searchFold`, `searchUpstreamPlan`), and both are held
//! to the conformance corpus in `tests/fixtures/search_query.json`: a change to either that the
//! other does not make fails one of the two suites. The rules below are the specification; the corpus pins every edge of it.
//!
//! **Settled by the owner (2026-10-27):** Gmail's syntax as the base (implicit AND, upper-case
//! `OR`, `-term`, `"phrase"`, parentheses) plus the field operators the chat services share
//! (`from:`, `in:`, `before:`, `after:`, `has:`), over OUR matching: a term matches a message when
//! its folded text is a SUBSTRING of the message's folded text, so matching is case- and
//! accent-insensitive and finds a term inside a word (`coord` finds "coordinator", `andoff` finds
//! "handoff"). Upstream search services match words (Slack's, measured, whole words and their
//! inflections, not a longer word's start or inside), so they can only ever offer
//! candidates; see [`upstream_plan`].
//!
//! # Folding ([`fold`])
//!
//! Both sides of every comparison are folded the same way, in this order:
//!
//! 1. Unicode NFD, every combining DIACRITIC dropped (the blocks [`is_diacritic`] lists: Combining
//!    Diacritical Marks and its supplements), then NFC: `café` is `cafe`, `Łódź`'s acute and
//!    `naïve`'s diaeresis go, and Hangul syllables come back whole. Only those blocks: the vowel
//!    signs of Indic and Thai scripts, Hebrew and Arabic points and Japanese voiced marks are
//!    combining marks too, and dropping them would make `कि` find every `क`.
//! 2. Lower-casing (Unicode's full mapping, as Rust's `str::to_lowercase` and JavaScript's
//!    `toLowerCase` both do).
//! 3. [`FOLD_TABLE`]: every difference between lower-casing and Unicode's full case folding that
//!    survives step 1 (`ς`→`σ`, `ß`→`ss`, `µ`→`μ`, ligatures…; derived from Python's
//!    `str.casefold`, Unicode 15, where Cherokee alone differs in direction but not in which
//!    letters it joins), plus letters whose mark is part of the letter (`ø`→`o`, `ł`→`l`, `đ`→`d`,
//!    `ħ`→`h`, `ŧ`→`t`, `ı`→`i`). Final sigma is the reason this is not optional: lower-casing
//!    maps `Σ` to `ς` at the end of a word and `σ` elsewhere.
//! 4. Every run of white space (Unicode `White_Space`, listed in [`is_space`]) becomes one space.
//!
//! # Syntax ([`parse`])
//!
//! ```text
//! query  := and
//! and    := or ( "AND"? or )*          juxtaposition is AND; a bare upper-case AND is allowed
//! or     := unary ( "OR" unary )*      OR binds its two neighbours
//! unary  := "-" unary | atom           "-" directly before a term, phrase, field or group
//! atom   := "(" and ")"? | phrase | field | word
//! phrase := '"' text ( '"' | end )     an unclosed quote runs to the end, as the page did before
//! field  := name ":" ( value | phrase )   name is from, in, before, after or has (any case)
//! ```
//!
//! - `a b OR c` is `a AND (b OR c)`: OR takes the term on each side. (The help pages of the products
//!   this syntax follows state no precedence; parentheses settle either reading.)
//! - `OR` and `AND` are operators only in capitals and only as whole words; `or` is a word. An
//!   operator with nothing on one side is ignored, so a query being typed (`a OR`) searches `a`.
//! - `(` opens a group only at the start of a token, and `)` closes one only while a group is open,
//!   at the start or the end of a token: `f(x)` and `:)` are words outside a group. An unclosed
//!   group runs to the end.
//! - A quote ends the word it touches: `a"b c"` is the word `a` and the phrase `b c`.
//! - A lone `-` is a word; `--x` is `x`. A `-` before nothing (an empty phrase, `()`) is dropped.
//! - Phrases and field values are trimmed of spaces after folding (only spaces: U+FEFF is text).
//! - A field whose value is not valid is an ordinary word, as Gmail treats an unknown operator:
//!   `from:` needs a value; `in:` takes `thread` or `main`; `has:` takes `link`; `before:` and
//!   `after:` take a calendar date `YYYY-MM-DD`, `YYYY/MM/DD` or `MM/DD/YYYY`.
//!
//! # Matching ([`Query::matches`])
//!
//! - A word or phrase matches when its folded text is a substring of the folded haystack: the
//!   author's display name, the text the message draws, and every link address, one a line.
//! - `from:x` matches when `x` is the author's id exactly, or its folded form is a substring of the
//!   author's folded display name.
//! - `in:thread` matches a reply; `in:main` a message that is not one.
//! - `before:D` matches a message created before the start of day D in the reader's time zone, and
//!   `after:D` one created at or after it (Gmail's reading: `after:` includes its day, `before:`
//!   does not). The caller resolves days, because only it knows the zone.
//! - `has:link` matches a message with a link.
//! - A query that is only negations matches everything they do not exclude; an empty query (no
//!   term at all) is no search, and [`Query::is_empty`] says so.
//!
//! The unit is the message. Whether a combined row on the page shows when any of its messages
//! matches is the page's decision, outside this module.

use unicode_normalization::UnicodeNormalization as _;

/// A label for the folding rules, stored beside anything folded with them.
pub const FOLD_VERSION: &str = "fold-2";

/// What lower-casing leaves for full case folding, and letters whose mark is not a combining
/// character. Mirrored, entry for entry, by `SEARCH_FOLD_TABLE` in
/// `web/src/core/search/language.js` (a test reads that table back and compares).
pub const FOLD_TABLE: &[(char, &str)] = &[
    ('ß', "ss"),
    ('ς', "σ"),
    ('ſ', "s"),
    ('ϐ', "β"),
    ('ϑ', "θ"),
    ('ϕ', "φ"),
    ('ϖ', "π"),
    ('ϰ', "κ"),
    ('ϱ', "ρ"),
    ('ϵ', "ε"),
    ('ﬀ', "ff"),
    ('ﬁ', "fi"),
    ('ﬂ', "fl"),
    ('ﬃ', "ffi"),
    ('ﬄ', "ffl"),
    ('ﬅ', "st"),
    ('ﬆ', "st"),
    ('ı', "i"),
    ('ø', "o"),
    ('ł', "l"),
    ('đ', "d"),
    ('ħ', "h"),
    ('ŧ', "t"),
    ('µ', "μ"),
    ('ŉ', "ʼn"),
    ('և', "եւ"),
    ('ẚ', "aʾ"),
    ('ﬓ', "մն"),
    ('ﬔ', "մե"),
    ('ﬕ', "մի"),
    ('ﬖ', "վն"),
    ('ﬗ', "մխ"),
    ('ᲀ', "в"),
    ('ᲁ', "д"),
    ('ᲂ', "о"),
    ('ᲃ', "с"),
    ('ᲄ', "т"),
    ('ᲅ', "т"),
    ('ᲆ', "ъ"),
    ('ᲇ', "ѣ"),
    ('ᲈ', "ꙋ"),
];

/// The combining diacritics [`fold`] drops: Combining Diacritical Marks, its Extended and
/// Supplement blocks, Combining Diacritical Marks for Symbols, and Combining Half Marks.
#[must_use]
pub fn is_diacritic(c: char) -> bool {
    matches!(
        c,
        '\u{300}'..='\u{36f}'
            | '\u{1ab0}'..='\u{1aff}'
            | '\u{1dc0}'..='\u{1dff}'
            | '\u{20d0}'..='\u{20ff}'
            | '\u{fe20}'..='\u{fe2f}'
    )
}

/// Unicode `White_Space`, the separators of a query and the runs [`fold`] collapses. Rust's
/// `char::is_whitespace` is exactly this set; it is spelled out so the page's copy can be checked
/// against it.
#[must_use]
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}'..='\u{d}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// Fold text for comparison: diacritics dropped, lower-cased, [`FOLD_TABLE`] applied, white space
/// collapsed. See the module documentation for the order and why.
#[must_use]
pub fn fold(text: &str) -> String {
    let unmarked: String = text.nfd().filter(|c| !is_diacritic(*c)).nfc().collect();
    let lower = unmarked.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_space = false;
    for c in lower.chars() {
        if is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
            continue;
        }
        in_space = false;
        match FOLD_TABLE.iter().find(|(from, _)| *from == c) {
            Some((_, to)) => out.push_str(to),
            None => out.push(c),
        }
    }
    out
}

/// A calendar day, as `before:` and `after:` name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day {
    /// The year.
    pub year: i32,
    /// The month, 1 to 12.
    pub month: u32,
    /// The day of the month, 1 to 31.
    pub day: u32,
}

impl Day {
    /// Read `YYYY-MM-DD`, `YYYY/MM/DD` or `MM/DD/YYYY` (months and days may have one digit), when
    /// it names a real date.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let separator = if text.contains('-') { '-' } else { '/' };
        let parts: Vec<&str> = text.split(separator).collect();
        if parts.len() != 3
            || parts
                .iter()
                .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
        {
            return None;
        }
        let (year, month, day) = if parts[0].len() == 4 {
            (parts[0], parts[1], parts[2])
        } else if parts[2].len() == 4 && separator == '/' {
            (parts[2], parts[0], parts[1])
        } else {
            return None;
        };
        if month.len() > 2 || day.len() > 2 {
            return None;
        }
        let day = Self {
            year: year.parse().ok()?,
            month: month.parse().ok()?,
            day: day.parse().ok()?,
        };
        (day.month >= 1 && day.month <= 12 && day.day >= 1 && day.day <= day.month_length())
            .then_some(day)
    }

    fn month_length(self) -> u32 {
        match self.month {
            2 if (self.year % 4 == 0 && self.year % 100 != 0) || self.year % 400 == 0 => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
    #[must_use]
    pub fn days_since_epoch(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let month = i64::from(self.month);
        let day_of_year =
            (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// The start of this day, in milliseconds since the epoch, in a zone `offset_minutes` east of
    /// UTC. The corpus resolves days this way; the server resolves them in the reader's named zone.
    #[must_use]
    pub fn start_ms_at_offset(self, offset_minutes: i64) -> i64 {
        self.days_since_epoch() * 86_400_000 - offset_minutes * 60_000
    }
}

impl std::fmt::Display for Day {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Where `in:` looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Thread replies.
    Thread,
    /// Messages that are not thread replies.
    Main,
}

/// One node of a parsed query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// A word, folded.
    Word(String),
    /// A quoted phrase, folded.
    Phrase(String),
    /// `from:`: the value as typed (for an exact id) and folded (for the name).
    From {
        /// As typed, compared with the author's id.
        raw: String,
        /// Folded, looked for in the author's name.
        folded: String,
    },
    /// `in:thread` or `in:main`.
    In(Place),
    /// `before:`.
    Before(Day),
    /// `after:`.
    After(Day),
    /// `has:link`.
    HasLink,
    /// `-x`.
    Not(Box<Node>),
    /// Every child.
    And(Vec<Node>),
    /// Any child.
    Or(Vec<Node>),
}

impl Node {
    /// The node as the corpus writes it: `["word", "x"]`, `["and", …]` and so on.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Word(text) => json!(["word", text]),
            Self::Phrase(text) => json!(["phrase", text]),
            Self::From { folded, .. } => json!(["from", folded]),
            Self::In(Place::Thread) => json!(["in", "thread"]),
            Self::In(Place::Main) => json!(["in", "main"]),
            Self::Before(day) => json!(["before", day.to_string()]),
            Self::After(day) => json!(["after", day.to_string()]),
            Self::HasLink => json!(["has", "link"]),
            Self::Not(inner) => json!(["not", inner.to_json()]),
            Self::And(children) | Self::Or(children) => {
                let mut list = vec![json!(if matches!(self, Self::And(_)) {
                    "and"
                } else {
                    "or"
                })];
                list.extend(children.iter().map(Self::to_json));
                serde_json::Value::Array(list)
            }
        }
    }
}

/// A message, as the matcher sees it. Build it with [`Subject::new`], which folds once.
#[derive(Clone, Debug)]
pub struct Subject {
    /// The folded haystack: author, drawn text and link addresses.
    pub text: String,
    /// The folded author display name.
    pub author: String,
    /// The author's id, as the provider gives it.
    pub author_id: String,
    /// Creation time, milliseconds since the epoch.
    pub created_ms: i64,
    /// Whether it is a thread reply.
    pub in_thread: bool,
    /// Whether it has a link.
    pub has_link: bool,
}

impl Subject {
    /// Fold a message for matching. `haystack` is the author's display name, the drawn text and
    /// the link addresses, joined by newlines (`haystack` below builds it).
    #[must_use]
    pub fn new(
        haystack: &str,
        author: &str,
        author_id: &str,
        created_ms: i64,
        in_thread: bool,
        has_link: bool,
    ) -> Self {
        Self {
            text: fold(haystack),
            author: fold(author),
            author_id: author_id.to_owned(),
            created_ms,
            in_thread,
            has_link,
        }
    }
}

/// The haystack of one message: the author's display name, the drawn text, and each link address,
/// one a line.
#[must_use]
pub fn haystack(author: &str, drawn_text: &str, hrefs: &[String]) -> String {
    let mut text = String::with_capacity(author.len() + drawn_text.len() + 1);
    text.push_str(author);
    text.push('\n');
    text.push_str(drawn_text);
    for href in hrefs {
        text.push('\n');
        text.push_str(href);
    }
    text
}

/// A parsed query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    root: Option<Node>,
}

impl Query {
    /// The parsed tree; `None` for a query with no term at all.
    #[must_use]
    pub fn root(&self) -> Option<&Node> {
        self.root.as_ref()
    }

    /// Whether the query has no term at all, and so is no search.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// Whether `subject` matches. `day_start_ms` gives the start of a day in the reader's zone.
    /// An empty query matches everything.
    pub fn matches(&self, subject: &Subject, day_start_ms: &dyn Fn(Day) -> i64) -> bool {
        self.root
            .as_ref()
            .is_none_or(|root| node_matches(root, subject, day_start_ms))
    }

    /// Every positive word and phrase, folded, in order: what an index can be asked for.
    #[must_use]
    pub fn positive_terms(&self) -> Vec<String> {
        fn walk(node: &Node, out: &mut Vec<String>) {
            match node {
                Node::Word(t) | Node::Phrase(t) => out.push(t.clone()),
                Node::And(children) | Node::Or(children) => {
                    children.iter().for_each(|c| walk(c, out));
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            walk(root, &mut out);
        }
        out
    }
}

fn node_matches(node: &Node, subject: &Subject, day_start_ms: &dyn Fn(Day) -> i64) -> bool {
    match node {
        Node::Word(term) | Node::Phrase(term) => subject.text.contains(term.as_str()),
        Node::From { raw, folded } => {
            subject.author_id == *raw || subject.author.contains(folded.as_str())
        }
        Node::In(Place::Thread) => subject.in_thread,
        Node::In(Place::Main) => !subject.in_thread,
        Node::Before(day) => subject.created_ms < day_start_ms(*day),
        Node::After(day) => subject.created_ms >= day_start_ms(*day),
        Node::HasLink => subject.has_link,
        Node::Not(inner) => !node_matches(inner, subject, day_start_ms),
        Node::And(children) => children
            .iter()
            .all(|c| node_matches(c, subject, day_start_ms)),
        Node::Or(children) => children
            .iter()
            .any(|c| node_matches(c, subject, day_start_ms)),
    }
}

/// One token of a query.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    /// Something that folded to nothing (an empty phrase): no term, but it is what a `-` before
    /// it applies to.
    Empty,
    Open,
    Close,
    Not,
    Or,
    And,
    Atom(Node),
}

/// Split a query into tokens. See the module documentation for the rules.
fn tokens(query: &str) -> Vec<Token> {
    let chars: Vec<char> = query.chars().collect();
    let mut out = Vec::new();
    let mut depth = 0_usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_space(c) {
            i += 1;
        } else if c == '"' {
            let (text, next) = phrase_at(&chars, i);
            let text = folded_value(&text);
            i = next;
            out.push(if text.is_empty() {
                Token::Empty
            } else {
                Token::Atom(Node::Phrase(text))
            });
        } else if c == '(' {
            depth += 1;
            out.push(Token::Open);
            i += 1;
        } else if c == ')' && depth > 0 {
            depth -= 1;
            out.push(Token::Close);
            i += 1;
        } else if c == '-' && chars.get(i + 1).is_some_and(|n| !is_space(*n)) {
            out.push(Token::Not);
            i += 1;
        } else {
            let start = i;
            while i < chars.len() && !is_space(chars[i]) && chars[i] != '"' {
                i += 1;
            }
            let mut raw: String = chars[start..i].iter().collect();
            let mut closes = 0;
            while depth > 0 && raw.len() > 1 && raw.ends_with(')') {
                raw.pop();
                depth -= 1;
                closes += 1;
            }
            if raw == "OR" {
                out.push(Token::Or);
            } else if raw == "AND" {
                out.push(Token::And);
            } else if let Some(node) = field(&raw, &chars, &mut i, closes == 0) {
                out.push(Token::Atom(node));
            } else {
                let word = fold(&raw);
                out.push(if word.is_empty() {
                    Token::Empty
                } else {
                    Token::Atom(Node::Word(word))
                });
            }
            out.extend(std::iter::repeat_n(Token::Close, closes));
        }
    }
    out
}

/// The phrase opening at `chars[at]` (a quote): its text as typed, and the index after it.
fn phrase_at(chars: &[char], at: usize) -> (String, usize) {
    let mut end = at + 1;
    while end < chars.len() && chars[end] != '"' {
        end += 1;
    }
    let text: String = chars[at + 1..end].iter().collect();
    (text, (end + 1).min(chars.len()))
}

/// Folded and trimmed of spaces: a phrase's or a field value's text.
fn folded_value(text: &str) -> String {
    fold(text).trim_matches(' ').to_owned()
}

/// The field `raw` names, when it is one with a valid value. A field with no value directly
/// followed by a quote (`from:"Ada L"`) takes the phrase as its value, moving `*i` past it, when
/// `may_take_phrase` (no `)` was cut from the token).
fn field(raw: &str, chars: &[char], i: &mut usize, may_take_phrase: bool) -> Option<Node> {
    let (name, value) = raw.split_once(':')?;
    let name = name.to_ascii_lowercase();
    if !matches!(name.as_str(), "from" | "in" | "before" | "after" | "has") {
        return None;
    }
    let mut value = value.to_owned();
    let mut after_phrase = None;
    if value.is_empty() && may_take_phrase && chars.get(*i) == Some(&'"') {
        let (text, next) = phrase_at(chars, *i);
        value = text;
        after_phrase = Some(next);
    }
    let node = match name.as_str() {
        "from" => {
            let folded = folded_value(&value);
            (!folded.is_empty()).then(|| Node::From {
                raw: value.trim_matches(is_space).to_owned(),
                folded,
            })
        }
        "in" => match folded_value(&value).as_str() {
            "thread" => Some(Node::In(Place::Thread)),
            "main" => Some(Node::In(Place::Main)),
            _ => None,
        },
        "has" => (folded_value(&value) == "link").then_some(Node::HasLink),
        "before" => Day::parse(&value).map(Node::Before),
        _ => Day::parse(&value).map(Node::After),
    }?;
    if let Some(next) = after_phrase {
        *i = next;
    }
    Some(node)
}

/// Parse a query. Never fails: whatever cannot be read as syntax is read as words.
#[must_use]
pub fn parse(query: &str) -> Query {
    let tokens = tokens(query);
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
    };
    let root = parser.and(false);
    Query { root }
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    /// `and`, up to the end, or up to a `)` when `in_group`.
    fn and(&mut self, in_group: bool) -> Option<Node> {
        let mut items = Vec::new();
        loop {
            match self.peek() {
                None => break,
                Some(Token::Close) if in_group => break,
                // A stray operator where a term should start: skip it.
                Some(Token::And | Token::Or | Token::Close) => self.at += 1,
                Some(_) => {
                    if let Some(node) = self.or() {
                        items.push(node);
                    }
                }
            }
        }
        combine(items, true)
    }

    /// `or`: one `unary`, then `OR unary` while there are any.
    fn or(&mut self) -> Option<Node> {
        let mut items: Vec<Node> = self.unary().into_iter().collect();
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            if let Some(node) = self.unary() {
                items.push(node);
            }
        }
        combine(items, false)
    }

    /// `unary`: a `-` before a unary, or an atom or group. `None`, consuming nothing more than
    /// the `-`s, where neither starts.
    fn unary(&mut self) -> Option<Node> {
        match self.peek()? {
            Token::Not => {
                self.at += 1;
                self.unary().map(|inner| Node::Not(Box::new(inner)))
            }
            Token::Open => {
                self.at += 1;
                let inner = self.and(true);
                if self.peek() == Some(&Token::Close) {
                    self.at += 1;
                }
                inner
            }
            Token::Atom(node) => {
                let node = node.clone();
                self.at += 1;
                Some(node)
            }
            Token::Empty => {
                self.at += 1;
                None
            }
            Token::Close | Token::Or | Token::And => None,
        }
    }
}

/// One node from several: none is `None`, one is itself, more is an AND or OR, flattened.
fn combine(items: Vec<Node>, and: bool) -> Option<Node> {
    let mut flat = Vec::with_capacity(items.len());
    for item in items {
        match item {
            Node::And(children) if and => flat.extend(children),
            Node::Or(children) if !and => flat.extend(children),
            other => flat.push(other),
        }
    }
    match flat.len() {
        0 => None,
        1 => flat.pop(),
        _ if and => Some(Node::And(flat)),
        _ => Some(Node::Or(flat)),
    }
}

/// How a query can be sent to an upstream search service that matches words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamPlan {
    /// The upstream queries, each a list of terms to AND: the positive terms of one alternative
    /// of the query that such a service can find. Empty when nothing can be sent.
    pub queries: Vec<Vec<String>>,
    /// Why some alternative cannot be served upstream, so the cache must be scanned for it; `None`
    /// when every alternative has a sendable query.
    pub scan_reason: Option<ScanReason>,
}

/// Why a query needs a scan of what the server holds, not only upstream candidates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanReason {
    /// An alternative has no positive word or phrase at all (only negations or fields).
    NoTerm,
    /// An alternative's terms are none of them word-aligned: shorter than three characters, or
    /// holding a character that is not a letter or digit.
    NotWordAligned,
    /// More alternatives than [`MAX_UPSTREAM_QUERIES`].
    TooManyAlternatives,
}

impl ScanReason {
    /// The reason as the wire and the corpus spell it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoTerm => "no-term",
            Self::NotWordAligned => "not-word-aligned",
            Self::TooManyAlternatives => "too-many-alternatives",
        }
    }
}

/// The most upstream queries one search sends.
pub const MAX_UPSTREAM_QUERIES: usize = 4;

/// Whether a folded term is shaped like a word a word-based upstream search could find: at least
/// three letters or digits, and nothing else but the spaces between a phrase's words. Only the
/// query's shape: whether the term stands as a word in a given message is unknowable here, so a
/// term that passes can still be missed upstream (`coord` in "coordinator").
#[must_use]
pub fn word_aligned(term: &str) -> bool {
    term.chars().filter(|c| *c != ' ').count() >= 3
        && term
            .split(' ')
            .all(|word| !word.is_empty() && word.chars().all(char::is_alphanumeric))
}

/// How to ask a word-based upstream service for CANDIDATES for `query`.
///
/// Negations and fields are dropped (dropping a condition only widens the set), the rest is put
/// in disjunctive normal form, and each alternative sends its word-aligned terms. Upstream search
/// is never proof: a service that matches words misses a term inside or at the start of a longer
/// word (Slack's found 72% of this language's matches over 20 terms, 2026-10-27), so every hit is
/// matched again here, and only a scan of what the server holds says a message is absent. Terms
/// are sent folded; how the service treats accents is unmeasured. `scan_reason` names an
/// alternative upstream cannot narrow at all.
#[must_use]
pub fn upstream_plan(query: &Query) -> UpstreamPlan {
    let Some(root) = query.root() else {
        return UpstreamPlan {
            queries: Vec::new(),
            scan_reason: Some(ScanReason::NoTerm),
        };
    };
    let Some(alternatives) = dnf(root, MAX_UPSTREAM_QUERIES) else {
        return UpstreamPlan {
            queries: Vec::new(),
            scan_reason: Some(ScanReason::TooManyAlternatives),
        };
    };
    let mut queries: Vec<Vec<String>> = Vec::new();
    let mut scan_reason = None;
    for terms in alternatives {
        if terms.is_empty() {
            scan_reason.get_or_insert(ScanReason::NoTerm);
            continue;
        }
        let sendable: Vec<String> = terms.into_iter().filter(|t| word_aligned(t)).collect();
        if sendable.is_empty() {
            scan_reason.get_or_insert(ScanReason::NotWordAligned);
        } else if !queries.contains(&sendable) {
            queries.push(sendable);
        }
    }
    UpstreamPlan {
        queries,
        scan_reason,
    }
}

/// The positive terms of each alternative of `node` in disjunctive normal form, or `None` past
/// `cap` alternatives. A negation or a field contributes no term (it is true for the purpose of
/// finding candidates).
fn dnf(node: &Node, cap: usize) -> Option<Vec<Vec<String>>> {
    match node {
        Node::Word(t) | Node::Phrase(t) => Some(vec![vec![t.clone()]]),
        Node::Or(children) => {
            let mut out = Vec::new();
            for child in children {
                out.extend(dnf(child, cap)?);
                if out.len() > cap {
                    return None;
                }
            }
            Some(out)
        }
        Node::And(children) => {
            let mut out: Vec<Vec<String>> = vec![Vec::new()];
            for child in children {
                let child = dnf(child, cap)?;
                let mut next = Vec::with_capacity(out.len() * child.len());
                for left in &out {
                    for right in &child {
                        let mut both = left.clone();
                        both.extend(right.iter().cloned());
                        next.push(both);
                    }
                }
                if next.len() > cap {
                    return None;
                }
                out = next;
            }
            Some(out)
        }
        _ => Some(vec![Vec::new()]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    const CORPUS: &str = include_str!("../tests/fixtures/search_query.json");

    fn corpus() -> Value {
        serde_json::from_str(CORPUS).expect("the corpus is JSON")
    }

    fn cases<'a>(corpus: &'a Value, kind: &str) -> &'a Vec<Value> {
        corpus[kind]
            .as_array()
            .unwrap_or_else(|| panic!("corpus lacks {kind}"))
    }

    #[test]
    fn the_corpus_folds() {
        let corpus = corpus();
        for case in cases(&corpus, "fold") {
            let input = case["in"].as_str().expect("in");
            assert_eq!(fold(input), case["out"].as_str().expect("out"), "{case}");
        }
    }

    #[test]
    fn the_corpus_parses() {
        let corpus = corpus();
        for case in cases(&corpus, "parse") {
            let query = parse(case["query"].as_str().expect("query"));
            let tree = query.root().map_or(Value::Null, Node::to_json);
            assert_eq!(tree, case["tree"], "{case}");
        }
    }

    #[test]
    fn the_corpus_matches() {
        let corpus = corpus();
        for case in cases(&corpus, "match") {
            let message = &case["message"];
            let offset = case["tz_offset_minutes"].as_i64().unwrap_or(0);
            let hrefs: Vec<String> = message["links"]
                .as_array()
                .map(|l| {
                    l.iter()
                        .filter_map(|h| h.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let author = message["author"].as_str().unwrap_or("");
            let created = message["created"].as_str().expect("created");
            let created_ms = created
                .parse::<jiff::Timestamp>()
                .expect("created is RFC 3339")
                .as_millisecond();
            let subject = Subject::new(
                &haystack(author, message["text"].as_str().unwrap_or(""), &hrefs),
                author,
                message["author_id"].as_str().unwrap_or(""),
                created_ms,
                message["in_thread"].as_bool().unwrap_or(false),
                !hrefs.is_empty(),
            );
            let query = parse(case["query"].as_str().expect("query"));
            let got = query.matches(&subject, &|day| day.start_ms_at_offset(offset));
            assert_eq!(got, case["matches"].as_bool().expect("matches"), "{case}");
        }
    }

    #[test]
    fn the_corpus_plans_upstream() {
        let corpus = corpus();
        for case in cases(&corpus, "upstream") {
            let plan = upstream_plan(&parse(case["query"].as_str().expect("query")));
            let queries: Value = plan
                .queries
                .iter()
                .map(|q| Value::from(q.clone()))
                .collect();
            assert_eq!(queries, case["queries"], "{case}");
            let reason = plan
                .scan_reason
                .map_or(Value::Null, |r| Value::from(r.as_str()));
            assert_eq!(reason, case["scan"], "{case}");
        }
    }

    #[test]
    fn the_fold_table_and_the_space_set_are_the_pages() {
        // The page states its tables as source text; this reads them back from it and compares
        // them both ways, so neither copy can drift without this failing. The page's search
        // language is its own module (`#624 voice-page-modularity`).
        let page = include_str!("../web/src/core/search/language.js");
        let block = |start: &str, end: &str| -> String {
            let from = page
                .find(start)
                .unwrap_or_else(|| panic!("language.js lacks {start}"));
            let rest = &page[from + start.len()..];
            rest[..rest.find(end).expect("the block ends")].to_owned()
        };
        let table = block("const SEARCH_FOLD_TABLE = new Map([", "]);");
        let mut pages: Vec<(char, String)> = Vec::new();
        for entry in table.split('[').skip(1) {
            let entry = entry.split(']').next().expect("an entry");
            let parts: Vec<&str> = entry.split('"').collect();
            assert!(parts.len() >= 5, "an entry of SEARCH_FOLD_TABLE: {entry:?}");
            let mut from = parts[1].chars();
            pages.push((from.next().expect("a letter"), parts[3].to_owned()));
            assert_eq!(from.next(), None, "{entry:?}");
        }
        let ours: Vec<(char, String)> = FOLD_TABLE
            .iter()
            .map(|(from, to)| (*from, (*to).to_owned()))
            .collect();
        assert_eq!(pages, ours, "SEARCH_FOLD_TABLE differs from FOLD_TABLE");

        // SEARCH_SPACE, decoded from its escapes, is exactly `is_space`.
        let literal = block("const SEARCH_SPACE = \"", "\";");
        let mut space = Vec::new();
        let mut rest = literal.as_str();
        while !rest.is_empty() {
            let (c, len) = match rest.as_bytes() {
                [b'\\', b'u', ..] => (
                    char::from_u32(u32::from_str_radix(&rest[2..6], 16).expect("hex"))
                        .expect("a char"),
                    6,
                ),
                [b'\\', b't', ..] => ('\t', 2),
                [b'\\', b'n', ..] => ('\n', 2),
                [b'\\', b'v', ..] => ('\u{b}', 2),
                [b'\\', b'f', ..] => ('\u{c}', 2),
                [b'\\', b'r', ..] => ('\r', 2),
                [b' ', ..] => (' ', 1),
                _ => panic!("SEARCH_SPACE holds something unexpected: {rest:?}"),
            };
            space.push(c);
            rest = &rest[len..];
        }
        let expected: Vec<char> = (0..=0x10FFFF_u32)
            .filter_map(char::from_u32)
            .filter(|c| is_space(*c))
            .collect();
        space.sort_unstable();
        assert_eq!(space, expected, "SEARCH_SPACE differs from is_space");
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            assert_eq!(is_space(c), c.is_whitespace(), "{c:?}");
        }

        // The diacritic blocks, as the page writes them.
        let ranges = block("const SEARCH_DIACRITICS = /[", "]/g;");
        assert_eq!(
            ranges,
            "\\u0300-\\u036f\\u1ab0-\\u1aff\\u1dc0-\\u1dff\\u20d0-\\u20ff\\ufe20-\\ufe2f"
        );
        for c in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
            let listed = [
                (0x300, 0x36f),
                (0x1ab0, 0x1aff),
                (0x1dc0, 0x1dff),
                (0x20d0, 0x20ff),
                (0xfe20, 0xfe2f),
            ]
            .iter()
            .any(|(a, b)| (*a..=*b).contains(&u32::from(c)));
            assert_eq!(is_diacritic(c), listed, "{c:?}");
        }
    }

    /// The page's half of [`the_page_agrees_on_seeded_random_queries`]: the page's search language
    /// module (`web/src/core/search/`, `#624 voice-page-modularity`), built by the page build's own
    /// `moduleForTests` with the pinned esbuild and imported, over queries and texts drawn from an
    /// adversarial alphabet with a fixed seed.
    const DIFFERENTIAL: &str = r#"
import { join } from "node:path";
import { pathToFileURL } from "node:url";
const { moduleForTests } = await import(pathToFileURL(join(process.argv[1], "web", "build.mjs")).href);
const api = await moduleForTests("src/core/search/index.js");
let seed = 20261027;
const pick = (list) => { seed = (seed * 1103515245 + 12345) % 2147483648; return list[seed % list.length]; };
const pieces = ["a", "b", "ab", "abc", "Café", "cafe", "ΟΣ", "σ", "ß", "ss", "ﬁ", "-", "--", "(", ")", "\"", " ", "\t",
  "\u00a0", "\u3000", "OR", "or", "AND", "NOT", "from:", "from:x", "From:Ada", "in:thread", "in:main", "in:x", "has:link",
  "has:", "before:2026-10-24", "after:10/24/2026", "after:2026-02-30", ":)", "f(x)", "x)", "e\u0301", "\u00e9", "\u0130",
  "\u0131", "\u30ac", "\ud55c", "\ud83d\udc4d\ud83c\udffd", "|", "{", "*", "_", "0", "123", "\ufeff", "\u00b5",
  "\u03bc", "\u1c80", "\u0915\u093f", "\u0e34", "from:U1", "from:\"U1\"", "has:link", "example.test", "\u200b"];
const lines = [];
for (let i = 0; i < 4000; i += 1) {
  let query = "";
  let text = "";
  for (let j = 0; j < 1 + (i % 7); j += 1) query += pick(pieces) + (j % 3 === 0 ? " " : "");
  for (let j = 0; j < 1 + (i % 11); j += 1) text += pick(pieces) + pick(["", " ", "\n"]);
  const author = pick(["Ada", "Ada Lovelace", "x", "Build Bot", "\u00c9mile"]);
  const authorId = pick(["x", "U1", "u1"]);
  const links = pick([[], [], ["https://example.test/a"]]);
  const inThread = pick([false, true]);
  const node = api.searchQueryParse(query);
  const subject = api.searchSubject({ author, authorId, text, links, createdMs: 1792900000000, inThread });
  const matched = api.searchQueryMatches(node, subject, (day) => Date.UTC(day.year, day.month - 1, day.day));
  lines.push(JSON.stringify({ query, text, author, author_id: authorId, links, in_thread: inThread, fold: api.searchFold(text),
    tree: JSON.parse(JSON.stringify(api.searchQueryTree(node))), plan: JSON.parse(JSON.stringify(api.searchUpstreamPlan(node))), matched }));
}
process.stdout.write(lines.join("\n"));
"#;

    #[test]
    fn the_page_agrees_on_seeded_random_queries() {
        // The corpus pins the edges someone thought of; this is the cross-language differential
        // the design asks for, over the edges nobody did. It does not skip without node: the page
        // suite needs node too, and a skipped differential would read as agreement.
        let page = env!("CARGO_MANIFEST_DIR");
        let output = std::process::Command::new("node")
            .args(["--input-type=module", "-e", DIFFERENTIAL, page])
            .output()
            .expect("node runs (the page suite needs it too)");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).expect("UTF-8");
        let mut checked = 0;
        for line in text.lines() {
            let case: Value = serde_json::from_str(line).expect("a JSON line");
            let query = parse(case["query"].as_str().expect("query"));
            let tree = query.root().map_or(Value::Null, Node::to_json);
            assert_eq!(tree, case["tree"], "parse: {case}");
            let plan = upstream_plan(&query);
            let queries: Value = plan
                .queries
                .iter()
                .map(|q| Value::from(q.clone()))
                .collect();
            assert_eq!(queries, case["plan"]["queries"], "plan: {case}");
            let reason = plan
                .scan_reason
                .map_or(Value::Null, |r| Value::from(r.as_str()));
            assert_eq!(reason, case["plan"]["scan"], "plan: {case}");
            let text = case["text"].as_str().expect("text");
            assert_eq!(
                fold(text),
                case["fold"].as_str().expect("fold"),
                "fold: {case}"
            );
            let author = case["author"].as_str().expect("author");
            let links: Vec<String> = case["links"]
                .as_array()
                .expect("links")
                .iter()
                .filter_map(|l| l.as_str().map(str::to_owned))
                .collect();
            let subject = Subject::new(
                &haystack(author, text, &links),
                author,
                case["author_id"].as_str().expect("author_id"),
                1_792_900_000_000,
                case["in_thread"].as_bool().expect("in_thread"),
                !links.is_empty(),
            );
            let matched = query.matches(&subject, &|day| day.start_ms_at_offset(0));
            assert_eq!(
                matched,
                case["matched"].as_bool().expect("matched"),
                "match: {case}"
            );
            checked += 1;
        }
        assert_eq!(checked, 4000);
    }

    #[test]
    fn a_day_is_read_in_three_spellings_and_only_when_real() {
        let day = Day {
            year: 2026,
            month: 2,
            day: 28,
        };
        assert_eq!(Day::parse("2026-02-28"), Some(day));
        assert_eq!(Day::parse("2026/2/28"), Some(day));
        assert_eq!(Day::parse("02/28/2026"), Some(day));
        assert_eq!(Day::parse("2026-02-29"), None);
        assert_eq!(Day::parse("2028-02-29").map(|d| d.day), Some(29));
        assert_eq!(Day::parse("28-02-2026"), None);
        assert_eq!(Day::parse("2026-13-01"), None);
        assert_eq!(Day::parse("yesterday"), None);
        assert_eq!(
            Day {
                year: 1970,
                month: 1,
                day: 1
            }
            .days_since_epoch(),
            0
        );
        assert_eq!(day.start_ms_at_offset(0), 1_772_236_800_000);
        assert_eq!(
            day.start_ms_at_offset(-420),
            1_772_236_800_000 + 420 * 60_000
        );
    }
}
