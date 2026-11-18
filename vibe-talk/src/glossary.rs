//! Proactive glossary for Discuss (`discuss-glossary-preamble`, simple version).
//!
//! The committed file `vibe-talk/prompts/discuss-glossary.md` is the source of
//! truth for definitions: one short line per term, human-reviewed, because a
//! wrong gloss is worse than an unknown term. Nothing here invents a
//! definition. Generation is deterministic over its inputs — the committed
//! entries, the unknown-term frequencies from [`crate::unknown_terms_log`],
//! and the channel window — so the same inputs always select the same warm-up
//! text, and any edit to the glossary file moves the warm-up fingerprint and
//! invalidates the cached warm-up. Tool-call retrieval of definitions is
//! deliberately not built; it is the recorded follow-up.

use std::collections::BTreeMap;

use crate::unknown_terms_log::TermTally;

/// One glossary term and its one-line definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlossaryEntry {
    /// The term as written, e.g. `dagrun`.
    pub term: String,
    /// One line of meaning, without the term.
    pub definition: String,
}

impl GlossaryEntry {
    /// The line the model sees: `- term — definition`.
    #[must_use]
    pub fn prompt_line(&self) -> String {
        format!("- {} — {}", self.term, self.definition)
    }

    /// The line stored in the committed file: `- **term**: definition`.
    #[must_use]
    pub fn file_line(&self) -> String {
        format!("- **{}**: {}", self.term, self.definition)
    }
}

/// Parse glossary markdown into entries, in file order.
///
/// Accepted entry shapes (one per `- ` line): `- **term**: definition`,
/// `- term — definition`, `- term – definition`, and `- term: definition`.
/// Headings, blanks, and other lines are ignored. A repeated term (compared
/// case-insensitively) keeps its first definition, so parsing is deterministic
/// and a duplicate cannot silently replace a reviewed gloss.
#[must_use]
pub fn parse_glossary(text: &str) -> Vec<GlossaryEntry> {
    let mut entries: Vec<GlossaryEntry> = Vec::new();
    for raw_line in text.lines() {
        let Some(rest) = raw_line.trim().strip_prefix("- ") else {
            continue;
        };
        let Some((term, definition)) = split_entry(rest.trim()) else {
            continue;
        };
        let term = term.trim().trim_matches('*').trim().trim_matches('`').trim();
        let definition = definition.trim();
        if term.is_empty() || definition.is_empty() {
            continue;
        }
        if entries
            .iter()
            .any(|held: &GlossaryEntry| held.term.eq_ignore_ascii_case(term))
        {
            continue;
        }
        entries.push(GlossaryEntry {
            term: term.to_owned(),
            definition: definition.to_owned(),
        });
    }
    entries
}

/// Split one entry body into `(term, definition)`, or `None` when it has no separator.
fn split_entry(body: &str) -> Option<(&str, &str)> {
    // Em dash first: it is the prompt form and never appears inside `**term**`.
    for separator in [" — ", " – ", " - "] {
        if let Some(at) = body.find(separator) {
            return Some((&body[..at], &body[at + separator.len()..]));
        }
    }
    // `**term**: definition` and `term: definition`.
    if let Some(at) = body.find("**:") {
        return Some((&body[..at + 2], &body[at + 3..]));
    }
    body.find(": ").map(|at| (&body[..at], &body[at + 2..]))
}

/// Render entries as the committed file body (without the `#` title).
#[must_use]
pub fn render_glossary_file(entries: &[GlossaryEntry]) -> String {
    let mut out = String::from("# glossary (Discuss warm-up)\n");
    for entry in entries {
        out.push('\n');
        out.push_str(&entry.file_line());
        out.push('\n');
    }
    out
}

