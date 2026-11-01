//! Reference ids in message text drawn as links: `[render] reference_links`. `#730 reference-links`.
//!
//! A message often names a piece of work by its id alone: a code review, a task, a paste, written as
//! `D123456` with no address around it, because everyone in the channel knows where those live. The
//! page drew such an id as plain text, where the reader wanted a link to it. Which ids mean what, and
//! where each one lives, is the deployment's knowledge, not this server's, so it is configuration:
//!
//! ```toml
//! [render]
//! reference_links = [
//!   { pattern = '\bD([0-9]{6,})\b', url = 'https://review.example.invalid/D$1' },
//!   { pattern = '\bT([0-9]{6,})\b', url = 'https://tasks.example.invalid/T$1' },
//! ]
//! ```
//!
//! Each `pattern` is a regular expression of the [`regex`] crate, whose matching time is linear in
//! the text whatever the pattern, and each `url` a template that names the pattern's groups as
//! [`regex::Captures::expand`] reads one: `$1`, `$name`, `${name}`, and `$$` for a dollar. What a
//! group captured is percent-encoded into the address, so a broad pattern cannot add a query, a
//! fragment or a path of the message's choosing. The rules are tried together: at each place in the
//! text the earliest match wins, the first rule listed breaking a tie.
//!
//! WHERE AN ID IS LINKED. In the words of a message, after comrak has said what is code and what is
//! already a link ([`super::adjust`]): never in a code span or a code block, which show text exactly
//! as typed; never in a link's own text, which would be a link inside a link, nor in what is left of a
//! link the page refused; and never in a run of text that is an address ([`address_runs`]): a URL
//! comrak did not make a link of, a host or a file name, a path with a dot in it, an e-mail address.
//! A slash alone does not make an address, so `D1234567/D1234568` is two ids. The run is the text
//! between blanks within one text node: emphasis or code beside an address ends its run there. The
//! link made is a link like any other: its address passes the same check every link passes and the
//! sanitizer's after that.
//!
//! CHECKED AT LOAD. [`ReferenceLinks::compile`] refuses a rule whose pattern does not compile or
//! matches the empty string; whose template is not an `http` or `https` address with a host of its
//! own, so that no message chooses where a link goes; or whose template names a group its pattern has
//! not got, or names one badly (`${}`, an unclosed `${`). The configuration then fails to load, with
//! the rule's place in the list and why, rather than drawing wrong links later. A pattern that matches
//! nothing but in context (`\b`) links nothing, and is passed over without hiding the other rules.

use std::fmt::Write as _;
use std::ops::Range;
use std::sync::{Arc, LazyLock, RwLock};

use regex::{Captures, Regex, RegexBuilder};

/// One configured rule, as the configuration file writes it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceLinkRule {
    /// The regular expression an id matches.
    pub pattern: String,
    /// The address the id links to, naming the pattern's groups as `$1` or `${name}`.
    pub url: String,
}

/// The most memory one compiled pattern may take. An operator's pattern is short; this only refuses
/// one that would compile into something enormous (a long counted repetition of a large class).
const PATTERN_SIZE_LIMIT: usize = 1 << 20;

/// The rules a process links ids by, compiled.
#[derive(Clone, Debug, Default)]
pub struct ReferenceLinks {
    rules: Vec<Compiled>,
}

#[derive(Clone, Debug)]
struct Compiled {
    pattern: Regex,
    /// The template as written, for the fingerprint.
    url: String,
    /// The template read once: literal text and the groups it names, by number.
    template: Vec<Segment>,
}

/// A part of an address template.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// Written as it is.
    Literal(String),
    /// What the group of this number captured, percent-encoded; nothing when it captured nothing.
    Group(usize),
}

/// A run of a text node's text: words as written, or an id and the address it links to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// Text as written.
    Text(String),
    /// An id, and the address it links to.
    Link {
        /// The id as written.
        text: String,
        /// The address made from the rule's template.
        url: String,
    },
}

