//! Private copies of the files a command sends, so that what the content guard and the
//! classifier inspect is exactly what gh sends.
//!
//! Without a copy, a body file could change between the moment the guard reads it and the moment
//! gh reads it, which can be many minutes later after a pacing sleep: a small, clean file could
//! be swapped for an archive. The classifier also reads GraphQL documents from files to tell a
//! query from a mutation. So before anything inspects a file, the wrapper copies it to
//! `<state dir>/snap-<nonce>/<n>/<original file name>` (directories mode 0700, files 0600),
//! rewrites the argument to point at the copy, and runs everything after that on the rewritten
//! arguments. The original file name is kept because gh uses it (a gist's file names come from
//! it). The copies are removed when the invocation ends; a directory left behind by a killed
//! wrapper is removed by a later invocation once it is a day old.

use crate::guard::BodySource;
use crate::guard::SourceKind;
use std::fs::DirBuilder;
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// Largest file copied, bytes. A larger file is refused; GitHub text bodies are limited to a few
/// kilobytes by the content guard anyway.
pub const MAX_SNAPSHOT_BYTES: u64 = 64 << 20;

/// Age after which an abandoned snapshot directory is removed, seconds.
pub const SWEEP_AGE_SECS: u64 = 24 * 3600;

/// The copies of one invocation. Dropping it removes them.
#[derive(Debug, Default)]
pub struct Snapshot {
    dir: Option<PathBuf>,
    /// Number of files copied.
    pub files: usize,
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        if let Some(d) = self.dir.take() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

fn private_dir(path: &Path) -> Result<(), String> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("cannot create snapshot directory {}: {e}", path.display()))
}

/// Copy every file in `sources` (found by `guard::file_sources` on `rest`) and return the
/// argument list with each path replaced by its copy. An error names the file and why it could
/// not be copied: missing, unreadable, larger than [`MAX_SNAPSHOT_BYTES`], or not found where
/// the parser said it was in `rest`.
pub fn take(
    state_dir: &Path,
    nonce: &str,
    sources: &[BodySource],
    rest: &[String],
) -> Result<(Snapshot, Vec<String>), String> {
    let mut snap = Snapshot::default();
    let mut out = rest.to_vec();
    for (i, s) in sources.iter().enumerate() {
        let SourceKind::File(path) = &s.kind else {
            continue;
        };
        let Some(loc) = &s.location else {
            return Err(format!(
                "{} names file {path}, but its place in the command line is unknown, so it \
                 cannot be copied for inspection",
                s.flag
            ));
        };
        let expected = format!("{}{path}", loc.prefix);
        if rest.get(loc.index) != Some(&expected) {
            return Err(format!(
                "{} names file {path}, but argument {} is not `{expected}`",
                s.flag, loc.index
            ));
        }
        let dir = match &snap.dir {
            Some(d) => d.clone(),
            None => {
                let d = state_dir.join(format!("snap-{nonce}"));
                private_dir(&d)?;
                snap.dir = Some(d.clone());
                d
            }
        };
        let sub = dir.join(i.to_string());
        private_dir(&sub)?;
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| "body".into());
        let copy = sub.join(name);
        copy_capped(Path::new(path), &copy).map_err(|e| format!("{} file {path}: {e}", s.flag))?;
        snap.files += 1;
        out[loc.index] = format!("{}{}", loc.prefix, copy.display());
    }
    Ok((snap, out))
}

fn copy_capped(src: &Path, dst: &Path) -> Result<(), String> {
    let file = std::fs::File::open(src).map_err(|e| format!("cannot read: {e}"))?;
    let mut buf = Vec::new();
    file.take(MAX_SNAPSHOT_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("cannot read: {e}"))?;
    if buf.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(format!(
            "larger than {MAX_SNAPSHOT_BYTES} bytes, too large to copy for inspection"
        ));
    }
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)
        .map_err(|e| format!("cannot write copy {}: {e}", dst.display()))?;
    out.write_all(&buf)
        .map_err(|e| format!("cannot write copy {}: {e}", dst.display()))
}