/// Words in `text` (whitespace-separated), the count the owner asked for.
#[must_use]
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Estimated tokens in `text`.
///
/// vibe-talk carries no tokenizer dependency, so this is the standard English
/// estimate of one token per four characters, rounded up. It is an estimate,
/// not a tokenizer count; the report that uses it says so.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Size of a glossary text: entries, words, characters, estimated tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlossaryStats {
    /// Parsed entries.
    pub entries: usize,
    /// Whitespace-separated words in the whole text.
    pub words: usize,
    /// Characters in the whole text.
    pub chars: usize,
    /// Estimated tokens ([`estimate_tokens`]).
    pub est_tokens: usize,
}

/// Measure `text` with its parsed `entries` count.
#[must_use]
pub fn glossary_stats(text: &str, entries: usize) -> GlossaryStats {
    GlossaryStats {
        entries,
        words: word_count(text),
        chars: text.chars().count(),
        est_tokens: estimate_tokens(text),
    }
}

/// Rank unknown-term tallies into candidate terms, most frequent first.
///
/// [`crate::unknown_terms_log::aggregate`] already sorts by count descending
/// then term ascending; this keeps that order, so ranking is deterministic.
#[must_use]
pub fn rank_candidates(tallies: &[TermTally]) -> Vec<String> {
    tallies.iter().map(|tally| tally.term.clone()).collect()
}

/// Build the generated glossary deterministically, inventing no definitions.
///
/// Order: `ranked_terms` that have a definition (from `base` or
/// `definitions`) in rank order, then every remaining `base` entry in term
/// order. A ranked term with no definition anywhere is omitted — it is a
/// candidate needing a reviewed definition, not an entry.
#[must_use]
pub fn build_glossary(
    base: &[GlossaryEntry],
    ranked_terms: &[String],
    definitions: &BTreeMap<String, String>,
) -> Vec<GlossaryEntry> {
    let lookup = |term: &str| -> Option<GlossaryEntry> {
        if let Some(held) = base.iter().find(|e| e.term.eq_ignore_ascii_case(term)) {
            return Some(held.clone());
        }
        definitions
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(term))
            .map(|(key, definition)| GlossaryEntry {
                term: key.clone(),
                definition: definition.clone(),
            })
    };
    let mut out: Vec<GlossaryEntry> = Vec::new();
    for term in ranked_terms {
        if out.iter().any(|e: &GlossaryEntry| e.term.eq_ignore_ascii_case(term)) {
            continue;
        }
        if let Some(entry) = lookup(term) {
            out.push(entry);
        }
    }
    let mut rest: Vec<GlossaryEntry> = base
        .iter()
        .filter(|e| !out.iter().any(|held| held.term.eq_ignore_ascii_case(&e.term)))
        .cloned()
        .collect();
    rest.sort_by(|a, b| {
        a.term
            .to_lowercase()
            .cmp(&b.term.to_lowercase())
            .then_with(|| a.term.cmp(&b.term))
    });
    out.extend(rest);
    out
}

/// Occurrences of `needle` in `haystack` (both already lowercased), non-overlapping.
fn occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.match_indices(needle).count()
}

/// Entries in relevance order for one window: window hits descending, then
/// unknown-term tally descending, then term ascending. A total order, so the
/// ranking is deterministic for the same inputs.
#[must_use]
pub fn ranked_entries(
    entries: &[GlossaryEntry],
    window_text: &str,
    tallies: &[TermTally],
) -> Vec<GlossaryEntry> {
    let window_lower = window_text.to_lowercase();
    let tally_of = |term: &str| -> usize {
        tallies
            .iter()
            .find(|tally| tally.term.eq_ignore_ascii_case(term))
            .map(|tally| tally.count)
            .unwrap_or(0)
    };
    let mut ranked: Vec<(usize, usize, GlossaryEntry)> = entries
        .iter()
        .map(|entry| {
            (
                occurrences(&window_lower, &entry.term.to_lowercase()),
                tally_of(&entry.term),
                entry.clone(),
            )
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.cmp(&a.1))
            .then_with(|| {
                a.2.term
                    .to_lowercase()
                    .cmp(&b.2.term.to_lowercase())
                    .then_with(|| a.2.term.cmp(&b.2.term))
            })
    });
    ranked.into_iter().map(|(_, _, entry)| entry).collect()
}

