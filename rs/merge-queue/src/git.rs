//! Shell-out Git operations.
//!
//! The queue shells out to `git` for the same reason the landing planner does: the host's Git
//! already knows the repository's transport, credentials, signing, and attribute configuration.
//! Candidates are built with plumbing (`merge-tree --write-tree` plus `commit-tree`), so building
//! one never touches a working tree; only validation needs a checkout.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

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
    /// Combined, but these regenerable paths still hold conflict markers: the tip must be
    /// regenerated before anything validates it.
    NeedsRegen(String, Vec<String>),
}

/// Paths the caller can regenerate, so conflicts in them do not block a combination.
pub type Regenerable<'a> = &'a dyn Fn(&str) -> bool;

/// Nothing is regenerable.
pub fn nothing_regenerable(_: &str) -> bool {
    false
}

/// A Git repository driven through the `git` executable.
#[derive(Clone, Debug)]
pub struct Git {
    /// Repository directory (a working tree or a bare repository).
    pub dir: PathBuf,
    /// Command prefix for network operations, such as a proxy wrapper.
    pub net_wrapper: Vec<String>,
    /// `-c key=value` settings applied to every invocation (for example a union-merge
    /// attributes file).
    pub settings: Vec<String>,
    /// Name and email for commits the queue creates (committer of replayed commits, author and
    /// committer of merge and regeneration commits). `None` uses Git's own configuration.
    pub identity: Option<(String, String)>,
    /// Fetch without credentials: credential helpers are disabled and prompting is off for
    /// `fetch`, so a public repository costs no credential use.
    pub anonymous_fetch: bool,
}

