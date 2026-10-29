//! Build provenance reported by `agentctl --version` and as the `build` object of
//! `agentctl capabilities`, and the startup check that compares it with a team's pinned checkout.
//!
//! The source fingerprint, commit, and dirty flag are the values `AGENTCTL_SOURCE_FP`,
//! `AGENTCTL_SOURCE_COMMIT`, and `AGENTCTL_SOURCE_DIRTY` held when the binary was built (see
//! `build.rs`); each is null for a build that named none. The first line of `--version` never
//! changes; a build that names its commit adds `commit ...` (and `source_fp ...`) lines after it.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Implementation edition of this binary.
pub(crate) const EDITION: &str = "rust";

/// Source fingerprint the binary was built from, when the build named one.
pub(crate) const SOURCE_FINGERPRINT: Option<&str> = option_env!("AGENTCTL_BUILD_SOURCE_FP");

/// Git commit the binary was built from, when the build named one.
pub(crate) const SOURCE_COMMIT: Option<&str> = option_env!("AGENTCTL_BUILD_SOURCE_COMMIT");

/// `"1"` when the build's checkout had uncommitted tracked changes, `"0"` when clean.
pub(crate) const SOURCE_DIRTY: Option<&str> = option_env!("AGENTCTL_BUILD_SOURCE_DIRTY");

/// Names the source checkout whose HEAD the caller's team pins (set in the team env file).
pub(crate) const CHECKOUT_ENV: &str = "AGENT_UTILS_CHECKOUT";

/// `1` lets a binary built from another commit run commands that write agents' state.
pub(crate) const ALLOW_STALE_ENV: &str = "AGENTCTL_ALLOW_STALE";

/// The `build` object of the capabilities document.
pub(crate) fn document() -> Value {
    json!({
        "edition": EDITION,
        "source_fp": SOURCE_FINGERPRINT,
        "commit": SOURCE_COMMIT,
        "dirty": SOURCE_DIRTY.map(|value| value == "1"),
    })
}

/// The text clap prints after the program name for `--version`.
pub(crate) fn version_text() -> &'static str {
    static TEXT: OnceLock<String> = OnceLock::new();
    TEXT.get_or_init(|| {
        version_lines(
            env!("CARGO_PKG_VERSION"),
            SOURCE_COMMIT,
            SOURCE_DIRTY,
            SOURCE_FINGERPRINT,
        )
    })
}

fn version_lines(
    version: &str,
    commit: Option<&str>,
    dirty: Option<&str>,
    fingerprint: Option<&str>,
) -> String {
    let mut text = version.to_owned();
    if let Some(commit) = commit {
        text.push_str("\ncommit ");
        text.push_str(commit);
        if dirty == Some("1") {
            text.push_str(" (dirty)");
        }
        if let Some(fingerprint) = fingerprint {
            text.push_str("\nsource_fp ");
            text.push_str(fingerprint);
        }
    }
    text
}

/// A binary whose embedded commit differs from the team checkout's HEAD.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Stale {
    pub(crate) built: String,
    pub(crate) pinned: String,
    pub(crate) checkout: PathBuf,
}

impl Stale {
    pub(crate) fn warning(&self) -> String {
        format!(
            "agentctl: warning: this binary was built from source commit {} but {CHECKOUT_ENV} ({}) is at {}; rebuild the team's tools from that checkout",
            short(&self.built),
            self.checkout.display(),
            short(&self.pinned)
        )
    }

    pub(crate) fn refusal(&self) -> String {
        format!(
            "refusing to change agents' state with a binary built from source commit {} while {CHECKOUT_ENV} ({}) is at {}; rebuild, or set {ALLOW_STALE_ENV}=1 to proceed once",
            short(&self.built),
            self.checkout.display(),
            short(&self.pinned)
        )
    }
}

fn short(commit: &str) -> &str {
    commit.get(..12).unwrap_or(commit)
}

/// Compare the embedded commit with the team checkout named by [`CHECKOUT_ENV`].
///
/// `Ok(None)` when either is absent or they match; an unreadable checkout is an `Err` the
/// caller reports as a warning. Reads files only: no Git or build process runs.
pub(crate) fn staleness(
    built: Option<&str>,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Result<Option<Stale>, String> {
    let Some(built) = built else {
        return Ok(None);
    };
    let Some(checkout) = environment(CHECKOUT_ENV).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let checkout = PathBuf::from(checkout);
    let pinned = checkout_head(&checkout)?;
    Ok((pinned != built).then(|| Stale {
        built: built.to_owned(),
        pinned,
        checkout,
    }))
}

fn read_line(path: &Path) -> Result<String, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(text.lines().next().unwrap_or("").trim().to_owned())
}

