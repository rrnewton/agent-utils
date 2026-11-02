//! Shell-out Git operations.
//!
//! The queue shells out to `git` for the same reason the landing planner does: the host's Git
//! already knows the repository's transport, credentials, signing, and attribute configuration.
//! Candidates are built with plumbing (`merge-tree --write-tree` plus `commit-tree`), so building
//! one never touches a working tree; only validation needs a checkout.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// How a change is combined onto the candidate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CombineMode {
    /// Replay the change's non-merge commits in order (a rebase merge; linear history).
    #[default]
    Rebase,
    /// Create one merge commit per change.
    Merge,
}

impl CombineMode {
    /// Stable machine-facing value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rebase => "rebase",
            Self::Merge => "merge",
        }
    }
}

/// Outcome of combining one change onto a commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Combined {
    /// The new tip commit.
    Clean(String),
    /// Textual conflict on these paths.
    Conflict(Vec<String>),
}

/// A Git repository driven through the `git` executable.
#[derive(Clone, Debug)]
pub struct Git {
    /// Repository directory (a working tree or a bare repository).
    pub dir: PathBuf,
    /// Command prefix for network operations, such as a proxy wrapper.
    pub net_wrapper: Vec<String>,
}

impl Git {
    /// Drive the repository at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            net_wrapper: Vec::new(),
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.dir).args(args);
        command
    }

    fn output(&self, mut command: Command, what: &str, allowed: &[i32]) -> Result<Output, String> {
        let output = command
            .output()
            .map_err(|error| format!("could not run {what}: {error}"))?;
        let code = output.status.code().unwrap_or(-1);
        if allowed.contains(&code) {
            Ok(output)
        } else {
            Err(format!(
                "{what} failed ({code}): {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Run git and return trimmed stdout.
    pub fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = self.output(self.command(args), &format!("git {}", args.join(" ")), &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Run a network git command through the configured wrapper.
    pub fn run_net(&self, args: &[&str]) -> Result<String, String> {
        if self.net_wrapper.is_empty() {
            return self.run(args);
        }
        let mut command = Command::new(&self.net_wrapper[0]);
        command
            .args(&self.net_wrapper[1..])
            .arg("git")
            .arg("-C")
            .arg(&self.dir)
            .args(args);
        let what = format!("{} git {}", self.net_wrapper.join(" "), args.join(" "));
        let output = self.output(command, &what, &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Resolve a revision to a commit SHA.
    pub fn rev_parse(&self, rev: &str) -> Result<String, String> {
        self.run(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{rev}^{{commit}}"),
        ])
        .map_err(|_| format!("cannot resolve {rev:?} to a commit"))
    }

    /// Resolve a revision, returning `None` when it does not exist.
    pub fn try_rev_parse(&self, rev: &str) -> Option<String> {
        self.rev_parse(rev).ok()
    }

    /// The tree of a commit.
    pub fn tree_of(&self, commit: &str) -> Result<String, String> {
        self.run(&["rev-parse", &format!("{commit}^{{tree}}")])
    }

    /// The Git common directory (shared by every worktree), absolute.
    pub fn common_dir(&self) -> Result<PathBuf, String> {
        let raw = self.run(&["rev-parse", "--git-common-dir"])?;
        let path = PathBuf::from(raw);
        Ok(if path.is_absolute() {
            path
        } else {
            self.dir.join(path)
        })
    }

    /// Whether `ancestor` is an ancestor of (or equal to) `descendant`.
    pub fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let output = self.output(
            self.command(&["merge-base", "--is-ancestor", ancestor, descendant]),
            "git merge-base --is-ancestor",
            &[0, 1],
        )?;
        Ok(output.status.success())
    }

    /// Paths that conflict when merging two commits; empty means a clean merge.
    pub fn merge_conflicts(&self, left: &str, right: &str) -> Result<Vec<String>, String> {
        match self.merge_tree(left, right, None)? {
            Ok(_) => Ok(Vec::new()),
            Err(paths) => Ok(paths),
        }
    }

    /// `git merge-tree --write-tree`: `Ok(tree)` when clean, `Err(paths)` on conflict.
    fn merge_tree(
        &self,
        left: &str,
        right: &str,
        merge_base: Option<&str>,
    ) -> Result<Result<String, Vec<String>>, String> {
        let base_arg = merge_base.map(|base| format!("--merge-base={base}"));
        let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
        if let Some(arg) = &base_arg {
            args.push(arg);
        }
        args.extend([left, right]);
        let output = self.output(self.command(&args), "git merge-tree", &[0, 1])?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut lines = stdout.lines();
        let tree = lines.next().unwrap_or_default().trim().to_owned();
        if output.status.success() {
            return Ok(Ok(tree));
        }
        let paths: BTreeSet<String> = lines
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        Ok(Err(paths.into_iter().collect()))
    }

    /// Files changed from the merge base of `base` and `head` through `head`.
    pub fn changed_files(&self, base: &str, head: &str) -> Result<BTreeSet<String>, String> {
        let range = format!("{base}...{head}");
        Ok(self
            .run(&["diff", "--name-only", "--no-renames", &range])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Files that differ between two commits.
    pub fn diff_files(&self, from: &str, to: &str) -> Result<BTreeSet<String>, String> {
        Ok(self
            .run(&["diff", "--name-only", "--no-renames", from, to])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Non-merge commits reachable from `head` but from none of `exclude`, oldest first.
    pub fn own_commits(&self, head: &str, exclude: &[&str]) -> Result<Vec<String>, String> {
        let negated: Vec<String> = exclude.iter().map(|rev| format!("^{rev}")).collect();
        let mut args = vec!["rev-list", "--reverse", "--topo-order", "--no-merges", head];
        args.extend(negated.iter().map(String::as_str));
        Ok(self
            .run(&args)?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    /// Author and message metadata for replaying a commit.
    fn commit_meta(&self, commit: &str) -> Result<(String, String, String, String), String> {
        let raw = self.run(&["log", "-1", "--format=%an%x00%ae%x00%aI%x00%B", commit])?;
        let mut parts = raw.splitn(4, '\0');
        let mut next = || parts.next().unwrap_or_default().to_owned();
        Ok((next(), next(), next(), next()))
    }

    fn commit_tree(
        &self,
        tree: &str,
        parents: &[&str],
        message: &str,
        author: Option<(&str, &str, &str)>,
    ) -> Result<String, String> {
        let mut args = vec!["commit-tree", tree];
        for parent in parents {
            args.extend(["-p", parent]);
        }
        args.extend(["-m", message]);
        let mut command = self.command(&args);
        if let Some((name, email, date)) = author {
            command
                .env("GIT_AUTHOR_NAME", name)
                .env("GIT_AUTHOR_EMAIL", email)
                .env("GIT_AUTHOR_DATE", date);
        }
        let output = self.output(command, "git commit-tree", &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Replay `commit` onto `onto` without a working tree (a plumbing cherry-pick).
    pub fn cherry_pick(&self, onto: &str, commit: &str) -> Result<Combined, String> {
        let parent = format!("{commit}^");
        match self.merge_tree(onto, commit, Some(&parent))? {
            Err(paths) => Ok(Combined::Conflict(paths)),
            Ok(tree) => {
                let (name, email, date, message) = self.commit_meta(commit)?;
                let message = message.trim_end();
                let new =
                    self.commit_tree(&tree, &[onto], message, Some((&name, &email, &date)))?;
                Ok(Combined::Clean(new))
            }
        }
    }

    /// Combine one change (`head`, excluding commits reachable from `exclude`) onto `onto`.
    pub fn combine(
        &self,
        mode: CombineMode,
        onto: &str,
        head: &str,
        exclude: &[&str],
        merge_message: &str,
    ) -> Result<Combined, String> {
        match mode {
            CombineMode::Merge => match self.merge_tree(onto, head, None)? {
                Err(paths) => Ok(Combined::Conflict(paths)),
                Ok(tree) => Ok(Combined::Clean(self.commit_tree(
                    &tree,
                    &[onto, head],
                    merge_message,
                    None,
                )?)),
            },
            CombineMode::Rebase => {
                let mut excluded: Vec<&str> = exclude.to_vec();
                excluded.push(onto);
                let mut tip = onto.to_owned();
                for commit in self.own_commits(head, &excluded)? {
                    match self.cherry_pick(&tip, &commit)? {
                        Combined::Clean(next) => tip = next,
                        conflict @ Combined::Conflict(_) => return Ok(conflict),
                    }
                }
                Ok(Combined::Clean(tip))
            }
        }
    }

    /// Point a ref at a commit.
    pub fn update_ref(&self, name: &str, commit: &str) -> Result<(), String> {
        self.run(&["update-ref", name, commit]).map(drop)
    }

    /// Delete a ref if present.
    pub fn delete_ref(&self, name: &str) -> Result<(), String> {
        self.run(&["update-ref", "-d", name]).map(drop)
    }

    /// Create a detached worktree at `path` checked out at `commit`.
    pub fn add_worktree(&self, path: &Path, commit: &str) -> Result<(), String> {
        if path.exists() {
            self.remove_worktree(path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let path_text = path.to_string_lossy();
        self.run(&["worktree", "add", "--detach", "--force", &path_text, commit])
            .map(drop)
    }

    /// Remove a worktree best-effort, including its directory.
    pub fn remove_worktree(&self, path: &Path) {
        let path_text = path.to_string_lossy();
        let _ = self.run(&["worktree", "remove", "--force", "--force", &path_text]);
        let _ = std::fs::remove_dir_all(path);
        let _ = self.run(&["worktree", "prune"]);
    }
}

#[cfg(test)]
pub(crate) mod testrepo {
    //! Throwaway repositories for unit tests.

    use super::Git;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// A temporary directory removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        /// Create a fresh directory under the system temp dir.
        pub fn new(tag: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default();
            let path = std::env::temp_dir()
                .join(format!("merge-queue-{tag}-{}-{nanos}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Run a git command in `dir` with a fixed identity, panicking on failure.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    /// Initialise a repository with one commit on `main`.
    pub fn init(dir: &Path) -> Git {
        git(dir, &["init", "-q", "-b", "main"]);
        write(dir, "README", "base\n");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "base"]);
        Git::new(dir)
    }

    /// Write a file relative to `dir`.
    pub fn write(dir: &Path, path: &str, text: &str) {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, text).unwrap();
    }

    /// Create `branch` from `from` with one commit writing `files`, then return to main.
    pub fn branch(dir: &Path, branch: &str, from: &str, files: &[(&str, &str)]) -> String {
        git(dir, &["checkout", "-q", "-b", branch, from]);
        for (path, text) in files {
            write(dir, path, text);
        }
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("{branch}: change")]);
        let sha = git(dir, &["rev-parse", "HEAD"]);
        git(dir, &["checkout", "-q", "main"]);
        sha
    }
}

#[cfg(test)]
mod unit {
    use super::testrepo::*;
    use super::*;

    #[test]
    fn rebase_combine_replays_commits_and_detects_conflicts() {
        let tmp = TempDir::new("git");
        let git = init(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("a.txt", "a\n")]);
        let b = branch(&tmp.0, "b", "main", &[("b.txt", "b\n")]);
        let c = branch(&tmp.0, "c", "main", &[("a.txt", "different\n")]);
        let Combined::Clean(tip) = git
            .combine(CombineMode::Rebase, &base, &a, &[], "")
            .unwrap()
        else {
            panic!("a should apply");
        };
        let Combined::Clean(tip2) = git.combine(CombineMode::Rebase, &tip, &b, &[], "").unwrap()
        else {
            panic!("b should apply");
        };
        assert!(git.is_ancestor(&base, &tip2).unwrap());
        assert_eq!(
            git.diff_files(&base, &tip2).unwrap(),
            ["a.txt".to_owned(), "b.txt".to_owned()].into()
        );
        assert_eq!(
            git.combine(CombineMode::Rebase, &tip, &c, &[], "").unwrap(),
            Combined::Conflict(vec!["a.txt".to_owned()])
        );
        assert_eq!(
            git.merge_conflicts(&a, &c).unwrap(),
            vec!["a.txt".to_owned()]
        );
        assert!(git.merge_conflicts(&a, &b).unwrap().is_empty());
    }

    #[test]
    fn merge_combine_makes_two_parent_commit() {
        let tmp = TempDir::new("git-merge");
        let git = init(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("a.txt", "a\n")]);
        let Combined::Clean(tip) = git
            .combine(CombineMode::Merge, &base, &a, &[], "Merge #1")
            .unwrap()
        else {
            panic!("merge should apply");
        };
        assert!(git.is_ancestor(&a, &tip).unwrap());
    }
}
