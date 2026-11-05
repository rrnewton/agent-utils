//! The known-flaky list: tests whose flakiness is already known and tracked, so a pass after a
//! retry inside a run is not news. A GREEN_FLAKY result whose retried tests are all on the list
//! lands without a hold, the tests recorded as flaky with the landing; one with any test not on
//! the list is held for a ruling as before.
//!
//! ```text
//! <state>/known-flaky.json   {"schema": "merge-queue-known-flaky/v1", "entries": [...]}
//! ```
//!
//! Each entry names a test and the issue that tracks its flakiness, with who added it and when.
//! An entry covers its test, the test's cells (`name[cell]`, as a validator that runs one test in
//! several configurations names them) and its cases (`step::case`). `merge-queue known-flaky`
//! adds, removes and lists entries; the file is replaced atomically, so a run reads either the
//! old list or the new one.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::now;
use crate::state::write_json;
use crate::tests::{TestId, CASE_SEPARATOR};

/// [`KnownFlaky::schema`].
pub const KNOWN_FLAKY_SCHEMA: &str = "merge-queue-known-flaky/v1";

/// The list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownFlaky {
    /// [`KNOWN_FLAKY_SCHEMA`].
    pub schema: String,
    /// The entries, in the order they were added.
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Default for KnownFlaky {
    fn default() -> Self {
        Self {
            schema: KNOWN_FLAKY_SCHEMA.to_owned(),
            entries: Vec::new(),
        }
    }
}

/// One known-flaky test.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The test (a test ID; see [`covers`] for what it covers).
    pub test: String,
    /// The issue tracking its flakiness: a link or a tracker ID.
    pub issue: String,
    /// Who added it.
    pub by: String,
    /// Why, when given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// RFC 3339 UTC time it was added.
    pub added_at: String,
}

/// Path of the list in the state directory `state_root`.
pub fn path(state_root: &Path) -> PathBuf {
    state_root.join("known-flaky.json")
}

/// Whether the entry for `entry` covers the test `id`: the test itself, one of its cells
/// (`entry[cell]`) or one of its cases (`entry::case`).
pub fn covers(entry: &str, id: &str) -> bool {
    match id.strip_prefix(entry) {
        Some("") => true,
        Some(rest) => rest.starts_with('[') || rest.starts_with(CASE_SEPARATOR),
        None => false,
    }
}

impl KnownFlaky {
    /// The entry covering `test`, if any.
    pub fn covering(&self, test: &TestId) -> Option<&Entry> {
        let id = test.to_string();
        self.entries.iter().find(|entry| covers(&entry.test, &id))
    }

    /// For each of `tests`, the entry covering it, when every one of them is covered; `None`
    /// when `tests` is empty or any test is not on the list.
    pub fn cover_all<'a>(&'a self, tests: &'a [TestId]) -> Option<Vec<(&'a TestId, &'a Entry)>> {
        if tests.is_empty() {
            return None;
        }
        tests
            .iter()
            .map(|test| self.covering(test).map(|entry| (test, entry)))
            .collect()
    }
}

/// Read the list; a missing file is an empty list.
pub fn load(state_root: &Path) -> Result<KnownFlaky, String> {
    let path = path(state_root);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(KnownFlaky::default());
        }
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let list: KnownFlaky = serde_json::from_str(&text)
        .map_err(|error| format!("unreadable {}: {error}", path.display()))?;
    if list.schema != KNOWN_FLAKY_SCHEMA {
        return Err(format!(
            "{} has schema {:?}, not {KNOWN_FLAKY_SCHEMA:?}",
            path.display(),
            list.schema
        ));
    }
    Ok(list)
}

/// Add `test` to the list, tracked by `issue`, or replace its entry. Returns the entry.
pub fn add(
    state_root: &Path,
    test: &str,
    issue: &str,
    by: &str,
    reason: &str,
) -> Result<Entry, String> {
    let test = TestId::parse(test)?.to_string();
    if issue.trim().is_empty() || by.trim().is_empty() {
        return Err(
            "a known-flaky entry names the issue tracking it (--issue) and who adds it (--by)"
                .to_owned(),
        );
    }
    let mut list = load(state_root)?;
    let entry = Entry {
        test: test.clone(),
        issue: issue.trim().to_owned(),
        by: by.trim().to_owned(),
        reason: reason.trim().to_owned(),
        added_at: now(),
    };
    match list.entries.iter_mut().find(|old| old.test == test) {
        Some(old) => *old = entry.clone(),
        None => list.entries.push(entry.clone()),
    }
    write_json(&path(state_root), &list)?;
    Ok(entry)
}

/// Remove the entry for exactly `test`. Returns it.
pub fn remove(state_root: &Path, test: &str) -> Result<Entry, String> {
    let test = TestId::parse(test)?.to_string();
    let mut list = load(state_root)?;
    let index = list
        .entries
        .iter()
        .position(|entry| entry.test == test)
        .ok_or_else(|| format!("{test} is not on the known-flaky list"))?;
    let entry = list.entries.remove(index);
    write_json(&path(state_root), &list)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_covers_its_test_its_cells_and_its_cases_only() {
        let entry = "suite/reconnect";
        assert!(covers(entry, "suite/reconnect"));
        assert!(covers(entry, "suite/reconnect[fast/debug]"));
        assert!(covers(entry, "suite/reconnect::case"));
        assert!(!covers(entry, "suite/reconnect-2"));
        assert!(!covers(entry, "suite/recon"));
        assert!(!covers(entry, "other/suite/reconnect"));
        // An entry for one cell covers that cell only.
        let cell = "suite/reconnect[fast/debug]";
        assert!(covers(cell, cell));
        assert!(!covers(cell, "suite/reconnect[slow/debug]"));
        assert!(!covers(cell, "suite/reconnect"));
    }

    #[test]
    fn every_test_must_be_covered() {
        let list = KnownFlaky {
            schema: KNOWN_FLAKY_SCHEMA.to_owned(),
            entries: vec![Entry {
                test: "a".to_owned(),
                issue: "https://example.invalid/issues/1".to_owned(),
                by: "owner".to_owned(),
                reason: String::new(),
                added_at: now(),
            }],
        };
        let a = TestId::step("a[cell]");
        let b = TestId::step("b");
        assert_eq!(
            list.cover_all(std::slice::from_ref(&a)).map(|c| c.len()),
            Some(1)
        );
        assert!(list.cover_all(&[a, b]).is_none());
        assert!(list.cover_all(&[]).is_none());
    }
}
