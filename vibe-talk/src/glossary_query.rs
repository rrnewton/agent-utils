//! Query planning for WTF lookups (`#2232 vibe-talk-wtf-lookup-log-and-replay`).
//!
//! A reader's selection is often longer than the term it asks about: "Row 7, the
//! x-gate test" asks what "Row 7" is, and the words after the comma only qualify
//! it. The first planner searched every window of the selection longest-first
//! and stopped at a fixed count, so a six-word selection spent its whole budget on
//! long windows that match nothing and never searched its two-word head. This
//! module splits a selection into its head noun phrase and its qualifier, and
//! plans searches in the order most likely to find a definition: the head, the
//! qualifier, the whole selection, then shorter windows and single rare words.
//! Each planned search ORs the spelling forms of one window (spaced, hyphenated,
//! singular, joined) into one query, so a form costs no extra search.
//!
//! Pure functions only: the lookup handler owns the searches, the budget and
//! the clock.

/// The most all-channel searches one lookup may run (global windows plus link
/// keys). Each is a full server search over the message cache.
pub const QUERY_BUDGET: usize = 10;
/// The most link-key searches inside [`QUERY_BUDGET`].
pub const LINK_QUERY_BUDGET: usize = 3;
/// Wall-clock budget for the retrieval phase, in milliseconds. Checked before
/// each search; a search already running finishes. Generous on purpose: it caps
/// a pathological lookup, it is not a latency target, and no test depends on it.
pub const RETRIEVAL_TIME_BUDGET_MS: u64 = 4_000;
/// Global searching stops once this many strong candidates are held.
pub const STRONG_ENOUGH: usize = 3;
/// The most searches a plan lists (the budget usually stops earlier).
pub const MAX_PLANNED: usize = 24;

/// Words that never make a query on their own and are dropped from the front
/// of a head or qualifier.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "in", "is", "it", "its", "of",
    "on", "or", "our", "that", "the", "their", "this", "to", "was", "with",
];

/// Clause marks that end a head noun phrase, in the order they are tried.
const CLAUSE_MARKS: &[&str] = &[", ", "; ", ": ", " - ", " \u{2014} ", " \u{2013} ", " ("];

/// Why the search loop stopped before the plan ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// Enough strong candidates were held.
    Strong,
    /// [`QUERY_BUDGET`] was spent.
    QueryBudget,
    /// [`RETRIEVAL_TIME_BUDGET_MS`] was spent.
    TimeBudget,
}

impl Stop {
    /// The log's name for the stop.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::QueryBudget => "query_budget",
            Self::TimeBudget => "time_budget",
        }
    }
}

/// Whether the search loop may run one more search, and if not, why.
///
/// # Errors
///
/// The [`Stop`] that ends the loop.
pub fn next_search_allowed(
    searches_run: usize,
    elapsed_ms: u64,
    strong_held: usize,
    budget: usize,
    time_budget_ms: u64,
) -> Result<(), Stop> {
    if strong_held >= STRONG_ENOUGH {
        Err(Stop::Strong)
    } else if searches_run >= budget {
        Err(Stop::QueryBudget)
    } else if elapsed_ms >= time_budget_ms {
        Err(Stop::TimeBudget)
    } else {
        Ok(())
    }
}

/// A selection split into its head noun phrase and its qualifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadSplit {
    /// The head noun phrase ("Row 7"); the whole cleaned selection when no
    /// clause mark splits it.
    pub head: String,
    /// The words after the first clause mark, leading determiners dropped
    /// ("x-gate test"); `None` without a clause mark or when nothing is left.
    pub qualifier: Option<String>,
}

/// What kind of window a planned query searches for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryKind {
    /// The head noun phrase.
    Head,
    /// The qualifier.
    Qualifier,
    /// The whole selection.
    Full,
    /// A shorter multi-word window.
    Window,
    /// One informative word.
    Word,
}

impl QueryKind {
    /// The log's name for the kind.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Head => "head",
            Self::Qualifier => "qualifier",
            Self::Full => "full",
            Self::Window => "window",
            Self::Word => "word",
        }
    }
}

/// One planned search: the query in the search language, the human form the
/// not-found answer names, and its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedQuery {
    /// Search-language query: quoted phrases ORed together.
    pub query: String,
    /// The spaced human form, for the reader and the log.
    pub human: String,
    /// What the window is.
    pub kind: QueryKind,
}