fn commit_hex(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The commit a Git checkout's HEAD names, from its files alone: `.git` (a directory, or a
/// `gitdir:` file for a submodule or linked worktree), `HEAD`, then a loose ref in the worktree
/// or common directory, then `packed-refs`.
pub(crate) fn checkout_head(checkout: &Path) -> Result<String, String> {
    let dot_git = checkout.join(".git");
    let git_dir = if dot_git.is_file() {
        let line = read_line(&dot_git)?;
        let target = line
            .strip_prefix("gitdir:")
            .map(str::trim)
            .ok_or_else(|| format!("{} does not name a gitdir", dot_git.display()))?;
        let target = PathBuf::from(target);
        if target.is_absolute() {
            target
        } else {
            checkout.join(target)
        }
    } else {
        dot_git
    };
    let head = read_line(&git_dir.join("HEAD"))?;
    let Some(reference) = head.strip_prefix("ref:").map(str::trim) else {
        return commit_hex(&head)
            .then_some(head.clone())
            .ok_or_else(|| format!("{} holds no commit", git_dir.join("HEAD").display()));
    };
    let common = match read_line(&git_dir.join("commondir")) {
        Ok(relative) if !relative.is_empty() => {
            let relative = PathBuf::from(relative);
            if relative.is_absolute() {
                relative
            } else {
                git_dir.join(relative)
            }
        }
        _ => git_dir.clone(),
    };
    for directory in [&git_dir, &common] {
        if let Ok(value) = read_line(&directory.join(reference)) {
            if commit_hex(&value) {
                return Ok(value);
            }
        }
    }
    let packed = fs::read_to_string(common.join("packed-refs")).unwrap_or_default();
    packed
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('^'))
        .find_map(|line| {
            let (commit, name) = line.split_once(' ')?;
            (name == reference && commit_hex(commit)).then(|| commit.to_owned())
        })
        .ok_or_else(|| format!("cannot resolve {reference} in {}", common.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_document_names_the_edition_and_checked_provenance() {
        let document = document();
        assert_eq!(document["edition"], "rust");
        let object = document.as_object().unwrap();
        assert_eq!(object.len(), 4);
        match &document["source_fp"] {
            Value::Null => assert!(SOURCE_FINGERPRINT.is_none()),
            Value::String(fingerprint) => {
                assert_eq!(fingerprint.len(), 64);
                assert!(fingerprint
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')));
            }
            other => panic!("unexpected source_fp {other:?}"),
        }
        match &document["commit"] {
            Value::Null => assert!(document["dirty"].is_null()),
            Value::String(commit) => assert!(commit_hex(commit)),
            other => panic!("unexpected commit {other:?}"),
        }
    }

    #[test]
    fn version_lines_add_provenance_only_for_a_named_commit() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let fingerprint = "a".repeat(64);
        assert_eq!(
            version_lines("1.2.3", None, None, Some(&fingerprint)),
            "1.2.3"
        );
        assert_eq!(
            version_lines("1.2.3", Some(commit), Some("0"), None),
            format!("1.2.3\ncommit {commit}")
        );
        assert_eq!(
            version_lines("1.2.3", Some(commit), Some("1"), Some(&fingerprint)),
            format!("1.2.3\ncommit {commit} (dirty)\nsource_fp {fingerprint}")
        );
    }

    fn scratch(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "agentctl-build-info-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn checkout_head_reads_detached_loose_packed_and_gitdir_layouts() {
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let root = scratch("heads");
        // A detached submodule-style checkout: `.git` names a separate gitdir.
        fs::create_dir_all(root.join("modules/sub")).unwrap();
        fs::write(root.join("modules/sub/HEAD"), format!("{a}\n")).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("sub/.git"), "gitdir: ../modules/sub\n").unwrap();
        assert_eq!(checkout_head(&root.join("sub")).unwrap(), a);
        // A branch checkout with a loose ref.
        fs::create_dir_all(root.join("main/.git/refs/heads")).unwrap();
        fs::write(root.join("main/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        fs::write(root.join("main/.git/refs/heads/main"), format!("{b}\n")).unwrap();
        assert_eq!(checkout_head(&root.join("main")).unwrap(), b);
        // A linked worktree whose branch lives only in the common directory's packed-refs.
        fs::create_dir_all(root.join("main/.git/worktrees/w")).unwrap();
        fs::write(
            root.join("main/.git/worktrees/w/HEAD"),
            "ref: refs/heads/topic\n",
        )
        .unwrap();
        fs::write(root.join("main/.git/worktrees/w/commondir"), "../..\n").unwrap();
        fs::write(
            root.join("main/.git/packed-refs"),
            format!("# pack-refs with: peeled\n{a} refs/heads/topic\n^{b}\n"),
        )
        .unwrap();
        fs::create_dir_all(root.join("w")).unwrap();
        fs::write(
            root.join("w/.git"),
            format!("gitdir: {}\n", root.join("main/.git/worktrees/w").display()),
        )
        .unwrap();
        assert_eq!(checkout_head(&root.join("w")).unwrap(), a);
        assert!(checkout_head(&root.join("absent")).is_err());
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn staleness_compares_only_when_both_sides_are_known() {
        let a = "1111111111111111111111111111111111111111";
        let b = "2222222222222222222222222222222222222222";
        let root = scratch("stale");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), format!("{a}\n")).unwrap();
        let named = root.display().to_string();
        let environment = move |name: &str| (name == CHECKOUT_ENV).then(|| named.clone());
        assert_eq!(staleness(None, &environment).unwrap(), None);
        assert_eq!(staleness(Some(a), &|_| None).unwrap(), None);
        assert_eq!(staleness(Some(a), &environment).unwrap(), None);
        let stale = staleness(Some(b), &environment).unwrap().unwrap();
        assert_eq!(stale.pinned, a);
        assert!(stale.warning().contains("222222222222"));
        assert!(stale.refusal().contains(ALLOW_STALE_ENV));
        fs::remove_dir_all(&root).unwrap();
    }
}