impl Git {
    /// Drive the repository at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            net_wrapper: Vec::new(),
            settings: Vec::new(),
            identity: None,
            anonymous_fetch: false,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.dir);
        for setting in self.settings_for(args) {
            command.arg("-c").arg(setting);
        }
        if self.anonymous_fetch && args.first() == Some(&"fetch") {
            command.env("GIT_TERMINAL_PROMPT", "0");
        }
        command.args(args);
        if let Some((name, email)) = &self.identity {
            command
                .env("GIT_COMMITTER_NAME", name)
                .env("GIT_COMMITTER_EMAIL", email);
        }
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

    /// `-c` settings for one invocation.
    fn settings_for(&self, args: &[&str]) -> Vec<String> {
        let mut settings = self.settings.clone();
        if self.anonymous_fetch && args.first() == Some(&"fetch") {
            settings.push("credential.helper=".to_owned());
        }
        settings
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
            .arg(&self.dir);
        for setting in self.settings_for(args) {
            command.arg("-c").arg(setting);
        }
        if self.anonymous_fetch && args.first() == Some(&"fetch") {
            command.env("GIT_TERMINAL_PROMPT", "0");
        }
        command.args(args);
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

    /// Contents of `path` at `commit`, or `None` when the path does not exist there.
    pub fn show_file(&self, commit: &str, path: &str) -> Result<Option<String>, String> {
        let spec = format!("{commit}:{path}");
        match self.run(&["show", &spec]) {
            Ok(text) => Ok(Some(text)),
            Err(_) => Ok(None),
        }
    }

    /// Best common ancestor of two commits.
    pub fn merge_base(&self, left: &str, right: &str) -> Result<String, String> {
        self.run(&["merge-base", left, right])
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
            Err((_, paths)) => Ok(paths),
        }
    }

    /// `git merge-tree --write-tree`: `Ok(tree)` when clean, `Err((tree, paths))` on conflict,
    /// where `tree` holds the conflicted files with conflict markers.
    #[allow(clippy::type_complexity)]
    fn merge_tree(
        &self,
        left: &str,
        right: &str,
        merge_base: Option<&str>,
    ) -> Result<Result<String, (String, Vec<String>)>, String> {
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
        Ok(Err((tree, paths.into_iter().collect())))
    }

    /// A long-lived `git merge-tree --stdin` process: conflict queries without a process start
    /// each.
    pub fn merge_tree_batch(&self) -> Result<MergeTreeBatch, String> {
        let mut command = self.command(&[
            "merge-tree",
            "--stdin",
            "-z",
            "--name-only",
            "--no-messages",
        ]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start git merge-tree --stdin: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or("merge-tree --stdin has no stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("merge-tree --stdin has no stdout")?;
        Ok(MergeTreeBatch {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
        })
    }

    /// Commits reachable from `head` but not from `base`.
    pub fn range(&self, base: &str, head: &str) -> Result<BTreeSet<String>, String> {
        let range = format!("{base}..{head}");
        Ok(self
            .run(&["rev-list", &range])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
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

    /// Conflict-marker lines in the files that differ between two commits,
    /// as `path:line: text` at commit `to`. A committed merge-conflict
    /// marker makes downstream files unparseable (a JSON DAG with markers
    /// once reached main), so callers refuse before validating.
    pub fn conflict_markers(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let files = self.diff_files(from, to)?;
        if files.is_empty() {
            return Ok(Vec::new());
        }
        let mut args: Vec<&str> = vec![
            "grep",
            "-n",
            "-E",
            "^<<<<<<< |^=======$|^>>>>>>> ",
            to,
            "--",
        ];
        args.extend(files.iter().map(String::as_str));
        // git grep exits 1 when nothing matches: that is the clean case.
        let output = self.output(self.command(&args), "git grep (conflict markers)", &[0, 1])?;
        Ok(String::from_utf8_lossy(&output.stdout)
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
        use std::io::Write;
        // Revisions go on stdin, so any number of excluded heads fits.
        let mut input = format!("{head}\n");
        for rev in exclude {
            input.push_str(&format!("^{rev}\n"));
        }
        let mut command = self.command(&[
            "rev-list",
            "--reverse",
            "--topo-order",
            "--no-merges",
            "--stdin",
        ]);
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not run git rev-list: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(input.as_bytes())
                .map_err(|error| format!("could not feed git rev-list: {error}"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not run git rev-list: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git rev-list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
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
        } else if let Some((name, email)) = &self.identity {
            command
                .env("GIT_AUTHOR_NAME", name)
                .env("GIT_AUTHOR_EMAIL", email);
        }
        let output = self.output(command, "git commit-tree", &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Replay `commit` onto `onto` without a working tree (a plumbing cherry-pick).
    pub fn cherry_pick(&self, onto: &str, commit: &str) -> Result<Combined, String> {
        self.cherry_pick_with(onto, commit, &nothing_regenerable)
    }

    /// Replay `commit` onto `onto`; conflicts confined to regenerable paths are committed with
    /// their markers and reported as [`Combined::NeedsRegen`].
    pub fn cherry_pick_with(
        &self,
        onto: &str,
        commit: &str,
        regenerable: Regenerable<'_>,
    ) -> Result<Combined, String> {
        let parent = format!("{commit}^");
        match self.merge_tree(onto, commit, Some(&parent))? {
            Err((tree, paths)) if paths.iter().all(|path| regenerable(path)) => {
                let (name, email, date, message) = self.commit_meta(commit)?;
                let new = self.commit_tree(
                    &tree,
                    &[onto],
                    message.trim_end(),
                    Some((&name, &email, &date)),
                )?;
                Ok(Combined::NeedsRegen(new, paths))
            }
            Err((_, paths)) => Ok(Combined::Conflict(paths)),
            Ok(tree) => {
                // Like `git rebase`, drop a commit whose change the target already has.
                if tree == self.tree_of(onto)? {
                    return Ok(Combined::Clean(onto.to_owned()));
                }
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
        self.combine_with(
            mode,
            onto,
            head,
            exclude,
            merge_message,
            &nothing_regenerable,
        )
    }

    /// Whether `head` already sits on `onto` with linear history, so a rebase would change
    /// nothing and the change can land with its own commit IDs (a fast-forward).
    pub fn fast_forwards(&self, onto: &str, head: &str) -> Result<bool, String> {
        if !self.is_ancestor(onto, head)? {
            return Ok(false);
        }
        let range = format!("{onto}..{head}");
        Ok(self.run(&["rev-list", "--merges", &range])?.is_empty())
    }

    /// [`Git::combine`], letting conflicts confined to regenerable paths through as
    /// [`Combined::NeedsRegen`]. In rebase mode a change already on `onto` with linear history
    /// keeps its own commits, as `git rebase` would.
    pub fn combine_with(
        &self,
        mode: CombineMode,
        onto: &str,
        head: &str,
        exclude: &[&str],
        merge_message: &str,
        regenerable: Regenerable<'_>,
    ) -> Result<Combined, String> {
        match mode {
            CombineMode::Merge => match self.merge_tree(onto, head, None)? {
                Err((tree, paths)) if paths.iter().all(|path| regenerable(path)) => {
                    Ok(Combined::NeedsRegen(
                        self.commit_tree(&tree, &[onto, head], merge_message, None)?,
                        paths,
                    ))
                }
                Err((_, paths)) => Ok(Combined::Conflict(paths)),
                Ok(tree) => Ok(Combined::Clean(self.commit_tree(
                    &tree,
                    &[onto, head],
                    merge_message,
                    None,
                )?)),
            },
            CombineMode::Rebase => {
                if self.fast_forwards(onto, head)? {
                    return Ok(Combined::Clean(head.to_owned()));
                }
                let mut excluded: Vec<&str> = exclude.to_vec();
                excluded.push(onto);
                let mut tip = onto.to_owned();
                let mut stale: BTreeSet<String> = BTreeSet::new();
                for commit in self.own_commits(head, &excluded)? {
                    match self.cherry_pick_with(&tip, &commit, regenerable)? {
                        Combined::Clean(next) => tip = next,
                        Combined::NeedsRegen(next, paths) => {
                            tip = next;
                            stale.extend(paths);
                        }
                        conflict @ Combined::Conflict(_) => return Ok(conflict),
                    }
                }
                if stale.is_empty() {
                    Ok(Combined::Clean(tip))
                } else {
                    Ok(Combined::NeedsRegen(tip, stale.into_iter().collect()))
                }
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

/// A running `git merge-tree --stdin -z --name-only --no-messages`.
pub struct MergeTreeBatch {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl MergeTreeBatch {
    fn token(&mut self) -> Result<String, String> {
        let mut buffer = Vec::new();
        let read = self
            .stdout
            .read_until(0, &mut buffer)
            .map_err(|error| format!("merge-tree --stdin read failed: {error}"))?;
        if read == 0 {
            return Err("merge-tree --stdin ended early (bad commit?)".to_owned());
        }
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        Ok(String::from_utf8_lossy(&buffer).into_owned())
    }

    /// Paths that conflict when merging `left` and `right`; empty means clean.
    pub fn conflicts(&mut self, left: &str, right: &str) -> Result<Vec<String>, String> {
        let stdin = self.stdin.as_mut().ok_or("merge-tree --stdin is closed")?;
        writeln!(stdin, "{left} {right}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("merge-tree --stdin write failed: {error}"))?;
        // Each record: status NUL tree NUL (conflicted path NUL)* NUL.
        let status = self.token()?;
        let _tree = self.token()?;
        let mut paths = BTreeSet::new();
        loop {
            let token = self.token()?;
            if token.is_empty() {
                break;
            }
            paths.insert(token);
        }
        match status.as_str() {
            "1" => Ok(Vec::new()),
            "0" => Ok(paths.into_iter().collect()),
            other => Err(format!("merge-tree --stdin: unexpected status {other:?}")),
        }
    }
}

impl Drop for MergeTreeBatch {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
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
    fn conflict_markers_reports_only_changed_files() {
        let tmp = TempDir::new("git-markers");
        let git = init(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let marked = branch(
            &tmp.0,
            "marked",
            "main",
            &[(
                "dag.json",
                "{\n<<<<<<< HEAD\n\"a\": 1944,\n=======\n\"a\": 1975,\n>>>>>>> other\n}\n",
            )],
        );
        let markers = git.conflict_markers(&base, &marked).unwrap();
        assert_eq!(markers.len(), 3, "got: {markers:?}");
        assert!(markers.iter().all(|line| line.contains("dag.json")));
        // A clean change reports nothing, and an unchanged marked file in
        // the base is not re-reported for an unrelated change.
        let clean = branch(&tmp.0, "clean", "main", &[("b.txt", "b\n")]);
        assert!(git.conflict_markers(&base, &clean).unwrap().is_empty());
    }

    #[test]
    fn union_attributes_resolve_append_only_conflicts() {
        let tmp = TempDir::new("git-union");
        let mut git = init(&tmp.0);
        write(&tmp.0, "list.txt", "a\n");
        git_add_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let x = branch(&tmp.0, "x", "main", &[("list.txt", "a\nx\n")]);
        let y = branch(&tmp.0, "y", "main", &[("list.txt", "a\ny\n")]);
        assert_eq!(
            git.merge_conflicts(&x, &y).unwrap(),
            vec!["list.txt".to_owned()]
        );
        let attributes = tmp.0.join("union.attributes");
        std::fs::write(&attributes, "list.txt merge=union\n").unwrap();
        git.settings = vec![format!("core.attributesFile={}", attributes.display())];
        assert!(git.merge_conflicts(&x, &y).unwrap().is_empty());
        let Combined::Clean(tip) = git
            .combine(CombineMode::Rebase, &base, &x, &[], "")
            .unwrap()
        else {
            panic!("x applies");
        };
        let Combined::Clean(tip) = git.combine(CombineMode::Rebase, &tip, &y, &[], "").unwrap()
        else {
            panic!("y applies with union");
        };
        assert_eq!(
            git.run(&["show", &format!("{tip}:list.txt")]).unwrap(),
            "a\nx\ny"
        );
    }

    fn git_add_commit(dir: &std::path::Path) {
        super::testrepo::git(dir, &["add", "-A"]);
        super::testrepo::git(dir, &["commit", "-q", "-m", "more"]);
    }

    #[test]
    fn rebase_keeps_a_change_already_on_the_base() {
        let tmp = TempDir::new("git-ff");
        let git = init(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("a.txt", "a\n")]);
        assert_eq!(
            git.combine(CombineMode::Rebase, &base, &a, &[], "")
                .unwrap(),
            Combined::Clean(a.clone())
        );
        // On top of another change it must be replayed (new commit IDs).
        let b = branch(&tmp.0, "b", "main", &[("b.txt", "b\n")]);
        let Combined::Clean(tip) = git.combine(CombineMode::Rebase, &a, &b, &[], "").unwrap()
        else {
            panic!("b applies onto a");
        };
        assert_ne!(tip, b);
        assert!(git.is_ancestor(&a, &tip).unwrap());
    }

    #[test]
    fn conflicts_in_regenerable_paths_pass_through_with_markers() {
        let tmp = TempDir::new("git-regen");
        let git = init(&tmp.0);
        write(&tmp.0, "gen.json", "v0\n");
        git_add_commit(&tmp.0);
        let a = branch(
            &tmp.0,
            "a",
            "main",
            &[("gen.json", "va\n"), ("a.src", "a\n")],
        );
        let b = branch(
            &tmp.0,
            "b",
            "main",
            &[("gen.json", "vb\n"), ("b.src", "b\n")],
        );
        let regen = |path: &str| path == "gen.json";
        let Combined::NeedsRegen(tip, paths) = git
            .combine_with(CombineMode::Rebase, &a, &b, &[], "", &regen)
            .unwrap()
        else {
            panic!("conflict confined to gen.json should pass through");
        };
        assert_eq!(paths, vec!["gen.json".to_owned()]);
        let text = git.run(&["show", &format!("{tip}:gen.json")]).unwrap();
        assert!(text.contains("<<<<<<<"), "{text}");
        assert!(git.run(&["show", &format!("{tip}:b.src")]).is_ok());
        // Without the regenerable list it is an ordinary conflict.
        assert_eq!(
            git.combine(CombineMode::Rebase, &a, &b, &[], "").unwrap(),
            Combined::Conflict(vec!["gen.json".to_owned()])
        );
    }

    #[test]
    fn anonymous_fetch_disables_credential_helpers_for_fetch_only() {
        let mut git = Git::new("/nonexistent");
        git.anonymous_fetch = true;
        assert!(git
            .settings_for(&["fetch", "origin"])
            .contains(&"credential.helper=".to_owned()));
        assert!(git.settings_for(&["push", "origin"]).is_empty());
    }

    #[test]
    fn batch_merge_tree_agrees_with_single_calls() {
        let tmp = TempDir::new("git-batch");
        let git = init(&tmp.0);
        write(&tmp.0, "shared.txt", "1\n2\n3\n");
        git_add_commit(&tmp.0);
        let a = branch(&tmp.0, "a", "main", &[("shared.txt", "A\n2\n3\n")]);
        let b = branch(
            &tmp.0,
            "b",
            "main",
            &[("shared.txt", "B\n2\n3\n"), ("x", "x\n")],
        );
        let c = branch(&tmp.0, "c", "main", &[("c", "c\n")]);
        let mut batch = git.merge_tree_batch().unwrap();
        for (left, right) in [(&a, &b), (&a, &c), (&b, &c), (&b, &a)] {
            assert_eq!(
                batch.conflicts(left, right).unwrap(),
                git.merge_conflicts(left, right).unwrap()
            );
        }
        assert_eq!(
            batch.conflicts(&a, &b).unwrap(),
            vec!["shared.txt".to_owned()]
        );
        let base = git.rev_parse("main").unwrap();
        assert_eq!(git.range(&base, &a).unwrap(), [a.clone()].into());
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