/// One word with surrounding punctuation and quotes trimmed.
#[must_use]
pub fn clean_word(word: &str) -> String {
    word.trim_matches(|c: char| {
        matches!(
            c,
            ',' | ';'
                | ':'
                | '.'
                | '!'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '"'
                | '\''
                | '`'
                | '\u{201c}'
                | '\u{201d}'
                | '\u{2018}'
                | '\u{2019}'
        )
    })
    .replace('"', "")
}

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word.to_lowercase().as_str())
}

fn is_number(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '#'))
}

/// Whether a word can carry a search on its own: not a stopword, not a bare
/// number, at least three characters.
#[must_use]
pub fn is_informative(word: &str) -> bool {
    word.chars().count() >= 3 && !is_stopword(word) && !is_number(word)
}

/// Cleaned words of `text`, empty ones dropped.
#[must_use]
pub fn clean_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(clean_word)
        .filter(|w| !w.is_empty())
        .collect()
}

fn drop_leading_stopwords(words: Vec<String>) -> Vec<String> {
    let first = words
        .iter()
        .position(|w| !is_stopword(w))
        .unwrap_or(words.len());
    words[first..].to_vec()
}

/// Narrow a head that opens with an ordinal label and runs on ("Box 5 again",
/// "Tier 4 hosts") to the label: a definition names the label, rarely the
/// run-on words. Returns the run-on words.
fn narrow_to_ordinal_label(head: &mut Vec<String>) -> Vec<String> {
    for take in 2..=3 {
        if head.len() > take && is_ordinal_head(&head[..take].join(" ")) {
            return head.split_off(take);
        }
    }
    Vec::new()
}

/// Split a selection at its first clause mark (`, ` `; ` `: ` ` - ` ` — ` ` (`)
/// into head and qualifier. Leading determiners are dropped from both. A head
/// that runs on past an ordinal label narrows to the label; its run-on words
/// are the qualifier when the selection has no clause of its own.
#[must_use]
pub fn split_head(selected: &str) -> HeadSplit {
    let trimmed = selected.trim();
    let cut = CLAUSE_MARKS
        .iter()
        .filter_map(|mark| trimmed.find(mark).map(|at| (at, mark.len())))
        .min_by_key(|(at, _)| *at);
    let (head_text, rest) = match cut {
        Some((at, len)) if at > 0 => (&trimmed[..at], Some(&trimmed[at + len..])),
        _ => (trimmed, None),
    };
    let mut head_words = drop_leading_stopwords(clean_words(head_text));
    if head_words.is_empty() {
        head_words = clean_words(trimmed);
    }
    let spill = narrow_to_ordinal_label(&mut head_words);
    let informative = |words: Vec<String>| {
        Some(drop_leading_stopwords(words))
            .filter(|words| words.iter().any(|w| is_informative(w)))
            .map(|words| words.join(" "))
    };
    let qualifier = rest
        .and_then(|rest| informative(clean_words(rest)))
        .or_else(|| informative(spill));
    HeadSplit {
        head: head_words.join(" "),
        qualifier,
    }
}

/// Whether a head reads as a local ordinal label: a short phrase whose last
/// word carries a digit ("Row 7", "Phase 2b", "Step 12"). Evidence for such a
/// head must name it exactly, never a longer number ("Row 70").
#[must_use]
pub fn is_ordinal_head(head: &str) -> bool {
    let words: Vec<&str> = head.split_whitespace().collect();
    (2..=3).contains(&words.len())
        && words
            .last()
            .is_some_and(|w| w.chars().any(|c| c.is_ascii_digit()) && w.chars().count() <= 4)
        && words[0].chars().any(char::is_alphabetic)
}

/// Cheap singular stem: drop a trailing `s` (never `ss`) on longer words.
#[must_use]
pub fn stem(word: &str) -> String {
    if word.len() > 3 && word.ends_with('s') && !word.ends_with("ss") {
        word[..word.len() - 1].to_owned()
    } else {
        word.to_owned()
    }
}

/// Fold for matching: the search fold with hyphens read as spaces.
#[must_use]
pub fn match_fold(text: &str) -> String {
    crate::search_query::fold(&text.replace(['-', '\u{2010}', '\u{2011}'], " "))
}

