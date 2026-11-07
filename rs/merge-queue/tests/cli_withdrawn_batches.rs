//! Withdrawn batches: a change withdrawn while its batch is in flight must
//! not be retried, waited on, or allowed to end the run for the changes
//! behind it.

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

fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr && i["status"] != "pending-intake")
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
        .map(|events| {
            events
                .iter()
                .map(|e| {
                    (
                        e["kind"].as_str().unwrap().to_owned(),
                        e["detail"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn status_json(fx: &Fixture) -> Value {
    let out = fx.mq(&["status", "--json"]);
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or(Value::Null)
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

fn remote_files(fx: &Fixture) -> Vec<String> {
    git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"])
        .lines()
        .map(str::to_owned)
        .collect()
}

struct Daemon(Child);

impl Daemon {
    fn spawn_serve(fx: &Fixture, extra: &[&str]) -> Child {
        let mut command = Command::new(env!("CARGO_BIN_EXE_merge-queue"));
        command
            .args(["serve", "--poll-ms", "50", "--base-poll-secs", "1"])
            .args(extra)
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (key, value) in IDENTITY {
            command.env(key, value);
        }
        command.env_remove("MERGE_QUEUE_STATE");
        command.spawn().expect("spawn serve")
    }

    /// A continuous daemon, as in cli_intake.rs.
    fn start(fx: &Fixture) -> Self {
        Self(Self::spawn_serve(fx, &[]))
    }

    /// One run (`serve --once`), as in cli_disjoint_root.rs.
    fn start_once(fx: &Fixture) -> Self {
        Self(Self::spawn_serve(
            fx,
            &["--once", "--depth", "3", "--max-batch", "1"],
        ))
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

/// A queue whose validator counts each attempt in `<root>/attempts`, waits
/// for `<root>/gate`, then refuses admission (COULD_NOT_RUN, exit 75).
/// Refusal retries are enabled with no backoff.
fn refusal_setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let attempts = fx.root.join("attempts");
    let gate = fx.root.join("gate");
    let validator = format!(
        "echo attempt >> '{attempts}'; while [ ! -e '{gate}' ]; do sleep 0.05; done; echo COULD_NOT_RUN; exit 75",
        attempts = attempts.display(),
        gate = gate.display()
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
        "3",
        "--refusal-backoff-secs",
        "0",
    ]);
    (fx, attempts, gate)
}

fn attempt_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Gated per-test-set setup, copied from cli_disjoint_root.rs: each
/// validation appends its test-set name (e.g. `a_t0_`) to `started` and
/// waits for a gate file of that name under `gates/`.
fn gated_setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
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
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, validator).unwrap();
    fx.mq_ok(&[
        "init",
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

fn started_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_refused_batch_withdrawn_before_its_retry_is_not_validated_again() {
    let (fx, attempts, gate) = refusal_setup("withdrawn-refused-retry");
    let before = fx.remote_main();
    let _daemon = Daemon::start(&fx);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    wait_for(
        "the validation to be in flight",
        Duration::from_secs(20),
        || attempt_lines(&attempts).len() == 1 && status_json(&fx)["slots_in_use"] == 1,
    );
    let out = fx.mq_ok(&["withdraw", "1"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    // The in-flight attempt now refuses; the withdrawal is already recorded,
    // so no retry may be started for the withdrawn batch.
    std::fs::write(&gate, "").unwrap();
    wait_for("the withdrawal", Duration::from_secs(30), || {
        status(&fx, 1) == "withdrawn"
    });
    // Give a wrongly-scheduled retry (backoff is 0) time to appear.
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        attempt_lines(&attempts).len(),
        1,
        "a withdrawn batch was validated again: {:?}",
        attempt_lines(&attempts)
    );
    assert_eq!(status(&fx, 1), "withdrawn");
    assert_eq!(fx.remote_main(), before, "a withdrawn change landed");
    let events = history(&item(&fx, 1));
    assert!(
        !events.iter().any(|(kind, _)| kind == "landed"),
        "{events:?}"
    );
}

#[test]
fn a_root_does_not_wait_for_a_withdrawn_batch_it_jumped_over() {
    let (fx, started, gates) = gated_setup("withdrawn-root-jumped-over");
    // Two root slots, so change 2 validates as a disjoint root on the base.
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(2);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    let before = fx.remote_main();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start_once(&fx);
    wait_for("both validations to start", Duration::from_secs(30), || {
        let lines = started_lines(&started);
        lines.iter().any(|line| line == "a_t0_") && lines.iter().any(|line| line == "b_t0_")
    });
    let out = fx.mq_ok(&["withdraw", "1"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    // Only the root's gate opens; change 1's gate stays closed, so the run
    // can only finish if the root does not wait for the withdrawn batch.
    open(&gates, "b_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert!(
        !gates.join("a_t0_").exists(),
        "change 1's gate was opened by the test"
    );
    assert_eq!(status(&fx, 1), "withdrawn");
    assert_eq!(status(&fx, 2), "landed");
    assert_ne!(fx.remote_main(), before, "change 2 did not land");
    let files = remote_files(&fx);
    assert!(files.iter().any(|f| f == "tests/b"), "{files:?}");
    assert!(!files.iter().any(|f| f == "tests/a"), "{files:?}");
    let events = history(&item(&fx, 1));
    assert!(
        !events.iter().any(|(kind, _)| kind == "landed"),
        "{events:?}"
    );
}

/// A queue with same-commit confirmation on whose validator fails test `a`
/// and gates every validation after the first (the culprit-search probes)
/// on `<root>/probe-gate`.
fn probing_setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let attempts = fx.root.join("attempts");
    let gate = fx.root.join("probe-gate");
    let validator = format!(
        r#"echo attempt >> '{attempts}'
n=$(wc -l < '{attempts}' | tr -d ' ')
if [ "$n" -gt 1 ]; then i=0; while [ ! -e '{gate}' ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; fi
echo "run-$$" > "$MQ_HANDLE_FILE"
rc=0
for t in $(ls tests); do
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        attempts = attempts.display(),
        gate = gate.display()
    );
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, validator).unwrap();
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
        "--confirm-reruns",
    ]);
    (fx, attempts, gate)
}

#[test]
fn withdrawing_a_red_batch_cancels_its_culprit_search_probes() {
    let (fx, attempts, _gate) = probing_setup("withdrawn-red-probes");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start_once(&fx);
    // The batch comes back red and its culprit search starts a probe,
    // which stays gated: the search is in flight.
    wait_for(
        "the culprit probe to start",
        Duration::from_secs(30),
        || attempt_lines(&attempts).len() >= 2,
    );
    let out = fx.mq_ok(&["withdraw", "1"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    // The probe's gate never opens: the run can only finish if the
    // withdrawn batch and its probes are cancelled, not waited on.
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "withdrawn");
    let events = history(&item(&fx, 1));
    assert!(
        !events.iter().any(|(kind, _)| kind == "ejected"),
        "{events:?}"
    );
}

#[test]
fn a_withdrawn_green_batch_does_not_end_the_run() {
    let (fx, started, gates) = gated_setup("withdrawn-green-batch");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start_once(&fx);
    wait_for(
        "the first batch to validate",
        Duration::from_secs(30),
        || started_lines(&started).iter().any(|line| line == "a_t0_"),
    );
    let out = fx.mq_ok(&["withdraw", "1"]);
    assert!(out.contains("recorded the withdrawal of #1"), "{out}");
    // Open every shape change 2 may be validated in: stacked on change 1,
    // rebuilt on the base, and the base probe.
    for set in ["a_t0_", "a_b_t0_", "b_t0_", "t0_"] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "withdrawn");
    assert_eq!(status(&fx, 2), "landed");
    let events = history(&item(&fx, 1));
    assert!(
        !events.iter().any(|(kind, _)| kind == "landed"),
        "{events:?}"
    );
    let files = remote_files(&fx);
    assert!(files.iter().any(|f| f == "tests/b"), "{files:?}");
    assert!(!files.iter().any(|f| f == "tests/a"), "{files:?}");
}
