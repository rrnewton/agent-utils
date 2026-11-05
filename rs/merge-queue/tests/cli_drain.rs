//! Draining the serve daemon for an upgrade (`drain-for-upgrade`): the validation in flight
//! finishes and its batch lands, no new validation starts (no batch, no culprit-search probe),
//! nothing is ejected, and the daemon exits 0; an idle daemon exits at once; a request left for
//! an earlier daemon never drains a new one; a withdrawn request drains nothing; with no daemon
//! there is nothing to drain.

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

fn state_dir(fx: &Fixture) -> PathBuf {
    let common = PathBuf::from(git(&fx.queue(), &["rev-parse", "--git-common-dir"]));
    let absolute = if common.is_absolute() {
        common
    } else {
        fx.queue().join(common)
    };
    absolute.join("merge-queue")
}

/// A queue whose validations each append a line to `started`, wait (at most 30 s) for `gate`,
/// and then fail exactly when a test file says FAIL.
fn setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    setup_with(tag, &[])
}

fn setup_with(tag: &str, extra: &[&str]) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let started = fx.root.join("started");
    let gate = fx.root.join("gate");
    let validator = format!(
        r#"echo x >> '{started}'; i=0; while [ ! -e '{gate}' ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done; [ -e '{gate}' ] || exit 1
rc=0
for t in $(ls tests); do
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        started = started.display(),
        gate = gate.display()
    );
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    (fx, started, gate)
}

struct Daemon(Child);

impl Daemon {
    /// One validation at a time: depth 1.
    fn start(fx: &Fixture, max_batch: &str) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args(["serve", "--poll-ms", "50", "--base-poll-secs", "1"])
            .args(["--depth", "1", "--max-batch", max_batch])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
    }

    /// Wait for the daemon to exit by itself; its exit code.
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

    fn running(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path).map_or(0, |text| text.lines().count())
}