/// Whether folded `haystack` holds folded `needle` on word boundaries: the
/// characters around each occurrence are not letters or digits (a trailing
/// plural `s` is allowed). `row 7` is in `row 7: …` but not in `row 70`.
#[must_use]
pub fn bounded_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        let before_ok = haystack[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let mut after = haystack[end..].chars();
        let after_ok = match after.next() {
            None => true,
            Some('s') => after.next().is_none_or(|c| !c.is_alphanumeric()),
            Some(c) => !c.is_alphanumeric(),
        };
        if before_ok && after_ok {
            return true;
        }
        from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// Whether `head` is followed by a definitional cue in folded `text`: the head
/// on word boundaries, then `:`, `=`, ` is `, ` are `, ` means `, ` - ` or `(`.
#[must_use]
pub fn defines_head(text: &str, head: &str) -> bool {
    if head.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(found) = text[from..].find(head) {
        let start = from + found;
        let end = start + head.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric());
        let tail = &text[end..];
        let cue = [
            ":", " :", "=", " =", " is ", " are ", " means ", " - ", " (", "(",
        ]
        .iter()
        .any(|cue| tail.starts_with(cue));
        if before_ok && cue {
            return true;
        }
        from = start + text[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

fn quote(form: &str) -> String {
    format!("\"{}\"", form.replace('"', ""))
}

/// The spelling forms of one window, best first: spaced (hyphens as spaces),
/// as written, singular, and (for the whole selection) joined.
fn forms(words: &[String], joined: bool) -> Vec<String> {
    let written = words.join(" ");
    let spaced = words
        .iter()
        .map(|w| w.replace('-', " "))
        .collect::<Vec<_>>()
        .join(" ");
    let singular = spaced.split(' ').map(stem).collect::<Vec<_>>().join(" ");
    let mut out: Vec<String> = Vec::new();
    for form in [spaced, written, singular] {
        let form = form.trim().to_owned();
        if !form.is_empty() && !out.iter().any(|held| held.eq_ignore_ascii_case(&form)) {
            out.push(form);
        }
    }
    if joined && words.len() > 1 {
        let joined: String = words.iter().map(|w| w.replace('-', "")).collect();
        if !out.iter().any(|held| held.eq_ignore_ascii_case(&joined)) {
            out.push(joined);
        }
    }
    out
}

/// The searches for a selection, best first (see the module doc). Every
/// planned query is distinct; a window with no informative word is skipped;
/// a bare number or stopword is never a query.
#[must_use]
pub fn plan(selected: &str) -> Vec<PlannedQuery> {
    let split = split_head(selected);
    let words = drop_leading_stopwords(clean_words(selected));
    let mut out: Vec<PlannedQuery> = Vec::new();
    let mut push = |words: &[String], kind: QueryKind, joined: bool| {
        if words.is_empty() || !words.iter().any(|w| is_informative(w) || is_number(w)) {
            return;
        }
        if words.len() == 1 && !is_informative(&words[0]) {
            return;
        }
        let forms = forms(words, joined);
        let Some(human) = forms.first().cloned() else {
            return;
        };
        if out
            .iter()
            .any(|held| held.human.eq_ignore_ascii_case(&human))
        {
            return;
        }
        let query = forms
            .iter()
            .map(|f| quote(f))
            .collect::<Vec<_>>()
            .join(" OR ");
        out.push(PlannedQuery { query, human, kind });
    };
    let head_words = clean_words(&split.head);
    push(&head_words, QueryKind::Head, false);
    if let Some(qualifier) = &split.qualifier {
        push(&clean_words(qualifier), QueryKind::Qualifier, false);
    }
    push(&words, QueryKind::Full, true);
    for take in (2..words.len()).rev() {
        for start in 0..=(words.len() - take) {
            push(&words[start..start + take], QueryKind::Window, false);
        }
    }
    let mut singles: Vec<&String> = words.iter().filter(|w| is_informative(w)).collect();
    singles.sort_by_key(|w| std::cmp::Reverse(w.chars().count()));
    for word in singles {
        push(std::slice::from_ref(word), QueryKind::Word, false);
    }
    out.truncate(MAX_PLANNED);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comma_splits_the_head_from_its_qualifier() {
        let split = split_head("Row 7, the alpha-12-v3 gate test");
        assert_eq!(split.head, "Row 7");
        assert_eq!(split.qualifier.as_deref(), Some("alpha-12-v3 gate test"));
        let split = split_head("the Family A test fix");
        assert_eq!(split.head, "Family A test fix");
        assert_eq!(split.qualifier, None);
        let split = split_head("Phase 2 (the slow one)");
        assert_eq!(split.head, "Phase 2");
        assert_eq!(split.qualifier.as_deref(), Some("slow one"));
        // A qualifier of stopwords alone is no qualifier.
        assert_eq!(split_head("Row 7, the").qualifier, None);
        // Hyphenated words are not clause marks.
        assert_eq!(split_head("x-gate test").head, "x-gate test");
    }

    #[test]
    fn a_head_that_runs_on_past_an_ordinal_label_narrows_to_the_label() {
        let split = split_head("Box 5 again, the flaky runner");
        assert_eq!(split.head, "Box 5");
        assert_eq!(split.qualifier.as_deref(), Some("flaky runner"));
        let split = split_head("Tier 4 hosts");
        assert_eq!(split.head, "Tier 4");
        assert_eq!(split.qualifier.as_deref(), Some("hosts"));
        assert_eq!(split_head("Phase 2b rollout").head, "Phase 2b");
        // A bare label, or a head with no label in front, is unchanged.
        assert_eq!(split_head("Row 7, the alpha-12-v3 gate test").head, "Row 7");
        assert_eq!(
            split_head("the Family A test fix").head,
            "Family A test fix"
        );
        assert_eq!(split_head("v3 gate test").head, "v3 gate test");
        let plan = plan("Box 5 again, the flaky runner");
        assert_eq!(plan[0].kind, QueryKind::Head);
        assert_eq!(plan[0].query, "\"Box 5\"");
    }

    #[test]
    fn the_plan_searches_the_head_first_and_never_a_bare_number() {
        let plan = plan("Row 7, the alpha-12-v3 gate test");
        assert_eq!(plan[0].kind, QueryKind::Head);
        assert_eq!(plan[0].human, "Row 7");
        assert_eq!(plan[0].query, "\"Row 7\"");
        assert_eq!(plan[1].kind, QueryKind::Qualifier);
        assert_eq!(plan[1].human, "alpha 12 v3 gate test");
        assert!(
            plan[1].query.contains("\"alpha-12-v3 gate test\""),
            "{plan:?}"
        );
        assert_eq!(plan[2].kind, QueryKind::Full);
        assert!(
            plan[2].query.contains("Row7thealpha12v3gatetest"),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|q| q.human == "7" || q.human == "the"),
            "{plan:?}"
        );
        assert!(plan.iter().any(|q| q.human == "gate"), "{plan:?}");
        // The head is inside any budget.
        assert!(plan.iter().take(QUERY_BUDGET).any(|q| q.human == "Row 7"));
    }

    #[test]
    fn a_short_selection_plans_its_windows_and_words() {
        let plan = plan("the Family A test fix");
        let humans: Vec<&str> = plan.iter().map(|q| q.human.as_str()).collect();
        assert_eq!(humans[0], "Family A test fix");
        assert!(humans.contains(&"Family A"), "{humans:?}");
        assert!(humans.contains(&"Family"), "{humans:?}");
        assert!(!humans.contains(&"A"), "{humans:?}");
    }

    #[test]
    fn bounded_matching_keeps_row_7_apart_from_row_70() {
        assert!(bounded_contains("status. row 7: widget build", "row 7"));
        assert!(!bounded_contains("row 70: packaging", "row 7"));
        assert!(!bounded_contains("arrow 7", "row 7"));
        assert!(bounded_contains("the gates are green", "gate"));
        assert!(!bounded_contains("gateway", "gate"));
        assert!(bounded_contains("row 70 and row 7.", "row 7"));
    }

    #[test]
    fn a_definitional_cue_follows_the_head() {
        assert!(defines_head("status table. row 7: widget build", "row 7"));
        assert!(defines_head("row 7 is the widget build", "row 7"));
        assert!(!defines_head("row 7, the gate test is green", "row 7"));
        assert!(!defines_head("row 70: packaging", "row 7"));
    }

    #[test]
    fn ordinal_heads_are_label_and_number() {
        assert!(is_ordinal_head("Row 7"));
        assert!(is_ordinal_head("Phase 2b"));
        assert!(!is_ordinal_head("Family A"));
        assert!(!is_ordinal_head("7"));
        assert!(!is_ordinal_head("the alpha-12-v3 gate test"));
    }

    #[test]
    fn the_budget_stops_on_strength_count_then_time() {
        assert_eq!(next_search_allowed(0, 0, 0, 10, 4_000), Ok(()));
        assert_eq!(next_search_allowed(1, 0, 3, 10, 4_000), Err(Stop::Strong));
        assert_eq!(
            next_search_allowed(10, 0, 0, 10, 4_000),
            Err(Stop::QueryBudget)
        );
        assert_eq!(
            next_search_allowed(2, 4_000, 0, 10, 4_000),
            Err(Stop::TimeBudget)
        );
    }
}