/// The entries kept for one window under a character budget, and how many were dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// Kept entries, most relevant first.
    pub kept: Vec<GlossaryEntry>,
    /// Entries left out to fit the budget.
    pub dropped: usize,
}

/// Warm-up header shared with `catchup::assemble_warmup`.
pub const WARMUP_HEADER: &str = "Reference material for the work this channel discusses (background only; \
     the messages after it are the news):";

/// Select the entries Discuss should see for `window_text` within `max_chars`.
///
/// Relevance is mechanical: count each term's occurrences in the window
/// (case-insensitive), then rank by window hits descending, unknown-term
/// tally descending, term ascending — a total order, so the same glossary,
/// window, tallies, and budget always keep the same entries in the same
/// order. Entries are added greedily in rank order while the assembled
/// warm-up (header, heading, lines) stays within `max_chars`; an entry that
/// does not fit is skipped and a shorter later one may still fit. With no
/// window hits the order falls back to tally then term, so a small glossary
/// is included whole whenever it fits.
#[must_use]
pub fn select_for_window(
    entries: &[GlossaryEntry],
    window_text: &str,
    tallies: &[TermTally],
    max_chars: usize,
) -> Selection {
    if entries.is_empty() || max_chars == 0 {
        return Selection {
            kept: Vec::new(),
            dropped: entries.len(),
        };
    }
    let ranked = ranked_entries(entries, window_text, tallies);

    // Assemble incrementally so the budget covers header and heading too.
    let mut text = String::from(WARMUP_HEADER);
    text.push_str("\n\n## Reference: discuss-glossary.md\n");
    let mut kept: Vec<GlossaryEntry> = Vec::new();
    for entry in &ranked {
        let line = entry.prompt_line();
        // Each kept line costs its text plus one newline separator.
        if text.chars().count() + line.chars().count() + 1 > max_chars {
            continue;
        }
        text.push_str(&line);
        text.push('\n');
        kept.push(entry.clone());
    }
    Selection {
        dropped: entries.len() - kept.len(),
        kept,
    }
}

/// Assemble the glossary warm-up for one window: header, heading, kept lines.
///
/// The result never exceeds `max_chars` characters (empty when the budget is
/// zero or no entry fits). When entries were dropped and the note itself
/// fits, it ends with `[glossary: N term(s) omitted to fit budget]`.
#[must_use]
pub fn assemble_glossary_warmup(
    entries: &[GlossaryEntry],
    window_text: &str,
    tallies: &[TermTally],
    max_chars: usize,
) -> (String, Selection) {
    let selection = select_for_window(entries, window_text, tallies, max_chars);
    if selection.kept.is_empty() {
        return (String::new(), selection);
    }
    let mut text = String::from(WARMUP_HEADER);
    text.push_str("\n\n## Reference: discuss-glossary.md\n");
    for entry in &selection.kept {
        text.push_str(&entry.prompt_line());
        text.push('\n');
    }
    let mut text = text.trim_end().to_owned();
    if selection.dropped > 0 {
        let note = format!("\n[glossary: {} term(s) omitted to fit budget]", selection.dropped);
        if text.chars().count() + note.chars().count() <= max_chars {
            text.push_str(&note);
        }
    }
    (text, selection)
}

/// Fingerprint of a selection: file fingerprint combined with the text hash.
///
/// Mixing in the assembled text means any glossary edit that changes what
/// Discuss sees changes the fingerprint, even at the same file length and
/// modification instant.
#[must_use]
pub fn selection_fingerprint(files_fingerprint: u64, text: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    files_fingerprint.hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// Parsed `glossary-report` options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlossaryReportOptions {
    /// Glossary markdown file to measure.
    pub glossary_path: std::path::PathBuf,
    /// Optional window text file for relevance and coverage counts.
    pub window_path: Option<std::path::PathBuf>,
    /// Warm-up budget in characters.
    pub max_chars: usize,
}

