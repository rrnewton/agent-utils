//! The source revision this binary was built from (see `build.rs`), reported by `--version`
//! and compared between the queue host and its donors.

use serde::{Deserialize, Serialize};

/// Git commit the binary was built from, when the build recorded one.
const COMMIT: Option<&str> = option_env!("MERGE_QUEUE_BUILD_COMMIT");

/// `"1"` when that checkout had uncommitted changes to the build's inputs.
const DIRTY: Option<&str> = option_env!("MERGE_QUEUE_BUILD_DIRTY");

/// A merge-queue build's source revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Revision {
    /// Full Git commit.
    pub commit: String,
    /// The build's inputs had uncommitted changes, so the commit does not describe it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
}

impl Revision {
    /// `commit` or `commit (dirty)`.
    pub fn describe(&self) -> String {
        if self.dirty {
            format!("{} (dirty)", self.commit)
        } else {
            self.commit.clone()
        }
    }
}

/// This binary's revision, when the build recorded one.
pub fn revision() -> Option<Revision> {
    COMMIT.map(|commit| Revision {
        commit: commit.to_owned(),
        dirty: DIRTY == Some("1"),
    })
}

/// `1` accepts a build with uncommitted changes as a donor tool (development and tests): two
/// dirty builds of one commit then match even if their changes differ.
pub const ALLOW_DIRTY_ENV: &str = "MERGE_QUEUE_ALLOW_DIRTY";

fn allow_dirty() -> bool {
    std::env::var(ALLOW_DIRTY_ENV).as_deref() == Ok("1")
}

/// Why `revision` cannot identify a donor tool, or `None` when it can: it must exist and be
/// clean (a dirty build's commit does not say what it runs), unless [`ALLOW_DIRTY_ENV`] is 1.
pub fn unusable(revision: Option<&Revision>) -> Option<String> {
    match revision {
        None => Some(
            "this merge-queue binary records no source revision (built outside a Git checkout \
             without MERGE_QUEUE_SOURCE_COMMIT); rebuild it from a checkout"
                .to_owned(),
        ),
        Some(revision) if revision.dirty && !allow_dirty() => Some(format!(
            "this merge-queue binary was built from {} with uncommitted changes; rebuild it \
             from a clean checkout (or set {ALLOW_DIRTY_ENV}=1 on every host, for development)",
            revision.commit
        )),
        Some(_) => None,
    }
}

/// The text clap prints after the program name for `--version`.
pub fn version_text() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| version_lines(env!("CARGO_PKG_VERSION"), revision().as_ref()))
}

fn version_lines(version: &str, revision: Option<&Revision>) -> String {
    match revision {
        Some(revision) => format!("{version}\ncommit {}", revision.describe()),
        None => format!("{version}\ncommit unknown (no source revision recorded)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names_the_revision_or_its_absence() {
        let clean = Revision {
            commit: "a".repeat(40),
            dirty: false,
        };
        let dirty = Revision {
            dirty: true,
            ..clean.clone()
        };
        assert_eq!(
            version_lines("0.1.0", Some(&clean)),
            format!("0.1.0\ncommit {}", "a".repeat(40))
        );
        assert_eq!(
            version_lines("0.1.0", Some(&dirty)),
            format!("0.1.0\ncommit {} (dirty)", "a".repeat(40))
        );
        assert!(version_lines("0.1.0", None).contains("commit unknown"));
        assert!(unusable(Some(&clean)).is_none());
        if !allow_dirty() {
            assert!(unusable(Some(&dirty)).unwrap().contains("uncommitted"));
        }
        assert!(unusable(None).unwrap().contains("no source revision"));
    }
}
