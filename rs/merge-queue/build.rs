//! Records the source revision for `merge-queue --version` and the donor tool identity.
//!
//! Builds of one revision are not byte-identical across hosts, so donors and the queue host
//! compare the revision a binary was built from, never a hash of the binary. It comes from,
//! in order:
//!
//! - `MERGE_QUEUE_SOURCE_COMMIT` (40-character lowercase hexadecimal Git commit) and
//!   `MERGE_QUEUE_SOURCE_DIRTY` (`0` or `1`, which requires the commit), for builds outside a
//!   checkout such as one from an archive. A malformed value fails the build rather than
//!   publishing a revision nobody can match.
//! - Otherwise, when this crate's manifest is tracked by the Git checkout it is built in, that
//!   checkout's `HEAD`, dirty when the build's own inputs (this crate, its path dependency
//!   and the workspace manifest and lock file) have uncommitted or untracked changes.
//!
//! When neither is available (no Git, a packaged or vendored copy), the binary records no
//! revision; it says so in `--version` and is refused as a donor tool.

use std::env::{self, VarError};
use std::path::Path;
use std::process::Command;

const SOURCE_COMMIT: &str = "MERGE_QUEUE_SOURCE_COMMIT";
const SOURCE_DIRTY: &str = "MERGE_QUEUE_SOURCE_DIRTY";

/// Paths, relative to this crate's directory, whose changes make a build dirty.
const INPUTS: [&str; 4] = [
    ".",
    "../pr-landing-planner",
    "../Cargo.toml",
    "../Cargo.lock",
];

fn commit_hex(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The variable's nonempty value; `None` when it is unset or empty.
fn read(name: &str) -> Option<String> {
    println!("cargo:rerun-if-env-changed={name}");
    match env::var(name) {
        Ok(value) if value.is_empty() => None,
        Ok(value) => Some(value),
        Err(VarError::NotPresent) => None,
        Err(VarError::NotUnicode(value)) => panic!("{name} is not valid Unicode: {value:?}"),
    }
}

/// Trimmed standard output of a successful `git` command run in `dir`.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `HEAD` and the dirty flag of the checkout tracking this crate, with the files whose
/// change must run this script again.
fn from_git(dir: &Path) -> Option<(String, bool)> {
    git(dir, &["ls-files", "--error-unmatch", "Cargo.toml"])?;
    let commit = git(dir, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    if !commit_hex(&commit) {
        return None;
    }
    let mut status = vec!["status", "--porcelain", "--untracked-files=all", "--"];
    status.extend(INPUTS);
    let dirty = !git(dir, &status)?.is_empty();
    // A commit moves HEAD or the branch it names; an edit touches an input. A branch whose
    // ref is packed has no file of its own until the next commit writes one, so its
    // directory is watched; a missing path would rerun this script on every build.
    let mut watched = vec![dir.join(git(dir, &["rev-parse", "--git-path", "HEAD"])?)];
    if let Some(branch) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        let path = dir.join(git(dir, &["rev-parse", "--git-path", &branch])?);
        watched.extend(path.parent().map(Path::to_path_buf));
    }
    watched.push(dir.join(git(dir, &["rev-parse", "--git-path", "packed-refs"])?));
    for path in watched.iter().filter(|path| path.exists()) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for input in INPUTS {
        println!("cargo:rerun-if-changed={}", dir.join(input).display());
    }
    Some((commit, dirty))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let commit = read(SOURCE_COMMIT);
    let dirty = read(SOURCE_DIRTY);
    if let Some(value) = &dirty {
        assert!(
            value == "0" || value == "1",
            "{SOURCE_DIRTY} must be 0 or 1, got {value:?}"
        );
        assert!(
            commit.is_some(),
            "{SOURCE_DIRTY} describes {SOURCE_COMMIT}; set both or neither"
        );
    }
    let revision = match commit {
        Some(commit) => {
            assert!(
                commit_hex(&commit),
                "{SOURCE_COMMIT} must be a 40-character lowercase hexadecimal Git commit, got {commit:?}"
            );
            Some((commit, dirty.as_deref() == Some("1")))
        }
        None => {
            let dir = env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
            from_git(Path::new(&dir))
        }
    };
    if let Some((commit, dirty)) = revision {
        println!("cargo:rustc-env=MERGE_QUEUE_BUILD_COMMIT={commit}");
        println!(
            "cargo:rustc-env=MERGE_QUEUE_BUILD_DIRTY={}",
            if dirty { "1" } else { "0" }
        );
    }
}
