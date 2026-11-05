//! The disjoint second root: with a window of at least two, one slot validates a batch
//! built on the base itself, disjoint from the stack. When the stack's head goes red,
//! the root is not cancelled with it: it lands on its one validation, while the stacked
//! batch behind the red head is rebuilt and validated again.

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::{git, Fixture};

#[test]
fn a_disjoint_root_survives_a_red_stack_head() {
    let fx = Fixture::new("disjoint-root");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // Red exactly for candidates containing change 1's or change 2's test file: the
    // whole stacked chain is bad, and only the disjoint root can land.
    let validator = r#"for t in $(ls tests); do o=pass; if [ "$t" = a ] || [ "$t" = b ]; then o=fail; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    fx.branch("a", &[("tests/a", "bad\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(dirs.pop().unwrap().join("report.json")).unwrap(),
    )
    .unwrap();
    let mut ejected: Vec<i64> = report["ejected"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["pr"].as_i64().unwrap())
        .collect();
    ejected.sort_unstable();
    assert_eq!(ejected, vec![1, 2], "{report}");
    let mut landed: Vec<i64> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    landed.sort_unstable();
    assert_eq!(landed, vec![3], "{report}");
    // Change 3 rode the disjoint root: one batch validation, built on the base itself.
    let root_runs: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([3]))
        .collect();
    assert_eq!(root_runs.len(), 1, "{report}");
    assert_eq!(root_runs[0]["parent"], report["base_start"], "{report}");
    // Change 2 was stacked on the red head: cancelled and validated again on the base.
    let stacked_runs: Vec<&Value> = report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([2]))
        .collect();
    assert!(stacked_runs.len() >= 2, "{report}");
}

fn latest_report(fx: &Fixture) -> Value {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("report.json").is_file())
        .collect();
    dirs.sort();
    serde_json::from_str(&std::fs::read_to_string(dirs.pop().unwrap().join("report.json")).unwrap())
        .unwrap()
}

fn batch_validations(report: &Value, pr: i64) -> Vec<&Value> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([pr]))
        .collect()
}

#[test]
fn a_disjoint_root_lands_once_when_head_and_second_are_red() {
    let fx = Fixture::new("disjoint-root-red-red");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validator = r#"for t in $(ls tests); do o=pass; if [ "$t" = a ] || [ "$t" = b ]; then o=fail; fi; printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    fx.branch("a", &[("tests/a", "bad\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    let mut landed: Vec<i64> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| {
            b["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    landed.sort_unstable();
    assert_eq!(landed, vec![3, 4], "{report}");
    // The root is decided as soon as the batches it jumped over are decided:
    // it does not wait for change 4 in pending, so it lands on its first
    // validation, on the base it was built on.
    let root_runs = batch_validations(&report, 3);
    assert_eq!(root_runs.len(), 1, "{report}");
    assert_eq!(root_runs[0]["parent"], report["base_start"], "{report}");
    // A disjoint root never lands on a base other than its parent: the landed
    // tip's parent commit is exactly the base it was built on.
    let root_tip = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["prs"] == serde_json::json!([3]))
        .unwrap()["tip"]
        .as_str()
        .unwrap()
        .to_owned();
    let parent = git(&fx.remote(), &["rev-parse", &format!("{root_tip}^")]);
    assert_eq!(parent, report["base_start"].as_str().unwrap(), "{report}");
}

const IDENTITY: [(&str, &str); 7] = [
    ("GIT_AUTHOR_NAME", "Dev"),
    ("GIT_AUTHOR_EMAIL", "dev@example.com"),
    ("GIT_COMMITTER_NAME", "Dev"),
    ("GIT_COMMITTER_EMAIL", "dev@example.com"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("TESTTMP", "1"),
];

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

struct Daemon(Child);

impl Daemon {
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
fn a_disjoint_root_waits_for_a_likely_red_head_when_gated() {
    let fx = Fixture::new("disjoint-root-gated");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validator = r#"for t in $(ls tests); do printf '{"id":"%s","outcome":"pass"}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
        "--disjoint-root-min-red-percent",
        "50",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    // No decision history means no evidence of a red head: change 3 validates
    // stacked on change 2, not as a disjoint root on the base.
    let runs = batch_validations(&report, 3);
    assert_eq!(runs.len(), 1, "{report}");
    assert_ne!(runs[0]["parent"], report["base_start"], "{report}");
}

#[test]
fn a_disjoint_root_is_cancelled_the_moment_the_head_lands() {
    let (fx, started, gates) = gated_setup("disjoint-root-green-head");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("the window to fill", &started, |lines| lines.len() >= 3);
    assert_eq!(started_lines(&started)[..3], ["a_t0_", "a_b_t0_", "c_t0_"]);
    // The head lands while the root is still validating: the root is
    // cancelled at once, its slot freed, and change 3 is rebuilt on the
    // stack (on change 2's candidate) without its gate ever opening.
    open(&gates, "a_t0_");
    wait_for(
        "change 3 rebuilt after the head landed",
        &started,
        |lines| lines.iter().any(|line| line == "a_b_c_t0_"),
    );
    open(&gates, "a_b_t0_");
    open(&gates, "a_b_c_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
    let report = latest_report(&fx);
    let root_runs = batch_validations(&report, 3);
    assert_eq!(root_runs.len(), 2, "{report}");
    assert_eq!(
        root_runs[0].get("cancelled").and_then(Value::as_bool),
        Some(true),
        "{report}"
    );
    assert_eq!(root_runs[0]["parent"], report["base_start"], "{report}");
    assert_ne!(root_runs[1]["parent"], report["base_start"], "{report}");
}
