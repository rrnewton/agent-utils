//! A batch that comes back red while a batch beneath it is still validating is decided in
//! order, but the batches built on its tree are cancelled at once: their changes, and requests
//! recorded meanwhile, are built beneath the red batch and validate in the freed slots instead of
//! waiting for the head.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const IDENTITY: [(&str, &str); 7] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    // Marks a test run (see support/mod.rs).
    ("TESTTMP", "1"),
];

/// A queue whose validations each append the candidate's test files to `started` (as
/// `a_b_t0_`), wait (at most 60 s) for the gate of that name, and then fail exactly when a test
/// file says FAIL. Returns the fixture, `started` and the gates directory.
fn setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let started = fx.root.join("started");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    let validator = format!(
        r#"set=$(ls tests | tr '\n' _); echo "$set" >> '{started}'
i=0; while [ ! -e '{gates}'/"$set" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
[ -e '{gates}'/"$set" ] || exit 1
rc=0
for t in $(ls tests); do
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        started = started.display(),
        gates = gates.display()
    );
    // In a file: the daemon's preflight reads the tools a validator command names.
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, validator).unwrap();
    fx.mq_ok(&[
        "init",
        "--no-speculative-stacking",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
    ]);
    (fx, started, gates)
}

fn open(gates: &Path, name: &str) {
    std::fs::write(gates.join(name), "").unwrap();
}

struct Daemon(Child);

impl Daemon {
    /// One run (`--once`), three batches of one change each in flight at most.
    fn start(fx: &Fixture) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args([
                "serve",
                "--once",
                "--poll-ms",
                "50",
                "--base-poll-secs",
                "1",
            ])
            .args(["--depth", "3", "--max-batch", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
    }

    fn exit_code(&mut self, timeout: Duration) -> Option<i32> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if let Ok(Some(status)) = self.0.try_wait() {
                return status.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the daemon did not exit within {timeout:?}");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn started_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn wait_for(what: &str, started: &Path, mut check: impl FnMut(&[String]) -> bool) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if check(&started_lines(started)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "timed out waiting for {what}; started: {:?}",
        started_lines(started)
    );
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr && item["status"] != "pending-intake")
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

#[test]
fn a_red_batch_behind_the_head_frees_the_slots_of_the_batches_built_on_it() {
    let (fx, started, gates) = setup("red-behind-head");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("the window to fill", &started, |lines| lines.len() >= 3);
    // The window's last slot is a disjoint root: change 3 validates on the base itself,
    // not on change 2's tree (the disjoint second root; before it, the third start was
    // the stacked a_b_c candidate).
    assert_eq!(started_lines(&started)[..3], ["a_t0_", "a_b_t0_", "c_t0_"]);
    // Change 2 fails while change 1, the head, still validates. Nothing was built on
    // change 2's tree, so nothing is cancelled: change 3's root validation stands.
    open(&gates, "a_b_t0_");
    open(&gates, "c_t0_");
    // A change submitted now is taken in and built on change 1, still before change 1
    // has finished; it is rebuilt on change 3 once change 3 lands.
    fx.branch("d", &[("tests/d", "ok\n")]);
    let out = fx.mq_ok(&["submit", "4", "--head", "d"]);
    assert!(out.contains("recorded #4"), "{out}");
    wait_for("change 4 taken in", &started, |lines| {
        lines.iter().any(|line| line == "a_d_t0_")
    });
    assert!(!gates.join("a_t0_").exists());
    // Change 1 lands, change 2 is decided red and ejected, and 3 and 4 land.
    open(&gates, "a_t0_");
    open(&gates, "a_d_t0_");
    open(&gates, "a_c_d_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 3), "landed");
    assert_eq!(status(&fx, 4), "landed");
    let files = git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]);
    for file in ["tests/a", "tests/c", "tests/d"] {
        assert!(files.lines().any(|line| line == file), "{files}");
    }
    assert!(!files.lines().any(|line| line == "tests/b"), "{files}");
    // Nothing was ever validated on change 2's tree but change 2 itself: with the
    // disjoint root there is no stacked candidate on it to cancel.
    let lines = started_lines(&started);
    assert_eq!(
        lines.iter().filter(|line| line.contains('b')).count(),
        1,
        "{lines:?}"
    );
    // Deciding change 2 later did not throw away the root or the intake batch: each
    // was validated once on the tree it landed from.
    for set in ["c_t0_", "a_c_d_t0_"] {
        assert_eq!(
            lines.iter().filter(|line| *line == set).count(),
            1,
            "{set}: {lines:?}"
        );
    }
    let runs = fx.queue().join(".git/merge-queue/runs");
    let run = std::fs::read_dir(&runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .max()
        .unwrap();
    let events = std::fs::read_to_string(run.join("events.jsonl")).unwrap();
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "red-behind-head"
                && event["detail"]
                    .as_str()
                    .unwrap()
                    .starts_with("#2 is red behind the head: nothing was built on it")
        }),
        "{events}"
    );
}
