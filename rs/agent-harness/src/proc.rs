//! The few process facts locating a transcript needs, read from a `/proc` tree whose root is
//! given, so tests can build one.

use std::fs;
use std::path::{Path, PathBuf};

/// `pid` and every process descended from it that `root` shows, `pid` first.
pub(crate) fn tree(root: &Path, pid: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| {
                    let child: u32 = entry.file_name().to_str()?.parse().ok()?;
                    Some((child, parent(root, child)?))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut pids = vec![pid];
    let mut index = 0;
    while index < pids.len() {
        let current = pids[index];
        for (child, of) in &parents {
            if *of == current && *child != current && !pids.contains(child) {
                pids.push(*child);
            }
        }
        index += 1;
    }
    pids
}

/// The fields of `/proc/<pid>/stat` after the command name, which may itself hold spaces and
/// parentheses.
fn stat_fields(root: &Path, pid: u32) -> Option<Vec<String>> {
    let text = fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
    let after = &text[text.rfind(')')? + 1..];
    Some(after.split_whitespace().map(str::to_owned).collect())
}

/// The parent of `pid`.
fn parent(root: &Path, pid: u32) -> Option<u32> {
    stat_fields(root, pid)?.get(1)?.parse().ok()
}

/// When `pid` started, in clock ticks after boot (the 22nd field of its `stat`).
pub(crate) fn start_ticks(root: &Path, pid: u32) -> Option<u64> {
    stat_fields(root, pid)?.get(19)?.parse().ok()
}

/// The value of `key` in `pid`'s environment, when readable and not empty.
pub(crate) fn environment(root: &Path, pid: u32, key: &str) -> Option<String> {
    let bytes = fs::read(root.join(pid.to_string()).join("environ")).ok()?;
    bytes.split(|byte| *byte == 0).find_map(|entry| {
        let entry = std::str::from_utf8(entry).ok()?;
        entry
            .strip_prefix(key)?
            .strip_prefix('=')
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    })
}

/// Every absolute path `pid` holds open, with the kernel's ` (deleted)` suffix removed.
pub(crate) fn open_files(root: &Path, pid: u32) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root.join(pid.to_string()).join("fd")) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let Ok(target) = fs::read_link(entry.path()) else {
            continue;
        };
        let text = target.to_string_lossy();
        if !text.starts_with('/') {
            continue;
        }
        let path = PathBuf::from(text.strip_suffix(" (deleted)").unwrap_or(&text));
        if !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

/// Every absolute path `pid` or a descendant holds open.
pub(crate) fn tree_open_files(root: &Path, pid: u32) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for member in tree(root, pid) {
        for path in open_files(root, member) {
            if !files.contains(&path) {
                files.push(path);
            }
        }
    }
    files
}

#[cfg(test)]
pub(crate) mod fake {
    //! A `/proc` tree on disk for tests.

    use std::fs;
    use std::path::{Path, PathBuf};

    /// Write a process `pid` with parent `ppid`, start time `ticks`, environment `env` and open
    /// files `files` under `root`.
    pub(crate) fn process(
        root: &Path,
        pid: u32,
        ppid: u32,
        ticks: u64,
        env: &[(&str, &str)],
        files: &[PathBuf],
    ) {
        let dir = root.join(pid.to_string());
        fs::create_dir_all(dir.join("fd")).unwrap();
        // Fields after the name: state, ppid, then 17 more before starttime.
        let mut fields = vec!["S".to_owned(), ppid.to_string()];
        fields.extend(std::iter::repeat_n("0".to_owned(), 17));
        fields.push(ticks.to_string());
        fs::write(
            dir.join("stat"),
            format!("{pid} (a (b) c) {}\n", fields.join(" ")),
        )
        .unwrap();
        let environ: Vec<u8> = env
            .iter()
            .flat_map(|(key, value)| format!("{key}={value}\0").into_bytes())
            .collect();
        fs::write(dir.join("environ"), environ).unwrap();
        for (index, file) in files.iter().enumerate() {
            std::os::unix::fs::symlink(file, dir.join("fd").join(index.to_string())).unwrap();
        }
    }

    /// A fresh scratch directory.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "agent-harness-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{process, scratch};
    use super::*;

    #[test]
    fn a_tree_holds_descendants_and_their_open_files() {
        let root = scratch("proc-tree");
        let log = root.join("log.jsonl");
        fs::write(&log, b"").unwrap();
        process(
            &root,
            10,
            1,
            500,
            &[("HOME", "/home/a"), ("EMPTY", "")],
            &[],
        );
        process(&root, 11, 10, 501, &[], std::slice::from_ref(&log));
        process(&root, 12, 11, 502, &[], &[]);
        process(&root, 13, 1, 503, &[], &[]);
        assert_eq!(tree(&root, 10), [10, 11, 12]);
        assert_eq!(start_ticks(&root, 11), Some(501));
        assert_eq!(environment(&root, 10, "HOME").as_deref(), Some("/home/a"));
        assert_eq!(environment(&root, 10, "EMPTY"), None);
        assert_eq!(environment(&root, 10, "HOM"), None);
        assert_eq!(tree_open_files(&root, 10), [log]);
        assert!(open_files(&root, 99).is_empty());
        fs::remove_dir_all(root).ok();
    }
}