/// Parse `vibe-talk glossary-report` arguments. `Ok(None)` = help shown.
pub fn parse_glossary_report_args(
    args: impl Iterator<Item = String>,
) -> anyhow::Result<Option<GlossaryReportOptions>> {
    let mut max_chars: usize = 6_000;
    let mut window_path: Option<std::path::PathBuf> = None;
    let mut glossary_path: Option<std::path::PathBuf> = None;
    let mut it = args.peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!(
                    "usage: vibe-talk glossary-report [--max-chars N] [--window FILE] <glossary.md>\n\
                     \n\
                     Measure a Discuss glossary: entries, words, characters, and estimated\n\
                     tokens (one token per four characters, rounded up — an estimate, as\n\
                     vibe-talk carries no tokenizer). With --window, also report how many\n\
                     entries relevance selection keeps within the budget, how many it\n\
                     drops, and the window terms covered before (none) and after.\n"
                );
                return Ok(None);
            }
            "--max-chars" => {
                let Some(value) = it.next() else {
                    anyhow::bail!("--max-chars needs a value");
                };
                max_chars = value
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--max-chars must be a number"))?;
            }
            "--window" => {
                let Some(value) = it.next() else {
                    anyhow::bail!("--window needs a value");
                };
                window_path = Some(std::path::PathBuf::from(value));
            }
            _ if arg.starts_with("--max-chars=") => {
                max_chars = arg["--max-chars=".len()..]
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--max-chars must be a number"))?;
            }
            _ if arg.starts_with("--window=") => {
                window_path = Some(std::path::PathBuf::from(&arg["--window=".len()..]));
            }
            _ if arg.starts_with('-') => anyhow::bail!("unknown option {arg}"),
            _ => {
                if glossary_path.is_some() {
                    anyhow::bail!("glossary-report takes one glossary path");
                }
                glossary_path = Some(std::path::PathBuf::from(arg));
            }
        }
    }
    let Some(glossary_path) = glossary_path else {
        anyhow::bail!("usage: vibe-talk glossary-report [--max-chars N] [--window FILE] <glossary.md>");
    };
    Ok(Some(GlossaryReportOptions {
        glossary_path,
        window_path,
        max_chars,
    }))
}