impl ReferenceLinks {
    /// The rules `rules` names, compiled, or why one cannot be used: its place in the list (from 1)
    /// and the reason.
    ///
    /// # Errors
    ///
    /// A sentence naming the first rule that cannot be used and why: a pattern that does not compile
    /// or matches the empty string, a template that is not an `http` or `https` address with a host
    /// of its own, or a template naming a group the pattern has not got, or naming one badly.
    pub fn compile(rules: &[ReferenceLinkRule]) -> Result<Self, String> {
        let mut compiled = Vec::with_capacity(rules.len());
        for (index, rule) in rules.iter().enumerate() {
            let place = index + 1;
            let pattern = RegexBuilder::new(&rule.pattern)
                .size_limit(PATTERN_SIZE_LIMIT)
                .build()
                .map_err(|error| {
                    format!("reference link {place}: its pattern does not compile: {error}")
                })?;
            if pattern.is_match("") {
                return Err(format!(
                    "reference link {place}: its pattern matches the empty string, so it would link nothing"
                ));
            }
            let template = parse_template(&pattern, &rule.url)
                .map_err(|why| format!("reference link {place}: its url {why}"))?;
            check_address(&template)
                .map_err(|why| format!("reference link {place}: its url {why}"))?;
            compiled.push(Compiled {
                pattern,
                url: rule.url.clone(),
                template,
            });
        }
        Ok(Self { rules: compiled })
    }

    /// Whether there is any rule at all: with none, no text is looked at.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// What names these rules in the search index's version, so an index built under other rules is
    /// built again: `None` with no rules, or a stable hash of every pattern and template, in order.
    #[must_use]
    pub fn fingerprint(&self) -> Option<String> {
        if self.rules.is_empty() {
            return None;
        }
        // FNV-1a, 64 bits: stable across processes and builds, unlike the standard library's hasher.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for rule in &self.rules {
            for byte in rule
                .pattern
                .as_str()
                .bytes()
                .chain([0])
                .chain(rule.url.bytes())
                .chain([0])
            {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        let mut name = String::with_capacity(16);
        let _ = write!(name, "{hash:016x}");
        Some(name)
    }

    /// `text` cut at every id the rules find in it, or `None` when they find none.
    ///
    /// At each place the earliest match wins, the first rule listed breaking a tie, and the text after
    /// it is searched from its end, so matches never overlap. A match that is empty, or inside an
    /// address ([`address_runs`]), is not an id: that rule alone searches on, from past it, so another
    /// rule's match at the same place still counts. Every rule's search only ever moves forward, so a
    /// body is read about once per rule however many ids, or near-ids inside addresses, it holds.
    #[must_use]
    pub fn pieces(&self, text: &str) -> Option<Vec<Piece>> {
        if self.rules.is_empty() {
            return None;
        }
        let addresses = address_runs(text);
        let address_at = |at: usize| {
            addresses
                .iter()
                .find(|run| run.contains(&at))
                .map(|run| run.end)
        };
        let mut next: Vec<Option<Captures<'_>>> = self
            .rules
            .iter()
            .map(|rule| rule.pattern.captures_at(text, 0))
            .collect();
        let mut pieces = Vec::new();
        let mut written = 0;
        let mut from = 0;
        loop {
            // Each rule's next match, once it is one: at or after `from`, not empty, not in an address.
            for (rule, slot) in self.rules.iter().zip(next.iter_mut()) {
                while let Some(whole) = slot.as_ref().and_then(|found| found.get(0)) {
                    let again = if whole.start() < from {
                        from
                    } else if whole.is_empty() {
                        whole.start()
                            + text[whole.start()..]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8)
                    } else if let Some(end) = address_at(whole.start()) {
                        end
                    } else {
                        break;
                    };
                    *slot = if again <= text.len() {
                        rule.pattern.captures_at(text, again)
                    } else {
                        None
                    };
                }
            }
            let Some(index) = next
                .iter()
                .enumerate()
                .filter_map(|(index, found)| {
                    found
                        .as_ref()
                        .and_then(|found| found.get(0))
                        .map(|whole| (whole.start(), index))
                })
                .min()
                .map(|(_, index)| index)
            else {
                break;
            };
            let captures = next[index].as_ref().expect("the slot chosen holds a match");
            let whole = captures.get(0).expect("a match has a whole");
            if whole.start() > written {
                pieces.push(Piece::Text(text[written..whole.start()].to_owned()));
            }
            pieces.push(Piece::Link {
                text: whole.as_str().to_owned(),
                url: expand(&self.rules[index].template, captures),
            });
            written = whole.end();
            from = whole.end();
        }
        if pieces.is_empty() {
            return None;
        }
        if written < text.len() {
            pieces.push(Piece::Text(text[written..].to_owned()));
        }
        Some(pieces)
    }
}

