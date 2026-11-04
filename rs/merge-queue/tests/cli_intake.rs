//! Intake while a run holds the writer lock: `submit`, `withdraw` and `retry` record their
//! request instead of failing, the next holder of the lock applies the requests in order, and a
//! withdrawal recorded while its batch validates keeps that batch from landing.

mod support;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
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
    fx.queue().join(".git").join("merge-queue")
}

/// Seed the remote and set the queue up with a command validator running `validator`.
fn init(fx: &Fixture, validator: &str) {
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
        validator,
    ]);
}

/// A queue whose validator passes at once.
fn queue(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    init(&fx, "exit 0");
    fx
}

/// A queue whose validations wait until the returned gate file exists, then pass.
fn gated(tag: &str) -> (Fixture, PathBuf) {
    let fx = Fixture::new(tag);
    let gate = fx.root.join("gate");
    init(
        &fx,
        &format!("while [ ! -e '{}' ]; do sleep 0.05; done", gate.display()),
    );
    (fx, gate)
}

struct Daemon(Child);

impl Daemon {
    fn start(fx: &Fixture) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args(["serve", "--poll-ms", "50", "--base-poll-secs", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        Self(command.spawn().expect("spawn serve"))
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

/// The change's item record, or `null` when it has none.
fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .cloned()
        .unwrap_or(Value::Null)
}

fn status(fx: &Fixture, pr: i64) -> String {
    item(fx, pr)["status"]
        .as_str()
        .unwrap_or("missing")
        .to_owned()
}

fn history(item: &Value) -> Vec<(String, String)> {
    item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["kind"].as_str().unwrap().to_owned(),
                e["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn status_json(fx: &Fixture) -> Value {
    let out = fx.mq(&["status", "--json"]);
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or(Value::Null)
}

/// Hold the writer lock, as a run does, until the returned file is dropped.
fn hold_writer_lock(fx: &Fixture) -> std::fs::File {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(state_dir(fx).join("lock"))
        .unwrap();
    file.try_lock_exclusive().unwrap();
    file
}

#[test]
fn a_submission_while_a_run_validates_is_recorded_and_lands_after_it() {
    let (fx, gate) = gated("intake-submit");
    let _daemon = Daemon::start(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    wait_for("the batch to validate", Duration::from_secs(20), || {
        status_json(&fx)["slots_in_use"] == 1
    });
    // The run holds the writer lock: the submission is recorded, not refused.
    fx.branch("b", &[("tests/b", "ok\n")]);
    let out = fx.mq_ok(&["submit", "2", "--head", "b", "--prioritized"]);
    assert!(out.contains("recorded #2"), "{out}");
    assert_eq!(item(&fx, 2), Value::Null);
    let report = status_json(&fx);
    assert_eq!(report["intake"]["pending"], 1, "{report}");
    let text = fx.mq_ok(&["status"]);
    assert!(text.contains("intake: 1 request(s)"), "{text}");
    // A head that does not resolve is refused at once, lock or not.
    let missing = fx.mq(&["submit", "3", "--head", "no-such-branch"]);
    assert!(!missing.status.success());
    let stderr = String::from_utf8_lossy(&missing.stderr);
    assert!(stderr.contains("does not resolve"), "{stderr}");
    std::fs::write(&gate, "").unwrap();
    wait_for("both changes to land", Duration::from_secs(30), || {
        status(&fx, 1) == "landed" && status(&fx, 2) == "landed"
    });
    let submitted = history(&item(&fx, 2));
    assert!(
        submitted.iter().any(|(kind, detail)| kind == "submitted"
            && detail.contains("recorded at")
            && detail.contains("while another writer held the queue")),
        "{submitted:?}"
    );
    assert_eq!(status_json(&fx)["intake"]["pending"], 0);
}

#[test]
fn a_withdrawal_recorded_while_its_batch_validates_keeps_the_batch_from_landing() {
    let (fx, gate) = gated("intake-withdraw");
    let _daemon = Daemon::start(&fx);
    let before = fx.remote_main();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    wait_for("the batch to validate", Duration::from_secs(20), || {
        status_json(&fx)["slots_in_use"] == 1
    });
    let out = fx.mq_ok(&["withdraw", "1", "--reason", "found a bug"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    // The validation passes, but the run reads the recorded withdrawal before it lands.
    std::fs::write(&gate, "").unwrap();
    wait_for("the withdrawal", Duration::from_secs(30), || {
        status(&fx, 1) == "withdrawn"
    });
    assert_eq!(fx.remote_main(), before, "a withdrawn change landed");
    let events = history(&item(&fx, 1));
    assert!(
        events
            .iter()
            .any(|(kind, detail)| kind == "withdrawn"
                && detail.starts_with("found a bug (recorded at")),
        "{events:?}"
    );
    assert!(
        !events.iter().any(|(kind, _)| kind == "landed"),
        "{events:?}"
    );
}

#[test]
fn requests_recorded_while_the_lock_is_held_apply_in_order_at_the_next_writer() {
    let fx = queue("intake-order");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let lock = hold_writer_lock(&fx);
    // Withdrawn, then submitted again: only the order of the two makes #1 queued.
    let out = fx.mq_ok(&["withdraw", "1"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    let out = fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(out.contains("recorded #1"), "{out}");
    let out = fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert!(out.contains("recorded #2"), "{out}");
    let out = fx.mq_ok(&["retry", "1"]);
    assert!(out.contains("recorded the retry of #1"), "{out}");
    // A change that is not in the queue is refused at once.
    let unknown = fx.mq(&["retry", "9"]);
    assert!(!unknown.status.success());
    assert!(
        String::from_utf8_lossy(&unknown.stderr).contains("#9 is not in the queue"),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    // Nothing is applied while the lock is held.
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(item(&fx, 2), Value::Null);
    assert_eq!(status_json(&fx)["intake"]["pending"], 4);
    drop(lock);
    // The next writer applies them first, in order, then runs.
    let out = fx.mq(&["run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("applied withdraw #1 recorded at"),
        "{stderr}"
    );
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    let kinds: Vec<String> = history(&item(&fx, 1))
        .into_iter()
        .map(|(kind, _)| kind)
        .collect();
    let at = kinds.iter().position(|k| k == "withdrawn").unwrap();
    assert_eq!(
        &kinds[at..at + 3],
        ["withdrawn", "submitted", "retry"],
        "{kinds:?}"
    );
    assert_eq!(status_json(&fx)["intake"]["pending"], 0);
}

#[test]
fn a_recorded_request_that_cannot_be_applied_is_kept_with_its_reason() {
    let fx = queue("intake-refused");
    fx.branch("c", &[("tests/c", "ok\n")]);
    let lock = hold_writer_lock(&fx);
    let out = fx.mq_ok(&["submit", "3", "--head", "c"]);
    assert!(out.contains("recorded #3"), "{out}");
    // The branch goes away before the submission is applied.
    git(&fx.dev(), &["push", "-q", "origin", ":c"]);
    drop(lock);
    let out = fx.mq(&["run"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("refused submit #3 recorded at") && stderr.contains("does not resolve"),
        "{stderr}"
    );
    assert_eq!(item(&fx, 3), Value::Null);
    let report = status_json(&fx);
    assert_eq!(report["intake"]["pending"], 0, "{report}");
    assert_eq!(report["intake"]["refused"], 1, "{report}");
    let refusal = &report["intake"]["recent_refusals"][0];
    assert_eq!(refusal["request"]["op"], "submit", "{report}");
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("does not resolve"),
        "{report}"
    );
    // `status` exits 1 here (no daemon runs); its report still names the refusal.
    let text = String::from_utf8_lossy(&fx.mq(&["status"]).stdout).into_owned();
    assert!(
        text.contains("intake refused: 1 in the last week") && text.contains("submit #3"),
        "{text}"
    );
}
