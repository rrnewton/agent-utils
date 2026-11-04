//! Shared fixtures: a bare "remote", a working clone that plays the developers, and a clone the
//! queue operates on, all in a private temporary directory.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary directory removed on drop.
pub struct Fixture {
    /// Root of the fixture.
    pub root: PathBuf,
}

const IDENTITY: [(&str, &str); 7] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    // Marks a test run. Some hosts wrap git in a logger that starts a telemetry process for
    // every call unless TESTTMP is set; it outlives the call and costs several times the call's
    // own CPU, and these fixtures make tens of thousands of git calls.
    ("TESTTMP", "1"),
];

impl Fixture {
    /// Create a bare remote with one commit on `main`, a developer clone and a queue clone.
    pub fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!(
            "merge-queue-it-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let fixture = Self { root };
        git(
            &fixture.root,
            &["init", "-q", "--bare", "-b", "main", "remote.git"],
        );
        git(&fixture.root, &["clone", "-q", "remote.git", "dev"]);
        fixture.write("README", "base\n");
        fixture.write("shared.txt", "1\n2\n3\n4\n5\n");
        fixture.commit_all("base");
        git(&fixture.dev(), &["push", "-q", "origin", "HEAD:main"]);
        git(&fixture.root, &["clone", "-q", "remote.git", "queue"]);
        fixture
    }

    /// Developer clone.
    pub fn dev(&self) -> PathBuf {
        self.root.join("dev")
    }

    /// Queue clone.
    pub fn queue(&self) -> PathBuf {
        self.root.join("queue")
    }

    /// Bare remote.
    pub fn remote(&self) -> PathBuf {
        self.root.join("remote.git")
    }

    /// Write a file in the developer clone.
    pub fn write(&self, path: &str, text: &str) {
        let full = self.dev().join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full, text).unwrap();
    }

    /// Commit everything in the developer clone.
    pub fn commit_all(&self, message: &str) {
        git(&self.dev(), &["add", "-A"]);
        git(&self.dev(), &["commit", "-q", "-m", message]);
    }

    /// Push a branch from `main` with one commit writing `files`; returns its head.
    pub fn branch(&self, name: &str, files: &[(&str, &str)]) -> String {
        self.branch_from(name, "main", files)
    }

    /// Push a branch from `from` with one commit writing `files`; returns its head.
    pub fn branch_from(&self, name: &str, from: &str, files: &[(&str, &str)]) -> String {
        git(&self.dev(), &["checkout", "-q", "-B", name, from]);
        for (path, text) in files {
            self.write(path, text);
        }
        self.commit_all(&format!("{name}: change"));
        git(
            &self.dev(),
            &["push", "-q", "-f", "origin", &format!("HEAD:{name}")],
        );
        let head = git(&self.dev(), &["rev-parse", "HEAD"]);
        git(&self.dev(), &["checkout", "-q", "main"]);
        head
    }

    /// The remote's current base commit.
    pub fn remote_main(&self) -> String {
        git(&self.remote(), &["rev-parse", "main"])
    }

    /// Run the merge-queue binary against the queue clone.
    pub fn mq(&self, args: &[&str]) -> Output {
        self.mq_env(args, &[])
    }

    /// Run the merge-queue binary with extra environment variables.
    pub fn mq_env(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command.args(args).current_dir(self.queue());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        for (key, value) in env {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        command.output().unwrap()
    }

    /// Run the binary and require success; returns stdout.
    pub fn mq_ok(&self, args: &[&str]) -> String {
        let output = self.mq(args);
        assert!(
            output.status.success(),
            "merge-queue {args:?} failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Run git in `dir` with a fixed identity and no user configuration; panics on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(dir).args(args);
    for (key, value) in IDENTITY {
        command.env(key, value);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "git {args:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
