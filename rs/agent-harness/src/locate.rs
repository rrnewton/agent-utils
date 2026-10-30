//! What locates an agent's transcript, and what was found.

use std::fs;
use std::path::{Path, PathBuf};

/// What is known about one running agent. Every field is optional; each harness uses what it
/// can and says which evidence chose the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hints {
    /// The harness process (a wrapper that starts it counts: its descendants are searched).
    pub pid: Option<u32>,
    /// The agent's working directory.
    pub cwd: Option<PathBuf>,
    /// The home directory the harness runs under; the process's `HOME` when unset.
    pub home: Option<PathBuf>,
    /// The harness's own conversation id, when a record or the terminal multiplexer knows it.
    pub session: Option<String>,
    /// The root of the process table, `/proc` unless a test says otherwise.
    pub proc_root: PathBuf,
}

impl Hints {
    /// Hints with only the harness process `pid` known.
    #[must_use]
    pub fn for_process(pid: u32) -> Self {
        Self {
            pid: Some(pid),
            ..Self::default()
        }
    }

    /// The process table root.
    pub(crate) fn proc_root(&self) -> &Path {
        if self.proc_root.as_os_str().is_empty() {
            Path::new("/proc")
        } else {
            &self.proc_root
        }
    }

    /// A variable from the harness process's environment.
    pub(crate) fn environment(&self, key: &str) -> Option<String> {
        crate::proc::environment(self.proc_root(), self.pid?, key)
    }

    /// The home directory: the hint, else the process's `HOME`, else this process's.
    pub(crate) fn home_dir(&self) -> Option<PathBuf> {
        self.home
            .clone()
            .or_else(|| self.environment("HOME").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
    }

    /// The session hint, when it is a safe file-name component.
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session
            .as_deref()
            .filter(|value| !value.is_empty() && !value.contains(['/', '\0']) && *value != "..")
    }

    /// Every absolute path the harness process or a descendant holds open.
    pub(crate) fn open_files(&self) -> Vec<PathBuf> {
        self.pid
            .map(|pid| crate::proc::tree_open_files(self.proc_root(), pid))
            .unwrap_or_default()
    }
}

/// A transcript that was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Located {
    /// The file.
    pub path: PathBuf,
    /// Why this file: which hint chose it, in words for a log.
    pub evidence: String,
}

/// The newest of `paths` by modification time.
pub(crate) fn newest(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| {
            let modified = fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, path)| path)
}

/// The first entry `matches` accepts in the date-sharded tree `root/YYYY/MM/DD/...` that Codex
/// and Muse write, newest days first, searching at most `days` day directories.
pub(crate) fn find_in_dated_tree(
    root: &Path,
    days: usize,
    matches: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let sorted = |path: &Path| -> Vec<PathBuf> {
        let mut entries: Vec<PathBuf> = fs::read_dir(path)
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        entries.reverse();
        entries
    };
    let mut seen = 0;
    for year in sorted(root) {
        for month in sorted(&year) {
            for day in sorted(&month) {
                seen += 1;
                if seen > days {
                    return None;
                }
                for entry in sorted(&day) {
                    if let Some(found) = matches(&entry) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::fake::scratch;

    #[test]
    fn the_dated_tree_is_searched_newest_day_first_within_its_bound() {
        let root = scratch("dated");
        for day in ["2026/10/29", "2026/10/30"] {
            let dir = root.join(day).join("s-1");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("log"), day).unwrap();
        }
        let found = find_in_dated_tree(&root, 5, &|entry| {
            let log = entry.join("log");
            log.is_file().then_some(log)
        })
        .unwrap();
        assert!(found.to_string_lossy().contains("2026/10/30"));
        assert_eq!(
            find_in_dated_tree(&root, 0, &|entry| Some(entry.to_owned())),
            None
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_session_hint_must_be_one_safe_name() {
        let mut hints = Hints::default();
        for bad in ["", "a/b", "..", "a\0b"] {
            hints.session = Some(bad.to_owned());
            assert_eq!(hints.session_id(), None, "{bad:?}");
        }
        hints.session = Some("01a1-b2".to_owned());
        assert_eq!(hints.session_id(), Some("01a1-b2"));
        assert_eq!(Hints::default().proc_root(), Path::new("/proc"));
    }
}