fn heartbeat(fx: &Fixture) -> Value {
    std::fs::read_to_string(state_dir(fx).join("serve.heartbeat.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn item_status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

fn newest_report(fx: &Fixture) -> Value {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(state_dir(fx).join("runs"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("report.json").is_file())
        .collect();
    dirs.sort();
    let text = std::fs::read_to_string(dirs.last().expect("a run report").join("report.json"));
    serde_json::from_str(&text.unwrap()).unwrap()
}

fn request_file(fx: &Fixture) -> PathBuf {
    state_dir(fx).join("serve.drain.json")
}

#[test]
fn a_drain_lands_the_validation_in_flight_starts_no_other_and_exits() {
    let (fx, started, gate) = setup("drain-in-flight");
    // Both queued before the daemon starts, so its first run plans both.
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx, "1");
    wait_for("the first validation", Duration::from_secs(30), || {
        lines(&started) == 1
    });
    let out = fx.mq_ok(&["drain-for-upgrade", "--reason", "new binary"]);
    assert!(out.contains("asked the serve daemon"), "{out}");
    let status = fx.mq_ok(&["status"]);
    assert!(
        status.contains("drain: requested at") && status.contains("new binary"),
        "{status}"
    );
    std::fs::write(&gate, "").unwrap();
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    // The batch in flight landed; the other change was never validated and stays queued.
    assert_eq!(item_status(&fx, 1), "landed");
    assert_eq!(item_status(&fx, 2), "queued");
    assert_eq!(lines(&started), 1);
    let report = newest_report(&fx);
    assert!(
        report["stopped"]
            .as_str()
            .unwrap()
            .starts_with("drained for upgrade"),
        "{report}"
    );
    assert!(!request_file(&fx).exists(), "the request is consumed");
}

#[test]
fn a_drain_starts_no_culprit_search_and_ejects_nothing() {
    // A red batch is split into batches of one change each (the default), or searched when
    // same-commit confirmation is on. Either way a drain lets the batch's validation finish
    // but starts no further validation, so both changes stay queued for the next daemon and
    // nothing is ejected.
    for (tag, extra) in [
        ("drain-red-batch", &[][..]),
        ("drain-red-search", &["--confirm-reruns"][..]),
    ] {
        let (fx, started, gate) = setup_with(tag, extra);
        // Both queued before the daemon starts, so its first run plans both.
        fx.branch("a", &[("tests/a", "ok\n")]);
        fx.branch("b", &[("tests/b", "FAIL\n")]);
        fx.mq_ok(&["submit", "1", "--head", "a"]);
        fx.mq_ok(&["submit", "2", "--head", "b"]);
        let mut daemon = Daemon::start(&fx, "2");
        wait_for("the batch's validation", Duration::from_secs(30), || {
            lines(&started) == 1
        });
        fx.mq_ok(&["drain-for-upgrade"]);
        std::fs::write(&gate, "").unwrap();
        assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0), "{tag}");
        assert_eq!(item_status(&fx, 1), "queued", "{tag}");
        assert_eq!(item_status(&fx, 2), "queued", "{tag}");
        assert_eq!(lines(&started), 1, "{tag}: no further validation started");
        let report = newest_report(&fx);
        assert!(report["ejected"].as_array().unwrap().is_empty(), "{report}");
        let stopped = report["stopped"].as_str().unwrap();
        assert!(
            stopped.starts_with("drained for upgrade") && stopped.contains("changes stay queued"),
            "{tag}: {stopped}"
        );
    }
}

#[test]
fn an_idle_daemon_drains_at_once_and_wait_returns_when_it_exits() {
    let (fx, _, _) = setup("drain-idle");
    let mut daemon = Daemon::start(&fx, "1");
    wait_for("an idle daemon", Duration::from_secs(20), || {
        heartbeat(&fx)["state"] == "idle"
    });
    let out = fx.mq_ok(&["drain-for-upgrade", "--wait", "--timeout", "30"]);
    assert!(out.contains("exited"), "{out}");
    assert_eq!(daemon.exit_code(Duration::from_secs(5)), Some(0));
}

#[test]
fn a_request_left_for_an_earlier_daemon_does_not_drain_a_new_one() {
    let (fx, _, gate) = setup("drain-stale-request");
    std::fs::write(&gate, "").unwrap();
    std::fs::create_dir_all(state_dir(&fx)).unwrap();
    std::fs::write(
        request_file(&fx),
        r#"{"schema":"merge-queue-drain/v1","pid":1,"requested_at":"2026-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    let mut daemon = Daemon::start(&fx, "1");
    wait_for(
        "the stale request to be removed",
        Duration::from_secs(20),
        || !request_file(&fx).exists(),
    );
    // Still serving: a change submitted now lands.
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    wait_for("landing", Duration::from_secs(30), || {
        item_status(&fx, 1) == "landed"
    });
    assert!(daemon.running());
}

#[test]
fn draining_needs_a_daemon_and_a_withdrawn_request_drains_nothing() {
    let (fx, started, gate) = setup("drain-cancel");
    let out = fx.mq(&["drain-for-upgrade"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no serve daemon is running"), "{stderr}");
    // Both queued before the daemon starts, so its first run plans both.
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx, "1");
    wait_for("the first validation", Duration::from_secs(30), || {
        lines(&started) == 1
    });
    fx.mq_ok(&["drain-for-upgrade"]);
    let out = fx.mq_ok(&["drain-for-upgrade", "--cancel"]);
    assert!(out.contains("withdrew the drain request"), "{out}");
    std::fs::write(&gate, "").unwrap();
    // Both batches validate and land; the daemon keeps serving.
    wait_for("both landings", Duration::from_secs(60), || {
        item_status(&fx, 2) == "landed"
    });
    assert_eq!(item_status(&fx, 1), "landed");
    assert_eq!(lines(&started), 2);
    assert!(daemon.running());
}