/// Run the glossary report. Pure over file contents, so tests need no clock.
#[must_use]
pub fn run_glossary_report(options: &GlossaryReportOptions) -> String {
    let text = std::fs::read_to_string(&options.glossary_path)
        .unwrap_or_else(|error| format!("<unreadable: {error}>"));
    let entries = parse_glossary(&text);
    let stats = glossary_stats(&text, entries.len());
    let fits = stats.chars <= options.max_chars;
    let mut out = format!(
        "glossary {}: {} entries, {} words, {} chars, ~{} tokens (estimate: 1 token / 4 chars)\n\
         budget {} chars: {}\n",
        options.glossary_path.display(),
        stats.entries,
        stats.words,
        stats.chars,
        stats.est_tokens,
        options.max_chars,
        if fits { "fits whole" } else { "does not fit whole" },
    );
    if let Some(window_path) = &options.window_path {
        let window = std::fs::read_to_string(window_path).unwrap_or_default();
        let (warmup, selection) = assemble_glossary_warmup(&entries, &window, &[], options.max_chars);
        let window_lower = window.to_lowercase();
        let present = entries
            .iter()
            .filter(|entry| occurrences(&window_lower, &entry.term.to_lowercase()) > 0)
            .count();
        out.push_str(&format!(
            "window {}: {} of {} glossary terms appear; kept {}, dropped {}; \
             covered before 0, after {}; warm-up {} chars\n",
            window_path.display(),
            present,
            entries.len(),
            selection.kept.len(),
            selection.dropped,
            selection
                .kept
                .iter()
                .filter(|entry| occurrences(&window_lower, &entry.term.to_lowercase()) > 0)
                .count(),
            warmup.chars().count(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, definition: &str) -> GlossaryEntry {
        GlossaryEntry {
            term: term.to_owned(),
            definition: definition.to_owned(),
        }
    }

    fn tally(term: &str, count: usize) -> TermTally {
        TermTally {
            term: term.to_owned(),
            count,
            first_seen: "2026-11-01T00:00:00Z".to_owned(),
            last_seen: "2026-11-02T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn parses_every_entry_shape_and_keeps_the_first_duplicate() {
        let text = "# title\n\n- **dagrun**: the validation runner.\n- lane — one agent workstream.\n- slot – an isolated worktree.\n- gate: the validation gate.\n- **dagrun**: a second gloss that must lose.\nnot an entry\n";
        let entries = parse_glossary(text);
        assert_eq!(entries.len(), 4, "{entries:?}");
        assert_eq!(entries[0], entry("dagrun", "the validation runner."));
        assert_eq!(entries[1].term, "lane");
        assert_eq!(entries[2].term, "slot");
        assert_eq!(entries[3].term, "gate");
    }

    #[test]
    fn generation_invents_no_definition_and_is_deterministic() {
        let base = vec![entry("lane", "one workstream."), entry("gate", "the gate.")];
        let ranked = vec!["gate".to_owned(), "mystery".to_owned(), "lane".to_owned()];
        let built = build_glossary(&base, &ranked, &BTreeMap::new());
        // `mystery` has no definition anywhere, so it is omitted, not glossed.
        assert_eq!(
            built.iter().map(|e| e.term.as_str()).collect::<Vec<_>>(),
            vec!["gate", "lane"]
        );
        assert_eq!(built, build_glossary(&base, &ranked, &BTreeMap::new()));
        let file = render_glossary_file(&built);
        assert_eq!(parse_glossary(&file), built);
    }

    #[test]
    fn selection_puts_window_terms_first_and_is_deterministic() {
        let entries = vec![
            entry("alpha", "first term."),
            entry("dagrun", "the validation runner."),
            entry("gate", "the validation gate."),
        ];
        let window = "please run dagrun before the gate, dagrun again";
        let first = select_for_window(&entries, window, &[], 6_000);
        let second = select_for_window(&entries, window, &[], 6_000);
        assert_eq!(first, second, "same inputs must select identically");
        assert_eq!(first.dropped, 0);
        assert_eq!(first.kept[0].term, "dagrun");
        assert_eq!(first.kept[1].term, "gate");
    }

    #[test]
    fn selection_respects_the_budget_and_reports_drops() {
        let entries: Vec<GlossaryEntry> = (0..40)
            .map(|i| entry(&format!("term{i:02}"), "a short definition."))
            .collect();
        let (text, selection) = assemble_glossary_warmup(&entries, "term00 term01", &[], 400);
        assert!(
            text.chars().count() <= 400,
            "{} chars over budget: {text}",
            text.chars().count()
        );
        assert!(selection.dropped > 0, "40 entries cannot fit 400 chars");
        assert_eq!(selection.kept.len() + selection.dropped, entries.len());
        assert_eq!(selection.kept[0].term, "term00");
    }

    #[test]
    fn zero_budget_selects_nothing() {
        let entries = vec![entry("lane", "one workstream.")];
        let (text, selection) = assemble_glossary_warmup(&entries, "lane", &[], 0);
        assert!(text.is_empty());
        assert_eq!(selection.dropped, 1);
    }

    #[test]
    fn stats_count_words_and_estimate_tokens() {
        let text = "- lane — one workstream.\n- gate — the gate.\n";
        let stats = glossary_stats(text, 2);
        assert_eq!(stats.entries, 2);
        assert_eq!(stats.words, word_count(text));
        assert_eq!(stats.est_tokens, estimate_tokens(text));
        assert!(stats.est_tokens > 0);
    }

    #[test]
    fn fingerprint_moves_when_the_selection_moves() {
        let before = selection_fingerprint(7, "- lane — one workstream.");
        let after = selection_fingerprint(7, "- lane — one renamed workstream.");
        assert_ne!(before, after);
        assert_eq!(before, selection_fingerprint(7, "- lane — one workstream."));
    }
}
