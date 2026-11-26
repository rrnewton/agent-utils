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
//!
//! `pinned` is RESERVED (`#1853 vibe-talk-labels-dropdown-pinned-builtin`):
//! it names the built-in Pinned label (the pin set in the store), never a
//! hashtag, so a typed `#pinned` yields no label here and `label:pinned`
//! search can mean the pin set alone.

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
        // `#1853`: `pinned` is the built-in label's reserved slug, never a hashtag.
        if label != "pinned" && !labels.contains(&label) {
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
        // `#1853`: the built-in label stands alone; no group ever expands it.
        if slug == "pinned" {
            return vec![slug.to_owned()];
        }
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

// --- labels added from the message menu ------------------------------------------------------
//
// `#2279 vibe-talk-post-facto-labels` (owner requests 380, 382). ONE label set of NAMES: a
// hashtag label's name begins with `#` (`#kvm`), every other label is a plain name
// (`merge queue`). A menu-added `#kvm` and a typed `#kvm` are the SAME label because the
// hashtag subset keeps #1825's representation — the bare folded slug in
// [`crate::model::Message::labels`] — and the menu-added slugs join the typed ones there
// before the groups apply ([`merge`]). Plain labels ride beside them in
// [`crate::model::Message::plain_labels`]. Only the menu's labels are stored (origin
// `manual`, `vibe-talk/src/store/sqlite.rs`); text-derived labels stay derived.

/// The longest plain label name, in characters, after folding.
pub const MAX_PLAIN_LABEL_CHARS: usize = 40;

/// The longest hashtag slug, as a label group may name one
/// ([`crate::store::validate_label_group`]).
pub const MAX_HASHTAG_SLUG_CHARS: usize = 64;

/// At most this many menu-added labels on one message.
pub const MAX_MANUAL_LABELS_PER_MESSAGE: usize = 32;

/// At most this many menu-added label rows on the whole server.
pub const MAX_MANUAL_LABEL_ROWS: usize = 20_000;

/// One label name, normalized: what the store keeps and every surface compares.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LabelName {
    /// `#kvm`: the slug, folded by the extractor's token rule, without its `#`.
    Hashtag(String),
    /// `merge queue`: folded with the search fold, white space collapsed.
    Plain(String),
}

impl LabelName {
    /// Normalize a name as the owner typed it in the menu. A leading `#` makes it a hashtag
    /// label under the extractor's token rule; anything else is a plain name.
    ///
    /// # Errors
    ///
    /// A sentence saying why the name cannot be a label, fit to show beside the field.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let mut chars = rest.chars();
            let well_formed = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
            if !well_formed {
                return Err(
                    "a # label is one word: a letter, then letters, digits, - or _".to_owned(),
                );
            }
            if rest.chars().count() > MAX_HASHTAG_SLUG_CHARS {
                return Err(format!(
                    "a # label is at most {MAX_HASHTAG_SLUG_CHARS} characters"
                ));
            }
            let slug = rest.to_ascii_lowercase();
            if slug == "pinned" {
                return Err(PINNED_RESERVED.to_owned());
            }
            return Ok(Self::Hashtag(slug));
        }
        let folded = crate::search_query::fold(trimmed)
            .trim_matches(' ')
            .to_owned();
        if folded.is_empty() {
            return Err("a label needs a name".to_owned());
        }
        if folded.chars().count() > MAX_PLAIN_LABEL_CHARS {
            return Err(format!(
                "a label is at most {MAX_PLAIN_LABEL_CHARS} characters"
            ));
        }
        if folded
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | ':'))
        {
            return Err("a label cannot hold a quote, a colon or a control character".to_owned());
        }
        if folded.starts_with('#') {
            return Err("a # label is one word: a letter, then letters, digits, - or _".to_owned());
        }
        if folded == "pinned" {
            return Err(PINNED_RESERVED.to_owned());
        }
        Ok(Self::Plain(folded))
    }

    /// Read a name back as the store keeps it ([`Self::stored`]): no refusal, since only
    /// [`Self::parse`] ever wrote one.
    #[must_use]
    pub fn from_stored(stored: &str) -> Self {
        match stored.strip_prefix('#') {
            Some(slug) => Self::Hashtag(slug.to_owned()),
            None => Self::Plain(stored.to_owned()),
        }
    }

    /// The name as the owner sees it, and as the store keeps it: `#kvm` or `merge queue`.
    #[must_use]
    pub fn stored(&self) -> String {
        match self {
            Self::Hashtag(slug) => format!("#{slug}"),
            Self::Plain(name) => name.clone(),
        }
    }

    /// `hashtag` or `plain`, for a log line that must not carry the name itself.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Hashtag(_) => "hashtag",
            Self::Plain(_) => "plain",
        }
    }
}

