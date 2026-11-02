//! `serve` daemon and `status` command: the daemon lands submitted changes
//! without a manual `run`, `status` tells the truth about liveness, and the
//! writer lock is free while the daemon is idle.

mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

const IDENTITY: [(&str, &str); 6] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
];

fn state_dir(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let path = PathBuf::from(common);
    let absolute = if path.is_absolute() {
        path
    } else {
        fx.queue().join(path)
    };
    absolute.join("merge-queue")
}

fn setup(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
    ]);
    fx
}

struct Daemon(Child);

impl Daemon {
    fn start(fx: &Fixture) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args(["serve", "--poll-ms", "50", "--base-poll-secs", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
    }

    fn stop(mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn item_status(fx: &Fixture, pr: i64) -> String {
    let out = fx.mq_ok(&["list", "--all", "--json"]);
    let items: Value = serde_json::from_str(&out).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

#[test]
fn serve_lands_a_submitted_change_without_a_manual_run() {
    let fx = setup("serve-reacts");
    let daemon = Daemon::start(&fx);
    let state = state_dir(&fx);
    wait_for("heartbeat", Duration::from_secs(5), || {
        state.join("serve.heartbeat.json").is_file()
    });
    // `status` agrees the daemon is alive while it idles.
    let status = fx.mq_ok(&["status"]);
    assert!(status.contains("daemon: running"), "{status}");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    wait_for("landing", Duration::from_secs(20), || {
        item_status(&fx, 1) == "landed"
    });
    assert!(
        git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"]).contains("tests/a"),
        "landed file missing on remote main"
    );
    daemon.stop();
}

#[test]
fn status_reports_not_running_dead_and_stale_truthfully() {
    let fx = setup("status-truth");
    let state = state_dir(&fx);
    // Not running: no pid file at all.
    let out = fx.mq(&["status"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "not-running must not read as success"
    );
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("not running"), "{text}");
    assert!(text.contains("merge-queue serve"), "remedy missing: {text}");
    let json_out = fx.mq(&["status", "--json"]);
    assert_eq!(json_out.status.code(), Some(1));
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&json_out.stdout)).unwrap();
    assert_eq!(report["daemon"], "not-running", "{report}");

    // Dead: pid file names a process that is not alive.
    std::fs::write(state.join("serve.pid"), "999999999\n").unwrap();
    let out = fx.mq(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("dead"), "{text}");
    assert!(text.contains("remedy:"), "{text}");

    // Stale: live pid (this test process) with an old heartbeat.
    std::fs::write(state.join("serve.pid"), format!("{}\n", std::process::id())).unwrap();
    let old = serde_json::json!({
        "pid": std::process::id(),
        "started_at": "2000-01-01T00:00:00Z",
        "heartbeat_at": "2000-01-01T00:00:00Z",
        "heartbeat_unix": 946684800,
        "cycles": 1,
        "state": "idle",
        "last_trigger": "none",
        "last_base_sha": "",
        "queued": 0,
        "active_batches": [],
        "window": 2,
        "slots_total": 0,
        "slots_in_use": 0
    });
    std::fs::write(
        state.join("serve.heartbeat.json"),
        serde_json::to_string_pretty(&old).unwrap(),
    )
    .unwrap();
    let out = fx.mq(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("stale"), "{text}");
    assert!(text.contains("remedy:"), "{text}");
    let json_out = fx.mq(&["status", "--json"]);
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&json_out.stdout)).unwrap();
    assert_eq!(report["daemon"], "stale", "{report}");
}

#[test]
fn serve_stops_gracefully_on_sigterm() {
    let fx = setup("serve-sigterm");
    let mut daemon = Daemon::start(&fx);
    let state = state_dir(&fx);
    wait_for("heartbeat", Duration::from_secs(5), || {
        state.join("serve.heartbeat.json").is_file()
    });
    let pid = daemon.0.id();
    let killed = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(killed.success());
    wait_for("daemon exit", Duration::from_secs(5), || {
        daemon.0.try_wait().is_ok_and(|s| s.is_some())
    });
    // A graceful stop removes the pid file: status says not running.
    assert!(!state.join("serve.pid").exists(), "pid file left behind");
    let out = fx.mq(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("not running"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn lock_is_free_while_the_daemon_is_idle() {
    let fx = setup("serve-lock");
    let daemon = Daemon::start(&fx);
    let state = state_dir(&fx);
    wait_for("heartbeat", Duration::from_secs(5), || {
        state.join("serve.heartbeat.json").is_file()
    });
    // Give the daemon a few idle cycles; it must not be holding the lock.
    std::thread::sleep(Duration::from_millis(400));
    // A writer command takes the lock: it succeeds immediately only
    // because the idle daemon holds nothing. (Checked before submitting,
    // because once a change is queued the daemon is legitimately busy.)
    let out = fx.mq(&["run", "--dry-run"]);
    assert!(
        out.status.success(),
        "dry-run blocked while daemon idle:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    let out = fx.mq(&["submit", "1", "--head", "a"]);
    assert!(
        out.status.success(),
        "submit blocked while daemon idle:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    daemon.stop();
}