/// `template` read as [`regex::Captures::expand`] reads one: `$$` is a dollar; `${name}` and `$name`
/// (the longest run of letters, digits and `_`) name a group, a name of digits alone its number; a
/// `$` naming nothing is a dollar. Unlike `expand`, which writes nothing for a group it has not got,
/// a group `pattern` has not got is refused, and so are `${}` and an unclosed `${`.
fn parse_template(pattern: &Regex, template: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut rest = template;
    while let Some(at) = rest.find('$') {
        literal.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        if let Some(tail) = after.strip_prefix('$') {
            literal.push('$');
            rest = tail;
            continue;
        }
        let (name, tail) = if let Some(braced) = after.strip_prefix('{') {
            let Some(end) = braced.find('}') else {
                return Err(format!(
                    "opens a group name with ${{ and never closes it: {template:?}"
                ));
            };
            if end == 0 {
                return Err(format!("names an empty group, ${{}}: {template:?}"));
            }
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], &after[end..])
        };
        rest = tail;
        if name.is_empty() {
            literal.push('$');
            continue;
        }
        let group = match name.parse::<usize>() {
            Ok(number) if number < pattern.captures_len() => Some(number),
            Ok(_) => None,
            Err(_) => pattern
                .capture_names()
                .position(|known| known == Some(name)),
        };
        let Some(group) = group else {
            return Err(format!(
                "names the group {name:?}, which its pattern has not got"
            ));
        };
        if !literal.is_empty() {
            segments.push(Segment::Literal(std::mem::take(&mut literal)));
        }
        segments.push(Segment::Group(group));
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

/// Whether a template is an `http` or `https` address whose host is written in it: its scheme and,
/// when it names a group, its whole authority, up to the first `/`, `?` or `#` after `://`, are
/// literal text before the first group, and the authority is not empty. So no message decides where
/// a link goes, only which page there.
fn check_address(template: &[Segment]) -> Result<(), String> {
    let mut prefix = String::new();
    let mut grouped = false;
    for segment in template {
        match segment {
            Segment::Literal(text) => prefix.push_str(text),
            Segment::Group(_) => {
                grouped = true;
                break;
            }
        }
    }
    let Some((scheme, after)) = prefix.split_once("://") else {
        return Err(format!("must be an http or https address, not {prefix:?}"));
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err(format!("must be an http or https address, not {prefix:?}"));
    }
    if after.is_empty() || after.starts_with(['/', '?', '#']) {
        return Err("has no host of its own; write the host in the url".to_owned());
    }
    if grouped && !after.contains(['/', '?', '#']) {
        return Err(
            "leaves its host to what the pattern captured; write the whole host in the url"
                .to_owned(),
        );
    }
    Ok(())
}

/// The address `template` makes of one match: its literal text, and each group it names
/// percent-encoded, every byte but an unreserved one (`A-Z a-z 0-9 - . _ ~`) written as `%XX`.
fn expand(template: &[Segment], captures: &Captures<'_>) -> String {
    let mut url = String::new();
    for segment in template {
        match segment {
            Segment::Literal(text) => url.push_str(text),
            Segment::Group(group) => {
                for byte in captures
                    .get(*group)
                    .map_or("", |found| found.as_str())
                    .bytes()
                {
                    if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                        url.push(char::from(byte));
                    } else {
                        let _ = write!(url, "%{byte:02X}");
                    }
                }
            }
        }
    }
    url
}