const PINNED_RESERVED: &str =
    "\"pinned\" is the built-in Pinned label: use Pin message in the same menu";

/// One message's labels as served: the three fields [`crate::ops::stamp`] writes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Merged {
    /// Hashtag slugs, typed then menu-added, groups applied ([`crate::model::Message::labels`]).
    pub labels: Vec<String>,
    /// Plain names, menu-added ([`crate::model::Message::plain_labels`]).
    pub plain: Vec<String>,
    /// The menu-added names the text does not also carry: the removable ones
    /// ([`crate::model::Message::manual_labels`]).
    pub manual: Vec<String>,
}

/// Merge one message's text-derived and menu-added labels (`#2279`).
///
/// A menu-added `#kvm` joins the typed slugs before `groups` apply, so it is the same label as
/// a typed `#kvm` everywhere `labels` is read. It counts as removable only while the text does
/// not also carry it (compared after grouping, so a typed spelling of the same group keeps it).
#[must_use]
pub fn merge(groups: &GroupIndex, content: &str, manual: &[String]) -> Merged {
    let typed = extract_hashtags(content);
    if manual.is_empty() {
        return Merged {
            labels: groups.apply(&typed),
            ..Merged::default()
        };
    }
    let typed_shown = groups.apply(&typed);
    let mut slugs = typed;
    let mut plain: Vec<String> = Vec::new();
    let mut removable: Vec<String> = Vec::new();
    for stored in manual {
        match LabelName::from_stored(stored) {
            LabelName::Hashtag(slug) => {
                let shown = groups.apply(std::slice::from_ref(&slug));
                if !shown.iter().all(|one| typed_shown.contains(one)) {
                    removable.push(stored.clone());
                }
                if !slugs.contains(&slug) {
                    slugs.push(slug);
                }
            }
            LabelName::Plain(name) => {
                if !plain.contains(&name) {
                    removable.push(stored.clone());
                    plain.push(name);
                }
            }
        }
    }
    Merged {
        labels: groups.apply(&slugs),
        plain,
        manual: removable,
    }
}

/// Every menu-added label, per message, as derivation and search see them (`#2279`): loaded
/// from the store at startup and written through on every change, so [`crate::ops::stamp`]
/// (sync) merges without a store read per serve.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ManualIndex {
    /// Channel id → message id → stored names, in the order they were added.
    by_channel: std::collections::HashMap<String, std::collections::HashMap<String, Vec<String>>>,
    /// The store's revision for these labels.
    revision: i64,
    /// Whether a store stands behind the index: read from one at startup, or written through.
    stored: bool,
}

impl ManualIndex {
    /// Build from the store's rows, oldest first.
    #[must_use]
    pub fn new(rows: &[crate::store::MessageLabelRow], revision: i64) -> Self {
        let mut index = Self {
            revision,
            stored: true,
            ..Self::default()
        };
        for row in rows {
            index
                .by_channel
                .entry(row.channel_id.clone())
                .or_default()
                .entry(row.message_id.clone())
                .or_default()
                .push(row.name.clone());
        }
        index
    }

