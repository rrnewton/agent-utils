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
use std::sync::atomic::{AtomicUsize, Ordering};

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
    /// Combined, but these regenerable paths conflicted and were settled on the target's side
    /// (see [`Git::resolve_to_target`]): the tip must be regenerated before anything validates
    /// it.
    NeedsRegen(String, Vec<String>),
}

/// Paths the caller can regenerate, so conflicts in them do not block a combination.
pub type Regenerable<'a> = &'a (dyn Fn(&str) -> bool + Sync);

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
    /// How long a network command ([`Git::run_net`]) may run before its process group is
    /// killed and it fails.
    pub net_timeout: std::time::Duration,
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
            net_timeout: crate::bounded::NET_TIMEOUT,
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

    /// Run a network git command through the configured wrapper, killed and failed after
    /// [`Git::net_timeout`].
    pub fn run_net(&self, args: &[&str]) -> Result<String, String> {
        let (command, what) = if self.net_wrapper.is_empty() {
            (self.command(args), format!("git {}", args.join(" ")))
        } else {
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
            (command, what)
        };
        let output = crate::bounded::output_within(command, self.net_timeout, &what)?;
        if !output.status.success() {
            return Err(format!(
                "{what} failed ({}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
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

    /// The bytes of the file `path` at `commit`, or `None` when no file is there.
    pub fn file_bytes(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let spec = format!("{commit}:{path}");
        let output = self.output(
            self.command(&["cat-file", "blob", &spec]),
            "git cat-file blob",
            &[0, 128],
        )?;
        Ok(output.status.success().then_some(output.stdout))
    }

    /// The commit the gitlink (submodule entry) `path` in `commit` records, if there is one.
    pub fn gitlink(&self, commit: &str, path: &str) -> Result<Option<String>, String> {
        let out = self.run(&["ls-tree", commit, "--", path])?;
        Ok(out.lines().find_map(|line| {
            let (meta, name) = line.split_once('\t')?;
            let mut fields = meta.split(' ');
            match (fields.next(), fields.next(), fields.next()) {
                (Some("160000"), Some("commit"), Some(id)) if name == path => Some(id.to_owned()),
                _ => None,
            }
        }))
    }

    /// Every path in `commit`'s tree (files and gitlinks).
    pub fn paths_in(&self, commit: &str) -> Result<Vec<String>, String> {
        let output = self.output(
            self.command(&["ls-tree", "-r", "-z", "--name-only", commit]),
            "git ls-tree",
            &[0],
        )?;
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    /// Whether `commit` is a commit present in the repository.
    pub fn has_commit(&self, commit: &str) -> bool {
        self.run(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
            .is_ok()
    }

    /// Store `bytes` as a blob and return its id.
    pub fn write_blob(&self, bytes: &[u8]) -> Result<String, String> {
        let mut command = self.command(&["hash-object", "-w", "--stdin"]);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not run git hash-object: {error}"))?;
        let fed = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(bytes));
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not run git hash-object: {error}"))?;
        fed.map_err(|error| format!("could not feed git hash-object: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git hash-object failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// A three-way merge of file contents (`git merge-file`): the merged bytes, or `None` when
    /// they conflict.
    pub fn merge_texts(
        &self,
        ours: &[u8],
        base: &[u8],
        theirs: &[u8],
    ) -> Result<Option<Vec<u8>>, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let git_dir = PathBuf::from(self.run(&["rev-parse", "--absolute-git-dir"])?);
        let stem = git_dir.join(format!(
            "merge-queue-merge-file-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let paths: Vec<PathBuf> = ["ours", "base", "theirs"]
            .iter()
            .map(|side| PathBuf::from(format!("{}.{side}", stem.display())))
            .collect();
        let result = (|| {
            for (path, bytes) in paths.iter().zip([ours, base, theirs]) {
                std::fs::write(path, bytes)
                    .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
            }
            let mut command = Command::new("git");
            command
                .arg("merge-file")
                .arg("-p")
                .args(&paths)
                .stdin(Stdio::null());
            let output = command
                .output()
                .map_err(|error| format!("could not run git merge-file: {error}"))?;
            match output.status.code() {
                Some(0) => Ok(Some(output.stdout)),
                Some(1..=127) => Ok(None),
                code => Err(format!(
                    "git merge-file failed ({}): {}",
                    code.unwrap_or(-1),
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
            }
        })();
        for path in &paths {
            let _ = std::fs::remove_file(path);
        }
        result
    }

    /// `tree` with each `(mode, id, path)` entry set (added, or replacing what is there; mode
    /// `0` removes the path), built in a throwaway index file like [`Git::replace_paths`].
    pub fn tree_with(
        &self,
        tree: &str,
        entries: &[(String, String, String)],
    ) -> Result<String, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let git_dir = PathBuf::from(self.run(&["rev-parse", "--absolute-git-dir"])?);
        let index = git_dir.join(format!(
            "merge-queue-index-set-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let indexed = |args: &[&str]| {
            let mut command = self.command(args);
            command
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_LITERAL_PATHSPECS", "1");
            command
        };
        let result = (|| {
            self.output(indexed(&["read-tree", tree]), "git read-tree", &[0])?;
            let mut input = Vec::new();
            for (mode, id, path) in entries {
                input.extend_from_slice(format!("{mode} {id}\t{path}\0").as_bytes());
            }
            self.feed(
                indexed(&["update-index", "-z", "--index-info"]),
                &input,
                "git update-index --index-info",
            )?;
            let output = self.output(indexed(&["write-tree"]), "git write-tree", &[0])?;
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })();
        let _ = std::fs::remove_file(&index);
        let _ = std::fs::remove_file(format!("{}.lock", index.display()));
        result
    }

    /// A commit of `tree` on `parent`, by the queue's identity.
    pub fn commit_on(&self, tree: &str, parent: &str, message: &str) -> Result<String, String> {
        self.commit_tree(tree, &[parent], message, None)
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
        self.merge_tree_favoring(left, right, merge_base, false)
    }

    /// [`Git::merge_tree`]; with `favor_left`, conflicting hunks of a text merge take `left`'s
    /// lines (`-X ours`), so only conflicts that are not text conflicts remain.
    #[allow(clippy::type_complexity)]
    fn merge_tree_favoring(
        &self,
        left: &str,
        right: &str,
        merge_base: Option<&str>,
        favor_left: bool,
    ) -> Result<Result<String, (String, Vec<String>)>, String> {
        let base_arg = merge_base.map(|base| format!("--merge-base={base}"));
        let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
        if favor_left {
            args.extend(["-X", "ours"]);
        }
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

    /// Conflict-marker lines at any commit in `from..to`, restricted to the
    /// machine-parsed files whose breakage is silent (`ci/dag/*.json`,
    /// `Cargo.lock` at any depth). A tip can resolve markers an intermediate
    /// commit introduced, but the intermediate commit still lands in
    /// history and breaks bisection, so range history is guarded too.
    /// Output lines carry the commit: `rev:path:line: text`.
    pub fn conflict_markers_in_range(&self, from: &str, to: &str) -> Result<Vec<String>, String> {
        let range = format!("{from}..{to}");
        let commits: Vec<String> = self
            .run(&["rev-list", &range])?
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        if commits.is_empty() {
            return Ok(Vec::new());
        }
        let mut args: Vec<&str> = vec!["grep", "-n", "-E", "^<<<<<<< |^=======$|^>>>>>>> "];
        args.extend(commits.iter().map(String::as_str));
        args.extend(["--", "ci/dag/*.json", "Cargo.lock", ":(glob)**/Cargo.lock"]);
        let output = self.output(self.command(&args), "git grep (range markers)", &[0, 1])?;
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

    /// A commit's full message.
    pub fn message(&self, commit: &str) -> Result<String, String> {
        self.run(&["log", "-1", "--format=%B", commit])
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
        // The message travels on stdin, never as an argument: Linux limits
        // one argv string to 131,072 bytes (MAX_ARG_STRLEN), so `-m` fails
        // with E2BIG on large consumer messages that git itself accepts.
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
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not run git commit-tree: {error}"))?;
        let fed = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(message.as_bytes()));
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not run git commit-tree: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git commit-tree failed ({}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        fed.map_err(|error| format!("could not feed git commit-tree: {error}"))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// Run `command` with `input` on its standard input; any exit but 0 is an error.
    fn feed(&self, mut command: Command, input: &[u8], what: &str) -> Result<(), String> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|error| format!("could not run {what}: {error}"))?;
        let fed = child
            .stdin
            .take()
            .map_or(Ok(()), |mut stdin| stdin.write_all(input));
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not run {what}: {error}"))?;
        if output.status.success() {
            fed.map_err(|error| format!("could not feed {what}: {error}"))
        } else {
            Err(format!(
                "{what} failed ({}): {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Merge `other` into `onto` (as [`Git::merge_tree`]) with every conflict settled on `onto`'s
    /// side, and return the tree. Conflicting hunks take `onto`'s lines (`-X ours`) while hunks
    /// that merge cleanly keep both sides' edits; a conflict that is not a text conflict
    /// (modify/delete, file/directory) takes `onto`'s whole entry, or its absence. `conflicted`
    /// is what the plain merge reported, every path of it regenerable. Settling them before
    /// anything runs keeps conflict markers out of every commit, so a generator that compiles one
    /// of these files still builds; the regeneration that follows rewrites the generated ones.
    fn resolve_to_target(
        &self,
        onto: &str,
        other: &str,
        merge_base: Option<&str>,
        conflicted: &[String],
    ) -> Result<String, String> {
        match self.merge_tree_favoring(onto, other, merge_base, true)? {
            Ok(tree) => Ok(tree),
            Err((tree, left)) => {
                if let Some(path) = left.iter().find(|path| !conflicted.contains(path)) {
                    return Err(format!(
                        "git merge-tree -X ours reported a conflict in {path} that the plain merge did not"
                    ));
                }
                self.replace_paths(&tree, onto, &left)
            }
        }
    }

    /// `tree` with each of `paths` replaced by its entry in commit `source`, or removed where
    /// `source` has none. Built in a throwaway index file in the Git directory, so no working
    /// tree and not the repository's own index is involved (the repository may be bare).
    fn replace_paths(&self, tree: &str, source: &str, paths: &[String]) -> Result<String, String> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let git_dir = PathBuf::from(self.run(&["rev-parse", "--absolute-git-dir"])?);
        let index = git_dir.join(format!(
            "merge-queue-index-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let indexed = |args: &[&str]| {
            let mut command = self.command(args);
            command
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_LITERAL_PATHSPECS", "1");
            command
        };
        let result = (|| {
            self.output(indexed(&["read-tree", tree]), "git read-tree", &[0])?;
            let mut list = vec!["ls-tree", "-r", "-z", "--full-tree", source, "--"];
            list.extend(paths.iter().map(String::as_str));
            let entries = self.output(indexed(&list), "git ls-tree", &[0])?.stdout;
            // Remove every path first (mode 0), then add `source`'s entries, which
            // `--index-info` reads in ls-tree's own format.
            let zero = "0".repeat(tree.len());
            let mut input = Vec::new();
            for path in paths {
                input.extend_from_slice(format!("0 {zero}\t{path}\0").as_bytes());
            }
            input.extend_from_slice(&entries);
            self.feed(
                indexed(&["update-index", "-z", "--index-info"]),
                &input,
                "git update-index --index-info",
            )?;
            let output = self.output(indexed(&["write-tree"]), "git write-tree", &[0])?;
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        })();
        let _ = std::fs::remove_file(&index);
        let _ = std::fs::remove_file(format!("{}.lock", index.display()));
        result
    }

    /// Replay `commit` onto `onto` without a working tree (a plumbing cherry-pick).
    pub fn cherry_pick(&self, onto: &str, commit: &str) -> Result<Combined, String> {
        self.cherry_pick_with(onto, commit, &nothing_regenerable)
    }

    /// Replay `commit` onto `onto`; conflicts confined to regenerable paths are settled on
    /// `onto`'s side ([`Git::resolve_to_target`]) and reported as [`Combined::NeedsRegen`].
    pub fn cherry_pick_with(
        &self,
        onto: &str,
        commit: &str,
        regenerable: Regenerable<'_>,
    ) -> Result<Combined, String> {
        let parent = format!("{commit}^");
        let (tree, settled) = match self.merge_tree(onto, commit, Some(&parent))? {
            Err((_, paths)) if paths.iter().all(|path| regenerable(path)) => (
                self.resolve_to_target(onto, commit, Some(&parent), &paths)?,
                paths,
            ),
            Err((_, paths)) => return Ok(Combined::Conflict(paths)),
            Ok(tree) => (tree, Vec::new()),
        };
        // Like `git rebase`, drop a commit whose change the target already has.
        let new = if tree == self.tree_of(onto)? {
            onto.to_owned()
        } else {
            let (name, email, date, message) = self.commit_meta(commit)?;
            let message = message.trim_end();
            self.commit_tree(&tree, &[onto], message, Some((&name, &email, &date)))?
        };
        Ok(if settled.is_empty() {
            Combined::Clean(new)
        } else {
            Combined::NeedsRegen(new, settled)
        })
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

    /// Whether `head` is a merge commit made directly on `onto`: `onto` is an ancestor of
    /// `head` and is its first parent. Landing it unchanged is then a fast-forward that keeps
    /// both parents, so the history it merges becomes an ancestor of the base. An `onto` that
    /// is merely some ancestor is not enough: the merge was made, and validated, on another
    /// base.
    pub fn fast_forwards_exact_merge(&self, onto: &str, head: &str) -> Result<bool, String> {
        if !self.is_ancestor(onto, head)? {
            return Ok(false);
        }
        let parents = self.run(&["rev-list", "--parents", "-n", "1", head])?;
        let parents: Vec<&str> = parents.split_whitespace().skip(1).collect();
        Ok(parents.len() > 1 && parents[0] == self.rev_parse(onto)?)
    }

    /// Whether `head` is a merge commit (two or more parents).
    pub fn is_merge(&self, head: &str) -> Result<bool, String> {
        let parents = self.run(&["rev-list", "--parents", "-n", "1", head])?;
        Ok(parents.split_whitespace().count() > 2)
    }

    /// Whether an exact change (one that must land as submitted, never replayed) can land at
    /// `head` on `onto`: a linear fast-forward ([`Git::fast_forwards`]) or a merge made
    /// directly on `onto` ([`Git::fast_forwards_exact_merge`]).
    pub fn lands_exactly(&self, onto: &str, head: &str) -> Result<bool, String> {
        Ok(self.fast_forwards(onto, head)? || self.fast_forwards_exact_merge(onto, head)?)
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
            CombineMode::Merge => {
                let (tree, settled) = match self.merge_tree(onto, head, None)? {
                    Err((_, paths)) if paths.iter().all(|path| regenerable(path)) => {
                        (self.resolve_to_target(onto, head, None, &paths)?, paths)
                    }
                    Err((_, paths)) => return Ok(Combined::Conflict(paths)),
                    Ok(tree) => (tree, Vec::new()),
                };
                let new = self.commit_tree(&tree, &[onto, head], merge_message, None)?;
                Ok(if settled.is_empty() {
                    Combined::Clean(new)
                } else {
                    Combined::NeedsRegen(new, settled)
                })
            }
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
    fn an_exact_merge_fast_forwards_only_from_its_own_first_parent() {
        let tmp = TempDir::new("git-exact-merge");
        let git = init(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        // Another history, unrelated to main, merged directly on the base.
        testrepo::git(&tmp.0, &["checkout", "-q", "--orphan", "other"]);
        testrepo::git(&tmp.0, &["rm", "-rfq", "."]);
        write(&tmp.0, "other.txt", "other\n");
        testrepo::git(&tmp.0, &["add", "-A"]);
        testrepo::git(&tmp.0, &["commit", "-q", "-m", "other history"]);
        let other = git.rev_parse("other").unwrap();
        testrepo::git(&tmp.0, &["checkout", "-q", "main"]);
        testrepo::git(
            &tmp.0,
            &[
                "merge",
                "-q",
                "--allow-unrelated-histories",
                "-m",
                "import",
                "other",
            ],
        );
        let merge = git.rev_parse("main").unwrap();
        testrepo::git(&tmp.0, &["reset", "-q", "--hard", &base]);
        assert!(
            !git.fast_forwards(&base, &merge).unwrap(),
            "a merge is not linear"
        );
        assert!(git.fast_forwards_exact_merge(&base, &merge).unwrap());
        assert!(git.lands_exactly(&base, &merge).unwrap());
        // Its second parent is an ancestor, not its first: not an exact merge onto it.
        assert!(!git.fast_forwards_exact_merge(&other, &merge).unwrap());
        // The base moved on: the merge was made on an older base.
        let moved = branch(&tmp.0, "moved", "main", &[("m.txt", "m\n")]);
        assert!(!git.fast_forwards_exact_merge(&moved, &merge).unwrap());
        assert!(!git.lands_exactly(&moved, &merge).unwrap());
        // A linear change on the base is no merge, but still lands exactly.
        let linear = branch(&tmp.0, "linear", "main", &[("l.txt", "l\n")]);
        assert!(!git.fast_forwards_exact_merge(&base, &linear).unwrap());
        assert!(git.lands_exactly(&base, &linear).unwrap());
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
    fn conflict_markers_in_range_catches_resolved_intermediate() {
        let tmp = TempDir::new("git-range-markers");
        let git = init(&tmp.0);
        write(&tmp.0, "ci/dag/validate.json", "{}\n");
        git_add_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let marked = branch(
            &tmp.0,
            "marked",
            "main",
            &[(
                "ci/dag/validate.json",
                "<<<<<<< HEAD\n\"a\": 1\n=======\n\"a\": 2\n>>>>>>> x\n",
            )],
        );
        // A later commit resolves the markers; the tip is clean...
        let fixed = branch(
            &tmp.0,
            "fixed",
            &marked,
            &[("ci/dag/validate.json", "{\"a\": 2}\n")],
        );
        assert!(git.conflict_markers(&base, &fixed).unwrap().is_empty());
        // ...but the range still names the intermediate commit's markers.
        let hits = git.conflict_markers_in_range(&base, &fixed).unwrap();
        assert_eq!(hits.len(), 3, "got: {hits:?}");
        assert!(hits
            .iter()
            .all(|line| line.contains("ci/dag/validate.json")));
        // Non-guarded files are out of scope for the range check.
        let other = branch(&tmp.0, "other", "main", &[("notes.txt", "<<<<<<< HEAD\n")]);
        assert!(git
            .conflict_markers_in_range(&base, &other)
            .unwrap()
            .is_empty());
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
    fn replay_preserves_a_message_over_the_argv_limit() {
        // Linux limits one argv string to 131,072 bytes (MAX_ARG_STRLEN), so a
        // message passed with `-m` can never reach `commit-tree` at all; the
        // message must travel on stdin. 140,000 bytes is over that limit.
        let tmp = TempDir::new("git-big-message");
        let mut git = init(&tmp.0);
        git.identity = Some(("Test".to_owned(), "test@example.com".to_owned()));
        let base = git.rev_parse("main").unwrap();
        let message = format!("big subject\n\n{}", "x".repeat(140_000));
        // Build the large-message commit with plumbing, feeding the message
        // on stdin so the fixture itself does not hit the argv limit.
        write(&tmp.0, "big.txt", "big\n");
        testrepo::git(&tmp.0, &["add", "-A"]);
        let tree = testrepo::git(&tmp.0, &["write-tree"]);
        let mut child = Command::new("git")
            .arg("-C")
            .arg(&tmp.0)
            .args(["commit-tree", &tree, "-p", &base])
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(message.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        let big = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        testrepo::git(&tmp.0, &["reset", "-q", "--hard", "main"]);
        // Land another change first so the big commit is replayed (not
        // fast-forwarded) through `commit_tree`.
        let a = branch(&tmp.0, "a", "main", &[("a.txt", "a\n")]);
        let Combined::Clean(tip) = git
            .combine(CombineMode::Rebase, &base, &a, &[], "")
            .unwrap()
        else {
            panic!("a should apply");
        };
        let Combined::Clean(tip2) = git
            .combine(CombineMode::Rebase, &tip, &big, &[], "")
            .unwrap()
        else {
            panic!("the large-message commit should replay");
        };
        assert_eq!(git.message(&tip2).unwrap(), message.trim_end());
    }

    #[test]
    fn conflicts_in_regenerable_paths_settle_on_the_target_side() {
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
        for mode in [CombineMode::Rebase, CombineMode::Merge] {
            let Combined::NeedsRegen(tip, paths) =
                git.combine_with(mode, &a, &b, &[], "", &regen).unwrap()
            else {
                panic!("{mode:?}: a conflict confined to gen.json should pass through");
            };
            assert_eq!(paths, vec!["gen.json".to_owned()], "{mode:?}");
            // The target's version, with no conflict markers for anything to trip on.
            assert_eq!(
                git.run(&["show", &format!("{tip}:gen.json")]).unwrap(),
                "va",
                "{mode:?}"
            );
            assert!(
                git.conflict_markers(&a, &tip).unwrap().is_empty(),
                "{mode:?}"
            );
            assert_eq!(git.run(&["show", &format!("{tip}:b.src")]).unwrap(), "b");
            assert_eq!(git.run(&["show", &format!("{tip}:a.src")]).unwrap(), "a");
        }
        // Without the regenerable list it is an ordinary conflict.
        assert_eq!(
            git.combine(CombineMode::Rebase, &a, &b, &[], "").unwrap(),
            Combined::Conflict(vec!["gen.json".to_owned()])
        );
    }

    #[test]
    fn only_the_conflicting_hunks_of_a_regenerable_path_take_the_target_side() {
        // A file the generator reads as well as writes (a static table compiled into it) must
        // stay well formed: the target's lines win where both sides edited the same lines, and
        // the change's edits elsewhere in the file survive.
        let tmp = TempDir::new("git-regen-hunks");
        let git = init(&tmp.0);
        write(
            &tmp.0,
            "table.rs",
            "(\"alpha\", 10),\n2\n3\n4\n(\"beta\", 5),\n",
        );
        git_add_commit(&tmp.0);
        let target = branch(
            &tmp.0,
            "target",
            "main",
            &[("table.rs", "(\"alpha\", 11),\n2\n3\n4\n(\"beta\", 5),\n")],
        );
        let change = branch(
            &tmp.0,
            "change",
            "main",
            &[
                (
                    "table.rs",
                    "// alpha grows by 3\n(\"alpha\", 13),\n2\n3\n4\n(\"beta\", 6),\n",
                ),
                ("src.rs", "uses beta\n"),
            ],
        );
        let regen = |path: &str| path == "table.rs";
        let Combined::NeedsRegen(tip, paths) = git
            .combine_with(CombineMode::Rebase, &target, &change, &[], "", &regen)
            .unwrap()
        else {
            panic!("a conflict confined to table.rs should pass through");
        };
        assert_eq!(paths, vec!["table.rs".to_owned()]);
        assert_eq!(
            git.run(&["show", &format!("{tip}:table.rs")]).unwrap(),
            "(\"alpha\", 11),\n2\n3\n4\n(\"beta\", 6),"
        );
        assert_eq!(
            git.run(&["show", &format!("{tip}:src.rs")]).unwrap(),
            "uses beta"
        );
    }

    #[test]
    fn a_modify_delete_conflict_in_a_regenerable_path_takes_the_target_side() {
        let tmp = TempDir::new("git-regen-delete");
        let git = init(&tmp.0);
        let cmd = |args: &[&str]| super::testrepo::git(&tmp.0, args);
        write(&tmp.0, "gen/old.json", "v0\n");
        write(&tmp.0, "gen/kept.json", "k0\n");
        git_add_commit(&tmp.0);
        // The target stopped generating old.json and rewrote kept.json; the change edited old.json
        // and deleted kept.json.
        cmd(&["checkout", "-q", "-b", "target", "main"]);
        cmd(&["rm", "-q", "gen/old.json"]);
        write(&tmp.0, "gen/kept.json", "k1\n");
        git_add_commit(&tmp.0);
        let target = git.rev_parse("HEAD").unwrap();
        cmd(&["checkout", "-q", "-b", "change", "main"]);
        write(&tmp.0, "gen/old.json", "v1\n");
        cmd(&["rm", "-q", "gen/kept.json"]);
        write(&tmp.0, "src.txt", "s\n");
        git_add_commit(&tmp.0);
        let change = git.rev_parse("HEAD").unwrap();
        cmd(&["checkout", "-q", "main"]);
        let regen = |path: &str| path.starts_with("gen/");
        let Combined::NeedsRegen(tip, paths) = git
            .combine_with(CombineMode::Rebase, &target, &change, &[], "", &regen)
            .unwrap()
        else {
            panic!("modify/delete conflicts confined to gen/ should pass through");
        };
        assert_eq!(
            paths,
            vec!["gen/kept.json".to_owned(), "gen/old.json".to_owned()]
        );
        assert_eq!(
            git.show_file(&tip, "gen/old.json").unwrap(),
            None,
            "deleted on the target, so deleted"
        );
        assert_eq!(
            git.show_file(&tip, "gen/kept.json").unwrap().as_deref(),
            Some("k1")
        );
        assert_eq!(
            git.show_file(&tip, "src.txt").unwrap().as_deref(),
            Some("s")
        );
    }

    #[test]
    fn a_conflict_outside_regenerable_paths_is_still_a_conflict() {
        let tmp = TempDir::new("git-regen-real");
        let git = init(&tmp.0);
        write(&tmp.0, "gen.json", "v0\n");
        write(&tmp.0, "real.txt", "r0\n");
        git_add_commit(&tmp.0);
        let target = branch(
            &tmp.0,
            "target",
            "main",
            &[("gen.json", "vt\n"), ("real.txt", "rt\n")],
        );
        let both = branch(
            &tmp.0,
            "both",
            "main",
            &[("gen.json", "vc\n"), ("real.txt", "rc\n")],
        );
        let real = branch(&tmp.0, "real", "main", &[("real.txt", "rc\n")]);
        let regen = |path: &str| path == "gen.json";
        for mode in [CombineMode::Rebase, CombineMode::Merge] {
            // Mixed with a generated-file conflict, the real conflict still wins, naming both.
            assert_eq!(
                git.combine_with(mode, &target, &both, &[], "", &regen)
                    .unwrap(),
                Combined::Conflict(vec!["gen.json".to_owned(), "real.txt".to_owned()]),
                "{mode:?}"
            );
            assert_eq!(
                git.combine_with(mode, &target, &real, &[], "", &regen)
                    .unwrap(),
                Combined::Conflict(vec!["real.txt".to_owned()]),
                "{mode:?}"
            );
        }
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
