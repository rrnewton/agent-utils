//! Hashtag labels derived from message text. `#1825 vibe-talk-hashtag-labels`.
//!
//! The owner seeds labels by typing hashtags (`#kvm #benchmarking` at the end of
//! a message, thread 166). This module is the ONE parser: labels are derived
//! from [`crate::model::Message::content`] on the server every time a message
//! is served — decided, never stored, exactly like [`crate::model::Message::noise`]
//! — so editing a message updates its labels live and no migration exists to
//! drift. The page never re-parses text; it reads the served labels.
//!
//! # The token rule
//!
//! A hashtag is a `#` at the start of the text or directly after whitespace or
//! an opening bracket/quote, followed by an ASCII letter, followed by zero or
//! more of `[a-z0-9_-]` (either case on input). The label is the token
//! ASCII-lowercased, deduped per message in first-seen order. Trailing
//! punctuation is excluded by construction: `#kvm,` labels `kvm`.
//!
//! NOT a label:
//!
//! * inside a fenced code block (``` fences) or an inline code span;
//! * inside a URL — any `#` in a whitespace-delimited `http://`/`https://`
//!   run, so `https://x/#anchor` yields nothing;
//! * a Slack channel reference (`<#C…|name>` markup: a `#` directly
//!   preceded by `<`);
//! * a `:thread:`/`:issue:`-style short reference (colon syntax, never
//!   matched) and `#` followed by a digit (`#123` stays an issue reference).
//!
//! # Namespace
//!
//! A hashtag label's slug is the bare folded token (`kvm`). Auto-generated
//! topics, when they are built, live under the `auto:` prefix (the `ws:`
//! kind-prefix precedent in the voice-overview plan §8.5), so they can never
//! collide with a typed hashtag; nothing here mints `auto:` labels. See
//! `.minibeads/issues/agent-utils-1825.md`.

/// Derive the hashtag labels of one message body, in first-seen order.
///
/// Pure and total: the same text always yields the same labels, which is what
/// lets every surface (cards, search, the Settings browse) agree without a
/// stored copy.
#[must_use]
pub fn extract_hashtags(content: &str) -> Vec<String> {
    let chars: Vec<char> = content.chars().collect();
    let masked = masked_positions(&chars);
    let mut labels: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '#' || masked[i] {
            i += 1;
            continue;
        }
        // Boundary: start of text, or whitespace / opening punctuation before.
        // A `#` glued to a word (`a#b`) or to `<` (channel refs, also masked)
        // starts nothing.
        let boundary = i == 0
            || chars[i - 1].is_whitespace()
            || matches!(chars[i - 1], '(' | '[' | '{' | '"' | '\'' | '“' | '‘');
        if !boundary {
            i += 1;
            continue;
        }
        // First character after `#` must be an ASCII letter: `#123` and
        // `#1abc` are issue references and bare numbers, never labels.
        let Some(first) = chars.get(i + 1) else {
            break;
        };
        if !first.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        let mut label = String::new();
        while j < chars.len() && (chars[j].is_ascii_alphanumeric() || matches!(chars[j], '-' | '_'))
        {
            label.push(chars[j].to_ascii_lowercase());
            j += 1;
        }
        if !labels.contains(&label) {
            labels.push(label);
        }
        i = j;
    }
    labels
}

/// The label groups as derivation sees them (`#1825`, owner thread 167): which
/// display each spelling belongs to, and which spellings a display stands for.
/// Built from the store's groups; empty when there are none, in which case
/// [`GroupIndex::apply`] is the identity.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupIndex {
    /// Spelling (or display) → the group's display.
    display_of: std::collections::HashMap<String, String>,
    /// Display → every spelling in its group, sorted.
    members_of: std::collections::HashMap<String, Vec<String>>,
}

impl GroupIndex {
    /// Build from `(display, spellings)` pairs, as the store serves them. The
    /// display is always a member of its own group, listed or not.
    #[must_use]
    pub fn new(groups: Vec<(String, Vec<String>)>) -> Self {
        let mut display_of = std::collections::HashMap::new();
        let mut members_of = std::collections::HashMap::new();
        for (display, mut spellings) in groups {
            if !spellings.contains(&display) {
                spellings.push(display.clone());
            }
            spellings.sort();
            spellings.dedup();
            for spelling in &spellings {
                display_of.insert(spelling.clone(), display.clone());
            }
            members_of.insert(display, spellings);
        }
        Self {
            display_of,
            members_of,
        }
    }

