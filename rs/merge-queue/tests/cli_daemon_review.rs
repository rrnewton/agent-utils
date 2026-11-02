//! Daemon and notification regression tests: the notification sweep never
//! clobbers a concurrent writer and never hangs on a sink, the daemon
//! retries deferred changes on its own, `status` tells a working daemon from
//! a hung one, an unreadable item record is surfaced, and one daemon owns a
//! state directory.

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use merge_queue::notify::{sweep, Notifier};
use merge_queue::serve::failure_backoff;
use merge_queue::state::State;
use serde_json::Value;
use support::{git, Fixture};

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

fn setup(tag: &str, validator: &str, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn script(fx: &Fixture, name: &str, body: &str) -> PathBuf {
    let path = fx.root.join(name);
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn read_item(state: &Path, pr: i64) -> Value {
    let text = std::fs::read_to_string(state.join("items").join(format!("{pr}.json"))).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn write_item(state: &Path, pr: i64, value: &Value) {
    std::fs::write(
        state.join("items").join(format!("{pr}.json")),
        serde_json::to_string_pretty(value).unwrap(),
    )
    .unwrap();
}

/// The sweep re-reads each item before writing and writes only its
/// notification fields: a landing recorded by another writer while an
/// earlier send was in flight is never reverted to `queued`.
#[test]
fn sweep_keeps_a_concurrent_landing() {
    let fx = setup("rc-lost-update", "exit 0", &[]);
    let state_path = state_dir(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-x"]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--notify", "lane-x"]);
    fx.mq_ok(&["withdraw", "1"]); // #1 owes a `withdrawn` notification
                                  // #2: queued with a refusal deferral, so it owes `refusal-deferred`.
    let mut two = read_item(&state_path, 2);
    two["validation_refusals"] = 1.into();
    two["last_refusal"] = "exit 75".into();
    two["next_retry_at"] = "9999-01-01T00:00:00Z".into();
    write_item(&state_path, 2, &two);
    // The "agentctl" plays a concurrent lock-holding `run` that lands #2
    // while the send for #1 is in flight.
    let item2 = state_path.join("items/2.json");
    let agentctl = script(
        &fx,
        "agentctl-lands-2.sh",
        &format!(
            "#!/bin/sh\nif [ ! -e {flag} ]; then touch {flag}; python3 -I -c 'import json,sys; p=sys.argv[1]; d=json.load(open(p)); d[\"status\"]=\"landed\"; d[\"landed_as\"]=\"f\"*40; json.dump(d,open(p,\"w\"))' {item}; fi\nexit 0\n",
            flag = fx.root.join("landed.flag").display(),
            item = item2.display()
        ),
    );
    let state = State::at(&state_path);
    let config = state.config().unwrap();
    let notifier = Notifier::new(vec![agentctl.display().to_string()]);
    sweep(&state, &config, &notifier).unwrap();
    let after = read_item(&state_path, 2);
    assert_eq!(
        after["status"], "landed",
        "sweep clobbered a concurrent writer's landing: {after}"
    );
}

/// A hung notification sink cannot block the sweep (and so the daemon loop
/// and `sync`): the send is killed at its deadline and is a failed attempt.
#[test]
fn hung_agentctl_times_out_as_a_failed_attempt() {
    let fx = setup("rc-hung", "exit 0", &[]);
    let state_path = state_dir(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-x"]);
    fx.mq_ok(&["withdraw", "1"]);
    let agentctl = script(&fx, "agentctl-hang.sh", "#!/bin/sh\nsleep 5\n");
    let state = State::at(&state_path);
    let config = state.config().unwrap();
    let notifier = Notifier::new(vec![agentctl.display().to_string()])
        .with_timeout(Duration::from_millis(500));
    let start = Instant::now();
    sweep(&state, &config, &notifier).unwrap();
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "sweep blocked {:?} on a hung sink",
        start.elapsed()
    );
    let item = read_item(&state_path, 1);
    assert_eq!(item["notify_delivery"]["attempts"], 1, "{item}");
    assert_ne!(item["notify_delivery"]["delivered"], true, "{item}");
    assert!(
        item["notify_delivery"]["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("timed out"),
        "{item}"
    );
}

/// The sweep holds the writer lock: while another writer holds it, it
/// refuses instead of writing items behind that writer's back.
#[test]
fn sweep_refuses_while_another_writer_holds_the_lock() {
    let fx = setup("rc-sweep-lock", "exit 0", &[]);
    let state_path = state_dir(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--notify", "lane-x"]);
    fx.mq_ok(&["withdraw", "1"]);
    let state = State::at(&state_path);
    let config = state.config().unwrap();
    let _held = state.lock().unwrap();
    let notifier = Notifier::new(vec!["/definitely/missing/agentctl".to_owned()]);
    assert!(sweep(&state, &config, &notifier).is_err());
    let item = read_item(&state_path, 1);
    assert!(
        item.get("notify_delivery").is_none() || item["notify_delivery"]["attempts"] == 0,
        "{item}"
    );
}

struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start_daemon(fx: &Fixture) -> Daemon {
    let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
    command
        .args(["serve", "--poll-ms", "50", "--base-poll-secs", "1"])
        .current_dir(fx.queue())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("GIT_AUTHOR_NAME", "Dev")
        .env("GIT_AUTHOR_EMAIL", "dev@example.com")
        .env("GIT_COMMITTER_NAME", "Dev")
        .env("GIT_COMMITTER_EMAIL", "dev@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("MERGE_QUEUE_STATE");
    Daemon(command.spawn().unwrap())
}

fn wait_for(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// After a refusal deferral the daemon retries on its own once
/// `next_retry_at` passes, though items/ and the base are unchanged.
#[test]
fn daemon_retries_a_deferred_change_when_its_backoff_passes() {
    let fx = Fixture::new("rc-deferred");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let marker = fx.root.join("refused-once");
    let validator = format!(
        "if [ -e {m} ]; then exit 0; else touch {m}; exit 75; fi",
        m = marker.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--refusal-max-retries",
        "0",
        "--refusal-backoff-secs",
        "1",
    ]);
    let state_path = state_dir(&fx);
    let _daemon = start_daemon(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(
        wait_for(Duration::from_secs(20), || read_item(&state_path, 1)
            ["validation_refusals"]
            .as_u64()
            .unwrap_or(0)
            >= 1),
        "first refusal never recorded"
    );
    let landed = wait_for(Duration::from_secs(12), || {
        read_item(&state_path, 1)["status"] == "landed"
    });
    assert!(
        landed,
        "deferred change never retried by the daemon after its 1s backoff: {}",
        read_item(&state_path, 1)
    );
}

/// `status` reports a daemon in a long work cycle as running (the heartbeat
/// is refreshed during the cycle), says how long it has been working, and
/// advises no kill.
#[test]
fn status_reports_a_working_daemon_as_running() {
    let fx = setup("rc-stale", "sleep 6; exit 0", &[]);
    let state_path = state_dir(&fx);
    let _daemon = start_daemon(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(wait_for(Duration::from_secs(10), || {
        std::fs::read_to_string(state_path.join("serve.heartbeat.json"))
            .unwrap_or_default()
            .contains("\"working\"")
    }));
    std::thread::sleep(Duration::from_secs(4));
    let out = fx.mq(&["status", "--stale-after-secs", "2", "--json"]);
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(
        report["daemon"], "running",
        "working daemon reported unhealthy: {report}"
    );
    assert!(
        report["working_secs"]
            .as_u64()
            .is_some_and(|secs| secs >= 3),
        "{report}"
    );
    assert!(report.get("remedy").is_none(), "{report}");
    let text = fx.mq_ok(&["status", "--stale-after-secs", "2"]);
    assert!(text.contains("working for "), "{text}");
    assert!(!text.contains("kill"), "{text}");
}

/// One unreadable item record is surfaced in the heartbeat and makes
/// `status` unhealthy, instead of reading as an empty queue.
#[test]
fn corrupt_item_is_surfaced_not_read_as_an_empty_queue() {
    let fx = setup("rc-corrupt", "exit 0", &[]);
    let state_path = state_dir(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    std::fs::write(state_path.join("items/999.json"), "{").unwrap();
    let out = fx.mq(&["serve", "--once", "--poll-ms", "50"]);
    assert!(out.status.success());
    let hb: Value = serde_json::from_str(
        &std::fs::read_to_string(state_path.join("serve.heartbeat.json")).unwrap(),
    )
    .unwrap();
    let status = read_item(&state_path, 1)["status"].clone();
    assert!(
        status == "landed" || !hb["last_error"].as_str().unwrap_or_default().is_empty(),
        "queued change neither landed nor any error surfaced: item {status}, heartbeat {hb}"
    );
    assert!(
        hb["last_error"]
            .as_str()
            .unwrap_or_default()
            .contains("999.json"),
        "{hb}"
    );
    let out = fx.mq(&["status", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "unreadable queue must be unhealthy"
    );
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert!(
        report["items_error"]
            .as_str()
            .is_some_and(|e| e.contains("999.json")),
        "{report}"
    );
}

/// One daemon per state directory, enforced by a lock rather than the pid
/// file: a second daemon is refused and leaves the first one's pid file
/// alone, and a leftover pid file naming a live unrelated process (pid
/// reuse) neither blocks a start nor reads as a running daemon.
#[test]
fn single_daemon_guard_is_a_lock_not_the_pid_file() {
    let fx = setup("rc-guard", "exit 0", &[]);
    let state_path = state_dir(&fx);
    // A stale pid file naming a live process that is not a daemon (this
    // test process), plus a lock file nobody holds: reported dead.
    std::fs::write(
        state_path.join("serve.pid"),
        format!("{}\n", std::process::id()),
    )
    .unwrap();
    std::fs::write(state_path.join("serve.lock"), "").unwrap();
    let out = fx.mq(&["status", "--json"]);
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(report["daemon"], "dead", "{report}");
    // And it does not block a start.
    let mut first = start_daemon(&fx);
    let pid = first.0.id().to_string();
    assert!(
        wait_for(Duration::from_secs(10), || {
            std::fs::read_to_string(state_path.join("serve.pid"))
                .is_ok_and(|text| text.trim() == pid)
        }),
        "first daemon never recorded its pid"
    );
    let second = fx.mq(&["serve", "--once"]);
    assert!(!second.status.success(), "second daemon must be refused");
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("already running"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(state_path.join("serve.pid"))
            .unwrap()
            .trim(),
        pid,
        "the refused daemon must not touch the running daemon's pid file"
    );
    let _ = first.0.kill();
    let _ = first.0.wait();
    // SIGKILL left the pid file behind; the freed lock says dead.
    let out = fx.mq(&["status", "--json"]);
    let report: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(report["daemon"], "dead", "{report}");
    // A one-shot daemon starts fine after the crash.
    let out = fx.mq(&["serve", "--once"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Failed work cycles are retried with bounded exponential backoff.
#[test]
fn failed_cycle_backoff_doubles_and_caps() {
    assert_eq!(failure_backoff(1), Duration::from_secs(1));
    assert_eq!(failure_backoff(2), Duration::from_secs(2));
    assert_eq!(failure_backoff(4), Duration::from_secs(8));
    assert_eq!(failure_backoff(9), Duration::from_secs(256));
    assert_eq!(failure_backoff(10), Duration::from_secs(300));
    assert_eq!(failure_backoff(u32::MAX), Duration::from_secs(300));
}