/// Remove `snap-*` directories in `state_dir` last modified more than [`SWEEP_AGE_SECS`] ago
/// (left behind by a wrapper that was killed). Returns how many were removed.
pub fn sweep(state_dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return 0;
    };
    let now = std::time::SystemTime::now();
    let mut removed = 0;
    for e in entries.flatten() {
        let name = e.file_name();
        if !name.to_string_lossy().starts_with("snap-") {
            continue;
        }
        let old = e
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_dir)
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age.as_secs() > SWEEP_AGE_SECS);
        if old && std::fs::remove_dir_all(e.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classify::classify;
    use crate::config::Config;
    use crate::guard::file_sources;
    use std::os::unix::fs::PermissionsExt;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "gh-paced-snap-{tag}-{}-{}",
            std::process::id(),
            crate::state::new_nonce(0.0)
        ));
        std::fs::create_dir_all(&d).expect("mkdir");
        d
    }

    fn snap_of(dir: &Path, line: &[String]) -> Result<(Snapshot, Vec<String>), String> {
        let c = classify(line, &Config::default());
        let rest = &line[c.rest_start..];
        take(dir, "00000000000000aa", &file_sources(&c, rest), rest)
    }

    /// The copy keeps the bytes and file name, is private, is what the rewritten argument names,
    /// and does not follow later changes to the original.
    #[test]
    fn copies_replace_the_paths_and_vanish_on_drop() {
        let dir = tmpdir("copy");
        let body = dir.join("body.md");
        std::fs::write(&body, b"short note").expect("write");
        let g = dir.join("notes.txt");
        std::fs::write(&g, b"gist text").expect("write");
        let line: Vec<String> = [
            "issue",
            "comment",
            "1",
            &format!("--body-file={}", body.display()),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (snap, rest) = snap_of(&dir, &line).expect("snapshot");
        assert_eq!(snap.files, 1);
        let copy = rest[1].strip_prefix("--body-file=").expect("prefix kept");
        assert_ne!(copy, body.display().to_string());
        assert!(copy.ends_with("/0/body.md"), "{copy}");
        std::fs::write(&body, b"CHANGED AFTER THE GUARD").expect("rewrite");
        assert_eq!(std::fs::read(copy).expect("read copy"), b"short note");
        let mode = std::fs::metadata(copy).expect("meta").permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let snapdir = dir.join("snap-00000000000000aa");
        let dmode = std::fs::metadata(&snapdir)
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(dmode & 0o777, 0o700);
        drop(snap);
        assert!(!snapdir.exists());

        let line: Vec<String> = ["gist", "create", &g.display().to_string()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (snap, rest) = snap_of(&dir, &line).expect("snapshot");
        assert!(rest[0].ends_with("/0/notes.txt"), "{rest:?}");
        assert_eq!(snap.files, 1);
        drop(snap);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_and_oversized_files_are_errors() {
        let dir = tmpdir("err");
        let line: Vec<String> = ["issue", "comment", "1", "-F", "/nonexistent/gh-paced-body"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = snap_of(&dir, &line).expect_err("missing file");
        assert!(err.contains("cannot read"), "{err}");
        let big = dir.join("big");
        let f = std::fs::File::create(&big).expect("create");
        f.set_len(MAX_SNAPSHOT_BYTES + 1).expect("set_len");
        let line: Vec<String> = ["issue", "comment", "1", "-F", &big.display().to_string()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let err = snap_of(&dir, &line).expect_err("oversized file");
        assert!(err.contains("too large"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_files_means_no_directory() {
        let dir = tmpdir("none");
        let line: Vec<String> = ["issue", "comment", "1", "-b", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (snap, rest) = snap_of(&dir, &line).expect("snapshot");
        assert_eq!(snap.files, 0);
        assert_eq!(rest, line[2..].to_vec());
        assert!(!dir.join("snap-00000000000000aa").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_removes_only_old_snapshot_directories() {
        let dir = tmpdir("sweep");
        let old = dir.join("snap-old");
        let fresh = dir.join("snap-fresh");
        let other = dir.join("keep-me");
        for d in [&old, &fresh, &other] {
            std::fs::create_dir_all(d).expect("mkdir");
        }
        let two_days_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 86400);
        std::fs::File::open(&old)
            .expect("open")
            .set_modified(two_days_ago)
            .expect("set mtime");
        std::fs::File::open(&other)
            .expect("open")
            .set_modified(two_days_ago)
            .expect("set mtime");
        assert_eq!(sweep(&dir), 1);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert!(other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