    /// Whether no group exists.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members_of.is_empty()
    }

    /// Map extracted spellings to what cards show: each spelling's group
    /// display, or the spelling itself, deduped in first-seen order.
    #[must_use]
    pub fn apply(&self, extracted: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for spelling in extracted {
            let shown: &str = self
                .display_of
                .get(spelling)
                .map_or(spelling.as_str(), String::as_str);
            if !out.iter().any(|held| held == shown) {
                out.push(shown.to_owned());
            }
        }
        out
    }

    /// Every spelling a label-filter slug may mean: the whole group when the
    /// slug names any member or the display, else just the slug. Search
    /// expands a `label:` node to these so any spelling finds the group.
    #[must_use]
    pub fn expand(&self, slug: &str) -> Vec<String> {
        match self.display_of.get(slug) {
            Some(display) => self
                .members_of
                .get(display)
                .cloned()
                .unwrap_or_else(|| vec![slug.to_owned()]),
            None => vec![slug.to_owned()],
        }
    }
}

/// Positions that cannot hold a hashtag: fenced code, inline code, URLs and
/// `<#…>` channel references. Masking (rather than stripping) keeps every
/// other position's neighbours exactly as written.
fn masked_positions(chars: &[char]) -> Vec<bool> {
    let mut masked = vec![false; chars.len()];
    mask_fences(chars, &mut masked);
    mask_inline_code(chars, &mut masked);
    mask_urls(chars, &mut masked);
    mask_channel_refs(chars, &mut masked);
    masked
}

/// ``` fences: a line whose first non-space characters are ``` toggles the
/// block; the fence lines themselves are masked too.
fn mask_fences(chars: &[char], masked: &mut [bool]) {
    let mut in_fence = false;
    let mut i = 0;
    while i < chars.len() {
        // Start of a line: skip blanks, then look for ```.
        let mut j = i;
        while j < chars.len() && (chars[j] == ' ' || chars[j] == '\t') {
            j += 1;
        }
        let is_fence = chars[j..].starts_with(&['`', '`', '`']);
        if is_fence {
            in_fence = !in_fence;
        }
        // Mask to end of line when fenced (or on the fence line itself).
        let mut k = i;
        while k < chars.len() && chars[k] != '\n' {
            if in_fence || is_fence {
                masked[k] = true;
            }
            k += 1;
        }
        i = k + 1;
    }
}

/// Inline code: a run of backticks opens a span closed by a run of the same
/// length; an unclosed run masks to the end of the text.
fn mask_inline_code(chars: &[char], masked: &mut [bool]) {
    let mut i = 0;
    while i < chars.len() {
        if masked[i] || chars[i] != '`' {
            i += 1;
            continue;
        }
        let mut run = 0;
        while i + run < chars.len() && chars[i + run] == '`' {
            run += 1;
        }
        // Find the closing run of the same length.
        let mut j = i + run;
        let mut closed = None;
        while j < chars.len() {
            if chars[j] == '`' && !masked[j] {
                let mut r = 0;
                while j + r < chars.len() && chars[j + r] == '`' {
                    r += 1;
                }
                if r == run {
                    closed = Some(j + r);
                    break;
                }
                j += r;
            } else {
                j += 1;
            }
        }
        let end = closed.unwrap_or(chars.len());
        for slot in &mut masked[i..end] {
            *slot = true;
        }
        i = end;
    }
}

/// URLs: `http://` or `https://` up to the next whitespace is all masked, so
/// a fragment (`#anchor`) inside one can never read as a hashtag. Scanned over
/// chars (never byte indices) so non-ASCII text before a URL cannot shift it.
fn mask_urls(chars: &[char], masked: &mut [bool]) {
    let https: Vec<char> = "https://".chars().collect();
    let http: Vec<char> = "http://".chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let scheme_len = if chars[i..].starts_with(&https) {
            https.len()
        } else if chars[i..].starts_with(&http) {
            http.len()
        } else {
            i += 1;
            continue;
        };
        let mut end = i + scheme_len;
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }
        for slot in &mut masked[i..end] {
            *slot = true;
        }
        i = end;
    }
}