/// The runs of `text` that are addresses, by byte: each run between blanks that, past the
/// punctuation around it (`(`, `"`, a closing `.` or `)`...), holds `://`; starts with `www.`; holds
/// `@`; holds a `/` and a `.`; or holds a `.` between two letters or digits, as a host or a file
/// name does. A slash alone is not an address (`D1234567/D1234568`), nor a full stop that ends a
/// sentence.
fn address_runs(text: &str) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut start = None;
    for (at, character) in text.char_indices().chain([(text.len(), ' ')]) {
        if character.is_whitespace() {
            if let Some(from) = start.take() {
                if is_address(&text[from..at]) {
                    runs.push(from..at);
                }
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    runs
}

/// Whether one run between blanks is an address. See [`address_runs`].
fn is_address(run: &str) -> bool {
    const AROUND: &[char] = &[
        '(', ')', '[', ']', '{', '}', '<', '>', '"', '\'', '.', ',', ';', ':', '!', '?', '*', '_',
        '`',
    ];
    let run = run.trim_matches(AROUND);
    if run.contains("://")
        || run.contains('@')
        || run
            .get(..4)
            .is_some_and(|start| start.eq_ignore_ascii_case("www."))
    {
        return true;
    }
    if run.contains('/') && run.contains('.') {
        return true;
    }
    let bytes = run.as_bytes();
    bytes.windows(3).any(|three| {
        three[1] == b'.' && three[0].is_ascii_alphanumeric() && three[2].is_ascii_alphanumeric()
    })
}

/// The rules this process links ids by: none until [`set_reference_links`].
static CONFIGURED: LazyLock<RwLock<Arc<ReferenceLinks>>> = LazyLock::new(RwLock::default);

/// Link ids in every body rendered after this by `links`, as `[render] reference_links` configures
/// it. Called once at startup.
pub fn set_reference_links(links: ReferenceLinks) {
    *CONFIGURED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(links);
}

#[cfg(test)]
thread_local! {
    /// A test's own rules, on its own thread, so tests with different rules run side by side.
    static TEST_LINKS: std::cell::RefCell<Option<Arc<ReferenceLinks>>> =
        const { std::cell::RefCell::new(None) };
}

/// This thread links ids by a test's own rules until the guard is dropped.
#[cfg(test)]
pub(crate) struct ReferenceLinksGuard(Option<Arc<ReferenceLinks>>);

#[cfg(test)]
impl ReferenceLinksGuard {
    pub(crate) fn set(links: ReferenceLinks) -> Self {
        Self(TEST_LINKS.with(|cell| cell.replace(Some(Arc::new(links)))))
    }
}

#[cfg(test)]
impl Drop for ReferenceLinksGuard {
    fn drop(&mut self) {
        TEST_LINKS.with(|cell| *cell.borrow_mut() = self.0.take());
    }
}

/// The configured rules. See [`set_reference_links`].
#[must_use]
pub fn reference_links() -> Arc<ReferenceLinks> {
    #[cfg(test)]
    if let Some(links) = TEST_LINKS.with(|cell| cell.borrow().clone()) {
        return links;
    }
    CONFIGURED
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[cfg(test)]
mod tests {
    use super::{Piece, ReferenceLinkRule, ReferenceLinks, ReferenceLinksGuard};
    use crate::render::{to_html_as, Markup, SingleNewline};

    fn rule(pattern: &str, url: &str) -> ReferenceLinkRule {
        ReferenceLinkRule {
            pattern: pattern.to_owned(),
            url: url.to_owned(),
        }
    }

    /// The example rules the module note shows: reviews and tasks, at example domains.
    fn examples() -> ReferenceLinks {
        ReferenceLinks::compile(&[
            rule(r"\bD([0-9]{6,})\b", "https://review.example.invalid/D$1"),
            rule(
                r"\bT(?<task>[0-9]{6,})\b",
                "https://tasks.example.invalid/T${task}",
            ),
        ])
        .expect("the examples compile")
    }

    /// A body rendered under the example rules, uncached, under the soft newline rule.
    fn drawn(text: &str) -> String {
        let _links = ReferenceLinksGuard::set(examples());
        to_html_as(text, Markup::Discord, SingleNewline::Soft)
    }

    const REVIEW: &str = r#"<a href="https://review.example.invalid/D1234567" target="_blank" rel="noopener noreferrer nofollow">D1234567</a>"#;

    #[test]
    fn an_id_in_the_words_is_a_link_to_where_its_rule_says() {
        assert_eq!(
            drawn("landed D1234567 today"),
            format!("<p>landed {REVIEW} today</p>\n")
        );
        assert!(
            drawn("see T7654321").contains(r#"<a href="https://tasks.example.invalid/T7654321""#),
            "a named group fills the template"
        );
        // Too short for the rule, or part of a longer word: not an id.
        for text in ["D12345 is short", "XD1234567 and D1234567X"] {
            assert!(!drawn(text).contains("<a "), "{text:?} drew a link");
        }
    }

    #[test]
    fn punctuation_around_an_id_stays_text_beside_its_link() {
        assert_eq!(
            drawn("(D1234567), D1234567."),
            format!("<p>({REVIEW}), {REVIEW}.</p>\n")
        );
        assert_eq!(
            drawn("**D1234567**"),
            format!("<p><strong>{REVIEW}</strong></p>\n"),
            "emphasis around it stays"
        );
    }

    #[test]
    fn code_existing_links_and_addresses_keep_an_id_as_written() {
        assert_eq!(drawn("`D1234567`"), "<p><code>D1234567</code></p>\n");
        assert!(
            !drawn("```\nD1234567\n```").contains("<a "),
            "a code block drew a link"
        );
        // A link's own text is never linked again, whether written as Markdown or autolinked.
        let labelled = drawn("[D1234567](https://elsewhere.example.invalid/x)");
        assert_eq!(labelled.matches("<a ").count(), 1, "{labelled}");
        assert!(labelled.contains("https://elsewhere.example.invalid/x"));
        let bare = drawn("https://elsewhere.example.invalid/D1234567");
        assert_eq!(bare.matches("<a ").count(), 1, "{bare}");
        assert!(!bare.contains("review.example.invalid"));
        // An address comrak did not make a link of, a path, a file or host name, an e-mail address.
        for text in [
            "elsewhere.example.invalid/D1234567",
            "src/D1234567.rs",
            "D1234567@example.invalid",
            "D1234567.example.invalid",
            "file.D1234567.txt",
        ] {
            assert!(
                !drawn(text).contains("review.example.invalid"),
                "{text:?} linked an id inside an address"
            );
        }
        // A slash alone is not an address, nor a full stop that ends a sentence.
        assert_eq!(
            drawn("D1234567/D1234568")
                .matches("review.example.invalid")
                .count(),
            2
        );
        assert!(drawn("landed D1234567.").contains(REVIEW));
    }

    #[test]
    fn a_slack_link_labelled_with_an_id_stays_that_link() {
        // Slack writes a link as `<address|label>`, which its reader makes a Markdown link before
        // anything is drawn: the id in its label is that link's text, not a second link.
        let _links = ReferenceLinksGuard::set(examples());
        let slack = to_html_as(
            "see <https://elsewhere.example.invalid/r|D1234567>",
            Markup::Slack,
            SingleNewline::Soft,
        );
        assert_eq!(slack.matches("<a ").count(), 1, "{slack}");
        assert!(
            slack.contains(r#"href="https://elsewhere.example.invalid/r""#)
                && slack.contains(">D1234567</a>"),
            "{slack}"
        );
    }

    #[test]
    fn what_is_left_of_a_refused_link_is_never_linked() {
        // The label and the address after it are the refused link's, shown as text.
        for text in [
            "[D1234567](ftp://elsewhere.example.invalid/y)",
            "[x](javascript:D1234567)",
        ] {
            let html = drawn(text);
            assert!(!html.contains("review.example.invalid"), "{text:?}: {html}");
        }
    }

    #[test]
    fn a_body_naming_many_ids_keeps_its_formatting() {
        let body = format!("**landed**\n\n{}", "D1234567 ".repeat(400));
        let html = drawn(&body);
        assert!(
            html.contains("<strong>landed</strong>"),
            "drawn as plain paragraphs"
        );
        assert_eq!(html.matches("review.example.invalid").count(), 400);
    }

    #[test]
    fn a_rule_matching_only_in_context_hides_no_other_rule() {
        let links = ReferenceLinks::compile(&[
            rule(r"\b(?:Q[0-9]+)?", "https://q.example.invalid/$0"),
            rule(r"\bD[0-9]+\b", "https://review.example.invalid/$0"),
        ])
        .expect("compiles");
        let pieces = links
            .pieces("see D1234567 now")
            .expect("the second rule's id");
        assert!(pieces.contains(&Piece::Link {
            text: "D1234567".to_owned(),
            url: "https://review.example.invalid/D1234567".to_owned()
        }));
    }

    #[test]
    fn an_address_long_or_hostile_costs_one_reading() {
        // Each match the rule finds is inside one address, which is passed over whole: once.
        let links =
            ReferenceLinks::compile(&[rule(r"([0-9]{6,})", "https://x.example.invalid/$1")])
                .expect("compiles");
        let text = format!("{}/x.y", "1".repeat(16_000));
        let started = std::time::Instant::now();
        assert_eq!(links.pieces(&text), None);
        let many = "D1/".repeat(5_461);
        let links =
            ReferenceLinks::compile(&[rule(r"\bD[0-9]+\b", "https://x.example.invalid/$0")])
                .expect("compiles");
        assert!(links.pieces(&format!("{many}x.y")).is_none());
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_captured_value_is_percent_encoded_into_the_address() {
        let links =
            ReferenceLinks::compile(&[rule(r"\bP:(\S+)", "https://p.example.invalid/paste/$1")])
                .expect("compiles");
        assert_eq!(
            links.pieces("P:a?b#c%d"),
            Some(vec![Piece::Link {
                text: "P:a?b#c%d".to_owned(),
                url: "https://p.example.invalid/paste/a%3Fb%23c%25d".to_owned()
            }])
        );
    }

    #[test]
    fn with_no_rules_nothing_is_looked_at() {
        let _links = ReferenceLinksGuard::set(ReferenceLinks::default());
        assert_eq!(
            to_html_as("landed D1234567", Markup::Discord, SingleNewline::Soft),
            "<p>landed D1234567</p>\n"
        );
        assert_eq!(ReferenceLinks::default().fingerprint(), None);
    }

    #[test]
    fn the_earliest_match_wins_and_the_first_rule_breaks_a_tie() {
        let links = ReferenceLinks::compile(&[
            rule(r"\bD\d+\b", "https://a.example.invalid/$0"),
            rule(r"\b[A-Z]\d+\b", "https://b.example.invalid/$0"),
        ])
        .expect("compiles");
        let link = |text: &str, url: &str| Piece::Link {
            text: text.to_owned(),
            url: url.to_owned(),
        };
        assert_eq!(
            links.pieces("T1 D2 x"),
            Some(vec![
                link("T1", "https://b.example.invalid/T1"),
                Piece::Text(" ".to_owned()),
                link("D2", "https://a.example.invalid/D2"),
                Piece::Text(" x".to_owned()),
            ])
        );
        assert_eq!(links.pieces("no ids"), None);
    }

    #[test]
    fn a_rule_that_cannot_be_used_is_refused_with_its_place_and_why() {
        let refused =
            |rules: &[ReferenceLinkRule]| ReferenceLinks::compile(rules).expect_err("refused");
        let good = rule(r"\bD(\d+)\b", "https://review.example.invalid/D$1");
        assert!(refused(&[
            good.clone(),
            rule(r"\bD(\d+", "https://x.example.invalid/$1")
        ])
        .starts_with("reference link 2: its pattern does not compile"));
        assert!(refused(&[rule(r"\d*", "https://x.example.invalid/")])
            .contains("matches the empty string"));
        for url in [
            "javascript:alert(1)",
            "ftp://x.example.invalid/$1",
            "review.example.invalid/D$1",
        ] {
            assert!(
                refused(&[rule(r"\bD(\d+)\b", url)]).contains("must be an http or https address"),
                "{url}"
            );
        }
        assert!(
            refused(&[rule(r"\bD(\d+)\b", "https://x.example.invalid/$2")])
                .contains(r#"names the group "2""#)
        );
        assert!(
            refused(&[rule(r"\bD(\d+)\b", "https://x.example.invalid/${id}")])
                .contains(r#"names the group "id""#)
        );
        assert!(
            refused(&[rule(r"\bD(\d+)\b", "https://x.example.invalid/$1a")])
                .contains(r#"names the group "1a""#)
        );
        assert!(
            refused(&[rule(r"\bD(\d+)\b", "https://x.example.invalid/${}")])
                .contains("names an empty group")
        );
        assert!(
            refused(&[rule(r"\bD(\d+)\b", "https://x.example.invalid/${1")])
                .contains("never closes it")
        );
        // The host is the template's, never the message's.
        for url in [
            "https://$1/x",
            "https:///$1",
            "https://x.example.invalid$1",
            "https://",
        ] {
            assert!(
                refused(&[rule(r"\bD(\d+)\b", url)]).contains("host"),
                "{url}"
            );
        }
        // `$$` is a dollar, and `$0` the whole match: both fine.
        assert!(
            ReferenceLinks::compile(&[rule(r"\bD\d+\b", "https://x.example.invalid/$$/$0")])
                .is_ok()
        );
        // The fingerprint names the rules, and changes with them.
        let one = ReferenceLinks::compile(std::slice::from_ref(&good))
            .unwrap()
            .fingerprint();
        let two =
            ReferenceLinks::compile(&[good, rule(r"\bT\d+\b", "https://t.example.invalid/$0")])
                .unwrap()
                .fingerprint();
        assert!(one.is_some() && two.is_some() && one != two);
        assert_eq!(
            one,
            ReferenceLinks::compile(&[rule(r"\bD(\d+)\b", "https://review.example.invalid/D$1")])
                .unwrap()
                .fingerprint()
        );
    }
}
