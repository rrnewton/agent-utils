//! Dynamic glossary entries learned by WTF lookups (`#2156 vibe-talk-wtf-glossary-lookup`).
//!
//! A dynamic entry is a machine-written gloss (term, one-line definition, best
//! source link) produced when a reader selects an unfamiliar term and presses
//! WTF?. Entries live in deployment state — never in this repository — in
//! their own subdirectory of the state directory, beside the unknown-terms
//! log they complement. The committed, human-reviewed glossary
//! (`vibe-talk/prompts/discuss-glossary.md`, parsed by [`crate::glossary`])
//! always wins over a dynamic row for the same term: a wrong reviewed gloss
//! is worse than an unknown term, and a machine gloss must never shadow one.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Subdirectory under the state directory holding the dynamic glossary.
pub const GLOSSARY_SUBDIR: &str = "glossary";
/// Live file name inside [`GLOSSARY_SUBDIR`].
pub const GLOSSARY_FILE: &str = "dynamic-glossary.jsonl";
/// Rotated backup kept beside the live file.
pub const GLOSSARY_BACKUP: &str = "dynamic-glossary.jsonl.1";
/// Total cap per file; rotation keeps live plus one backup.
pub const DEFAULT_MAX_BYTES: u64 = 1_048_576;
/// The longest selected text a lookup accepts, in characters.
pub const MAX_SELECTION_CHARS: usize = 200;
/// The most words a selection (or a chosen term) may hold.
pub const MAX_TERM_WORDS: usize = 8;
/// The longest context line a lookup accepts, in characters.
pub const MAX_CONTEXT_CHARS: usize = 500;
/// The longest definition stored or served, in characters.
pub const MAX_DEFINITION_CHARS: usize = 240;

/// Whether learned (dynamic) entries may join the Discuss warm-up.
///
/// Off by the owner's rule for the first slice: learned entries are
/// popover-only (stored, shown in the popover and as glossary links) and
/// the warm-up stays reviewed-only. Flipping this one constant is the
/// whole enable change once the owner approves it.
pub const DYNAMIC_IN_WARMUP: bool = false;

/// Where a definition's evidence lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GlossarySource {
    /// A cached message, linked in-app.
    Message {
        /// Channel the message lives in.
        channel: String,
        /// Message id.
        message: String,
        /// In-app href (`catchup::message_link`).
        href: String,
        /// Human label for the popover, e.g. `message, Tue 09:51`.
        label: String,
    },
    /// An already-fetched document, narrowed to a section where possible.
    Document {
        /// Document key.
        key: String,
        /// Section heading, when one resolved.
        heading: Option<String>,
        /// Document href.
        href: String,
        /// Human label for the popover.
        label: String,
    },
}

/// One dynamic glossary row as stored on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DynamicEntry {
    /// Row shape version.
    pub v: u32,
    /// The term as displayed.
    pub term: String,
    /// Normalized identity ([`normalize`]).
    pub norm: String,
    /// One-line definition. Empty on a tombstone row.
    #[serde(default)]
    pub definition: String,
    /// Best source, or `None` on a tombstone row.
    #[serde(default)]
    pub source: Option<GlossarySource>,
    /// Model that wrote the definition (audit only).
    #[serde(default)]
    pub model: String,
    /// RFC 3339 creation instant.
    #[serde(default)]
    pub created: String,
    /// Tombstone: the latest row for `norm` removes the entry.
    #[serde(default)]
    pub deleted: bool,
}

/// Normalize a term to its identity: trimmed, whitespace-collapsed, case-folded.
#[must_use]
pub fn normalize(term: &str) -> String {
    term.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Directory holding the dynamic glossary for a state directory.
#[must_use]
pub fn glossary_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(GLOSSARY_SUBDIR)
}

/// Live dynamic-glossary path for a state directory.
#[must_use]
pub fn glossary_path(state_dir: &Path) -> PathBuf {
    glossary_dir(state_dir).join(GLOSSARY_FILE)
}

/// State directory from the store path: the store file's parent.
#[must_use]
pub fn state_dir_of(storage_path: Option<&Path>) -> Option<PathBuf> {
    storage_path.and_then(|p| p.parent()).map(Path::to_path_buf)
}

/// Parse every row in `text`, skipping lines that do not parse.
#[must_use]
pub fn parse_rows(text: &str) -> Vec<DynamicEntry> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<DynamicEntry>(line).ok())
        .collect()
}