/// Slack channel references: `<#C…|name>` — mask from `<#` through `>`.
fn mask_channel_refs(chars: &[char], masked: &mut [bool]) {
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '<' && chars[i + 1] == '#' {
            let mut j = i;
            while j < chars.len() && chars[j] != '>' && chars[j] != '\n' {
                masked[j] = true;
                j += 1;
            }
            if j < chars.len() && chars[j] == '>' {
                masked[j] = true;
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{extract_hashtags, GroupIndex};

    fn labels(text: &str) -> Vec<String> {
        extract_hashtags(text)
    }

    #[test]
    fn plain_hashtags_at_the_end_of_a_message() {
        assert_eq!(
            labels("fixed the thing\n#kvm #benchmarking"),
            vec!["kvm", "benchmarking"]
        );
        assert_eq!(labels("#kvm"), vec!["kvm"]);
        assert_eq!(labels("(#kvm) [#perf]"), vec!["kvm", "perf"]);
    }

    #[test]
    fn case_folds_and_character_set() {
        assert_eq!(
            labels("#Benchmarking #KVM-2 #a_b #A"),
            vec!["benchmarking", "kvm-2", "a_b", "a"]
        );
        assert_eq!(labels("#kvm, done. #perf!"), vec!["kvm", "perf"]);
    }

    #[test]
    fn numbers_and_glued_hashes_are_not_labels() {
        assert!(
            labels("#123 is fixed").is_empty(),
            "#123 stays an issue reference"
        );
        assert!(labels("see #1abc").is_empty());
        assert!(
            labels("a#b and x#kvm").is_empty(),
            "a # glued to a word starts nothing"
        );
        assert!(labels("## Heading").is_empty());
        assert!(labels("trailing #").is_empty());
    }

    #[test]
    fn code_spans_and_fences_hide_hashtags() {
        assert!(labels("run `#kvm` now").is_empty());
        assert_eq!(labels("run `#kvm` then #real"), vec!["real"]);
        assert!(labels("```\n#kvm in a fence\n```").is_empty());
        assert_eq!(labels("```rust\nlet x = \"#no\";\n```\n#yes"), vec!["yes"]);
        assert!(
            labels("unclosed `#kvm").is_empty(),
            "an unclosed span masks to the end"
        );
    }

    #[test]
    fn urls_channel_refs_and_short_refs_hide_hashtags() {
        assert!(labels("see https://example.test/page#anchor for it").is_empty());
        assert_eq!(
            labels("see https://example.test/#anchor and #kvm"),
            vec!["kvm"]
        );
        assert!(labels("ask in <#C0123ABCD|general> please").is_empty());
        assert!(labels("in :thread:123 and :issue:456").is_empty());
        assert_eq!(labels("in :thread:123 about #kvm"), vec!["kvm"]);
    }

    #[test]
    fn headings_and_channel_shaped_words() {
        // Coordb amendment 1: '# Heading' (with a space) is never a label,
        // while '#word' at a line start counts as one (fine for chat).
        assert!(labels("# Heading\nbody").is_empty());
        assert_eq!(labels("#Heading\nbody"), vec!["heading"]);
        assert_eq!(labels("notes\n#kvm summary"), vec!["kvm"]);
        // Coordb amendment 2: a plain '#name' that happens to spell a channel
        // name is still a label; only the linked '<#C…|name>' form is not.
        assert_eq!(labels("ask in #general please"), vec!["general"]);
    }

    #[test]
    fn groups_map_spellings_to_the_display_and_expand_back() {
        let index = GroupIndex::new(vec![(
            "benchmarking".to_owned(),
            vec!["benchmark".to_owned(), "benchmarking".to_owned()],
        )]);
        // Cards show the display, once, in first-seen order.
        assert_eq!(
            index.apply(&[
                "benchmark".to_owned(),
                "kvm".to_owned(),
                "benchmarking".to_owned()
            ]),
            vec!["benchmarking", "kvm"]
        );
        // Any member (or the display) expands to the whole group for search.
        assert_eq!(index.expand("benchmark"), vec!["benchmark", "benchmarking"]);
        assert_eq!(
            index.expand("benchmarking"),
            vec!["benchmark", "benchmarking"]
        );
        assert_eq!(index.expand("kvm"), vec!["kvm"]);
        // No groups: the identity.
        assert_eq!(
            GroupIndex::default().apply(&["kvm".to_owned()]),
            vec!["kvm"]
        );
    }

    #[test]
    fn duplicates_fold_to_first_seen_order() {
        assert_eq!(labels("#b #a #B #a #c"), vec!["b", "a", "c"]);
        assert!(labels("no hashtags here").is_empty());
        assert!(labels("").is_empty());
    }
}