    /// One message's menu-added names; empty for none.
    #[must_use]
    pub fn of(&self, channel_id: &str, message_id: &str) -> &[String] {
        self.by_channel
            .get(channel_id)
            .and_then(|messages| messages.get(message_id))
            .map_or(&[], Vec::as_slice)
    }

    /// One channel's menu-added names by message id, for a search over the channel's cache.
    #[must_use]
    pub fn channel(&self, channel_id: &str) -> std::collections::HashMap<String, Vec<String>> {
        self.by_channel.get(channel_id).cloned().unwrap_or_default()
    }

    /// Replace one message's names (after a store write), at `revision`.
    pub fn set(&mut self, channel_id: &str, message_id: &str, names: Vec<String>, revision: i64) {
        let messages = self.by_channel.entry(channel_id.to_owned()).or_default();
        if names.is_empty() {
            messages.remove(message_id);
        } else {
            messages.insert(message_id.to_owned(), names);
        }
        if messages.is_empty() {
            self.by_channel.remove(channel_id);
        }
        self.revision = self.revision.max(revision);
        self.stored = true;
    }

    /// The revision these labels are at.
    #[must_use]
    pub fn revision(&self) -> i64 {
        self.revision
    }

    /// The revision, when a store stands behind the index; `None` on a server with no store.
    #[must_use]
    pub fn stored_revision(&self) -> Option<i64> {
        self.stored.then_some(self.revision)
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
    // `#2279 vibe-talk-post-facto-labels`: the menu's name normalizer and the merge rule.

    #[test]
    fn a_menu_name_with_a_hash_follows_the_extractors_token_rule() {
        use super::LabelName;
        assert_eq!(
            LabelName::parse("#KVM"),
            Ok(LabelName::Hashtag("kvm".to_owned()))
        );
        assert_eq!(
            LabelName::parse("  #bench_2-x "),
            Ok(LabelName::Hashtag("bench_2-x".to_owned()))
        );
        for refused in [
            "#", "#1abc", "#123", "#kv m", "#kvm!", "#é", "#pinned", "#Pinned",
        ] {
            assert!(
                LabelName::parse(refused).is_err(),
                "{refused:?} must be refused"
            );
        }
        assert!(LabelName::parse(&format!("#a{}", "b".repeat(64))).is_err());
        // What the extractor would mint from the same text is the same slug.
        assert_eq!(super::extract_hashtags("x #KVM"), vec!["kvm".to_owned()]);
    }

    #[test]
    fn a_menu_name_without_a_hash_is_a_plain_label_folded_like_search() {
        use super::LabelName;
        assert_eq!(
            LabelName::parse("  Merge   Queue "),
            Ok(LabelName::Plain("merge queue".to_owned()))
        );
        assert_eq!(
            LabelName::parse("Café"),
            Ok(LabelName::Plain("cafe".to_owned()))
        );
        assert_eq!(
            LabelName::parse("kvm"),
            Ok(LabelName::Plain("kvm".to_owned()))
        );
        for refused in [
            "",
            "   ",
            "pinned",
            "PINNED",
            "auto:topic",
            "say \"hi\"",
            "a\u{7}b",
        ] {
            assert!(
                LabelName::parse(refused).is_err(),
                "{refused:?} must be refused"
            );
        }
        assert!(LabelName::parse(&"x".repeat(41)).is_err());
        assert!(LabelName::parse(&"x".repeat(40)).is_ok());
        // Stored forms read back to the same name.
        for name in ["#kvm", "merge queue"] {
            assert_eq!(LabelName::from_stored(name).stored(), name);
        }
        assert_eq!(LabelName::parse("#kvm").map(|n| n.kind()), Ok("hashtag"));
        assert_eq!(LabelName::parse("kvm").map(|n| n.kind()), Ok("plain"));
    }

    #[test]
    fn a_menu_added_hashtag_is_the_typed_label_and_only_menu_labels_are_removable() {
        let groups = GroupIndex::default();
        // Typed only: as #1825 served it.
        let typed = super::merge(&groups, "fixed #kvm", &[]);
        assert_eq!(typed.labels, vec!["kvm".to_owned()]);
        assert!(typed.plain.is_empty() && typed.manual.is_empty());
        // Menu-added #kvm on a message with no hashtag: the same slug, removable.
        let added = super::merge(&groups, "fixed", &["#kvm".to_owned()]);
        assert_eq!(added.labels, vec!["kvm".to_owned()]);
        assert_eq!(added.manual, vec!["#kvm".to_owned()]);
        // Both typed and added: one label, not removable (the text carries it).
        let both = super::merge(&groups, "fixed #kvm", &["#kvm".to_owned()]);
        assert_eq!(both.labels, vec!["kvm".to_owned()]);
        assert!(both.manual.is_empty());
        // Typed first, then menu-added, deduped; a plain label rides beside.
        let mixed = super::merge(
            &groups,
            "#perf done",
            &[
                "#kvm".to_owned(),
                "merge queue".to_owned(),
                "merge queue".to_owned(),
            ],
        );
        assert_eq!(mixed.labels, vec!["perf".to_owned(), "kvm".to_owned()]);
        assert_eq!(mixed.plain, vec!["merge queue".to_owned()]);
        assert_eq!(
            mixed.manual,
            vec!["#kvm".to_owned(), "merge queue".to_owned()]
        );
    }

    #[test]
    fn groups_apply_to_menu_added_hashtags_as_to_typed_ones() {
        let groups = GroupIndex::new(vec![(
            "benchmarking".to_owned(),
            vec!["benchmark".to_owned(), "benchmarking".to_owned()],
        )]);
        let added = super::merge(&groups, "fixed", &["#benchmark".to_owned()]);
        assert_eq!(added.labels, vec!["benchmarking".to_owned()]);
        assert_eq!(added.manual, vec!["#benchmark".to_owned()]);
        // A typed spelling of the same group carries it: not removable.
        let carried = super::merge(&groups, "fixed #benchmarking", &["#benchmark".to_owned()]);
        assert_eq!(carried.labels, vec!["benchmarking".to_owned()]);
        assert!(carried.manual.is_empty());
    }

    #[test]
    fn the_manual_index_keys_by_channel_and_message() {
        let rows = vec![
            crate::store::MessageLabelRow {
                channel_id: "C1".to_owned(),
                message_id: "m1".to_owned(),
                name: "#kvm".to_owned(),
                added_ms: 1,
            },
            crate::store::MessageLabelRow {
                channel_id: "C2".to_owned(),
                message_id: "m1".to_owned(),
                name: "merge queue".to_owned(),
                added_ms: 2,
            },
        ];
        let mut index = super::ManualIndex::new(&rows, 7);
        assert_eq!(index.of("C1", "m1"), ["#kvm".to_owned()]);
        assert_eq!(index.of("C2", "m1"), ["merge queue".to_owned()]);
        assert!(index.of("C1", "m2").is_empty());
        index.set("C1", "m1", Vec::new(), 9);
        assert!(index.of("C1", "m1").is_empty());
        assert!(index.channel("C1").is_empty());
        assert_eq!(index.revision(), 9);
    }

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
    fn pinned_is_reserved_and_never_a_hashtag() {
        // `#1853`: `#pinned` names the built-in label, so the extractor
        // yields nothing for it, in any case, beside real labels.
        assert!(labels("kept for later #pinned").is_empty());
        assert!(labels("#Pinned #PINNED").is_empty());
        assert_eq!(labels("#pinned #kvm"), vec!["kvm"]);
        // The built-in never expands through a group either.
        assert_eq!(
            GroupIndex::default().expand("pinned"),
            vec!["pinned".to_owned()]
        );
    }

    #[test]
    fn duplicates_fold_to_first_seen_order() {
        assert_eq!(labels("#b #a #B #a #c"), vec!["b", "a", "c"]);
        assert!(labels("no hashtags here").is_empty());
        assert!(labels("").is_empty());
    }
}