/// The live entries in `rows`: the latest row per `norm` wins, tombstones
/// remove, order is first-learned order.
#[must_use]
pub fn live_entries(rows: &[DynamicEntry]) -> Vec<DynamicEntry> {
    let mut order: Vec<String> = Vec::new();
    let mut latest: std::collections::HashMap<String, DynamicEntry> =
        std::collections::HashMap::new();
    for row in rows {
        if !latest.contains_key(&row.norm) {
            order.push(row.norm.clone());
        }
        latest.insert(row.norm.clone(), row.clone());
    }
    order
        .iter()
        .filter_map(|norm| latest.get(norm))
        .filter(|row| !row.deleted && !row.definition.trim().is_empty())
        .cloned()
        .collect()
}

/// Load the live dynamic entries for `state_dir` (empty when no file exists).
#[must_use]
pub fn load_entries(state_dir: &Path) -> Vec<DynamicEntry> {
    let text = std::fs::read_to_string(glossary_path(state_dir)).unwrap_or_default();
    live_entries(&parse_rows(&text))
}

/// Find the live entry for `term` (normalized), if any.
#[must_use]
pub fn find<'a>(entries: &'a [DynamicEntry], term: &str) -> Option<&'a DynamicEntry> {
    let norm = normalize(term);
    entries.iter().find(|entry| entry.norm == norm)
}

/// Probe `selection` and its word-prefixes, longest first, against `entries`:
/// the logical term may be a prefix of what was selected.
#[must_use]
pub fn find_for_selection<'a>(
    entries: &'a [DynamicEntry],
    selection: &str,
) -> Option<&'a DynamicEntry> {
    let words: Vec<&str> = selection.split_whitespace().collect();
    for take in (1..=words.len()).rev() {
        for start in 0..=(words.len() - take) {
            let candidate = words[start..start + take].join(" ");
            if let Some(entry) = find(entries, &candidate) {
                return Some(entry);
            }
        }
    }
    None
}

/// Serialize one row as a single JSON line (no newline).
#[must_use]
pub fn row_line(row: &DynamicEntry) -> String {
    serde_json::to_string(row).unwrap_or_else(|_| "{}".to_owned())
}

/// Append one row, rotating to the backup when the cap would be exceeded.
///
/// Best-effort by contract, like the unknown-terms log: callers warn and
/// continue on error; a lookup never fails because the glossary write did.
///
/// # Errors
///
/// [`std::io::Error`] when the directory or file cannot be written.
pub fn append_row(path: &Path, row: &DynamicEntry, max_bytes: u64) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = row_line(row);
    line.push('\n');
    let line_len = line.len() as u64;
    if max_bytes > 0 {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() + line_len > max_bytes && meta.len() > 0 {
                let backup = path.with_file_name(GLOSSARY_BACKUP);
                let _ = std::fs::remove_file(&backup);
                std::fs::rename(path, &backup)?;
            }
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(term: &str, definition: &str) -> DynamicEntry {
        DynamicEntry {
            v: 1,
            term: term.to_owned(),
            norm: normalize(term),
            definition: definition.to_owned(),
            source: None,
            model: "test".to_owned(),
            created: "2026-11-23T00:00:00Z".to_owned(),
            deleted: false,
        }
    }

    #[test]
    fn latest_row_wins_and_tombstone_removes() {
        let rows = vec![
            entry("Family A", "first gloss."),
            entry("family  a", "second gloss."),
            DynamicEntry {
                deleted: true,
                definition: String::new(),
                ..entry("Family A", "")
            },
        ];
        assert!(live_entries(&rows).is_empty());
        let rows = vec![
            entry("Family A", "first gloss."),
            entry("FAMILY A", "second gloss."),
        ];
        let live = live_entries(&rows);
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].definition, "second gloss.");
    }

    #[test]
    fn selection_probe_prefers_the_longest_known_prefix() {
        let entries = vec![
            entry("Family A", "the family."),
            entry("Family", "the word."),
        ];
        let found =
            find_for_selection(&entries, "the Family A test fix landed").expect("prefix hit");
        assert_eq!(found.term, "Family A");
        assert!(find_for_selection(&entries, "something else entirely").is_none());
    }

    #[test]
    fn rows_round_trip_through_jsonl() {
        let row = entry("Family A test fix", "the fix for the family.");
        let text = format!("{}\nnot json\n", row_line(&row));
        assert_eq!(parse_rows(&text), vec![row]);
    }
}
