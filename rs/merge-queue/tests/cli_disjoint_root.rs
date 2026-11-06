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

/// After a red head is ejected and nothing the root jumped over landed, the
/// still-validating root becomes the stack head: refills build on its tip, so
/// when it lands the refills land straight after it with no base-moved
/// rebuild.
#[test]
fn a_decidable_root_becomes_the_stack_head_for_refills() {
    let (fx, started, gates) = gated_setup("disjoint-root-promoted");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("the window to fill", &started, |lines| lines.len() >= 3);
    assert_eq!(started_lines(&started)[..3], ["a_t0_", "a_b_t0_", "c_t0_"]);
    // The head and the batch on it come back red (the head's test fails), and
    // the culprit search may probe the base; the root keeps validating.
    open(&gates, "a_t0_");
    open(&gates, "a_b_t0_");
    open(&gates, "t0_");
    // The ejected head leaves the root decidable: the refill of change 2 must
    // be built on the root's tip (b_c), not on the base (b alone).
    wait_for("change 2 rebuilt on the root tip", &started, |lines| {
        lines.iter().any(|line| line == "b_c_t0_")
    });
    assert!(
        !started_lines(&started).iter().any(|line| line == "b_t0_"),
        "change 2 must not be rebuilt on the base: {:?}",
        started_lines(&started)
    );
    open(&gates, "c_t0_");
    open(&gates, "b_c_t0_");
    open(&gates, "b_c_d_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "ejected");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
    assert_eq!(status(&fx, 4), "landed");
    let lines = started_lines(&started);
    for set in ["b_c_t0_", "b_c_d_t0_"] {
        assert_eq!(
            lines.iter().filter(|line| *line == set).count(),
            1,
            "{set}: {lines:?}"
        );
    }
    let report = latest_report(&fx);
    let root_runs = batch_validations(&report, 3);
    assert_eq!(root_runs.len(), 1, "{report}");
    // Nothing was rebuilt for a moved base in this run.
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    assert!(
        !events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "base-moved"
        }),
        "{events}"
    );
    assert!(
        events.lines().any(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            event["kind"] == "disjoint-root-promoted"
        }),
        "{events}"
    );
}

/// A new change planned as a root while another root is already green but
/// not yet landed must not be built on the about-to-move base: its base
/// choice is deferred (bounded) until the pending landing completes, so
/// it validates once, on the post-landing base.
#[test]
fn a_new_root_waits_for_a_green_root_to_land_before_choosing_its_base() {
    let (fx, started, gates) = gated_setup("root-near-landing");
    // Two root slots, so a second root can be planned while the first is
    // still in flight.
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(2);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("both batches to validate", &started, |lines| {
        lines.len() >= 2
    });
    assert_eq!(started_lines(&started)[..2], ["a_t0_", "b_t0_"]);
    // Change 2's root comes back green but cannot land yet: change 1,
    // which it jumped over, is undecided.
    open(&gates, "b_t0_");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(status(&fx, 2), "queued");
    // Two new changes arrive: the first would be planned as a second root
    // on the current base, but that base is about to move when the green
    // root lands, so its base choice is deferred instead.
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        started_lines(&started).len(),
        2,
        "no new root may be built on the about-to-move base"
    );
    // Change 1 is decided (red, ejected): the green root lands, and only
    // then are the new roots built, on the post-landing base.
    open(&gates, "t0_");
    open(&gates, "a_t0_");
    wait_for("the new batches to validate", &started, |lines| {
        lines.iter().any(|line| line.contains('c')) && lines.iter().any(|line| line.contains('d'))
    });
    // Every candidate shape the new batches may take, stacked or rooted.
    for set in [
        "b_c_t0_",
        "b_d_t0_",
        "b_c_d_t0_",
        "c_d_t0_",
        "c_t0_",
        "d_t0_",
    ] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    let report = latest_report(&fx);
    assert_eq!(status(&fx, 1), "ejected", "{report}");
    assert_eq!(status(&fx, 2), "landed", "{report}");
    assert_eq!(status(&fx, 3), "landed", "{report}");
    assert_eq!(status(&fx, 4), "landed", "{report}");
    // Change 3 validated exactly once, and not on the run's starting base.
    let runs3 = batch_validations(&report, 3);
    assert_eq!(runs3.len(), 1, "{report}");
    assert_ne!(
        runs3[0]["parent"], report["base_start"],
        "change 3 must be built on the post-landing base: {report}"
    );
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    assert!(
        events.contains("root-near-landing") && events.contains("waits for green root #2"),
        "{events}"
    );
}

/// A held (INFRA-BLOCKED, not validating further) front item must not block
/// the greens behind it: the disjoint root built on the base lands in the
/// same run, and the stacked change behind the held item is rebuilt without
/// it and lands too. The run still ends on the held item's stop reason.
#[test]
fn a_held_front_item_does_not_block_the_greens_behind_it() {
    let fx = Fixture::new("held-front-root");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    // Candidates containing change 1's test file are INFRA-BLOCKED (the host
    // refused every measurement); everything else passes.
    let validator = r#"if [ -f tests/a ]; then echo "INFRA-BLOCKED: a"; exit 6; fi; for t in $(ls tests); do printf '{"id":"%s","outcome":"pass"}\n' "$t" >> "$MQ_RESULTS"; done; exit 0"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let out = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    // Exit 3: the run still ends on the held item's stop reason.
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    let report = latest_report(&fx);
    // The held change is neither landed nor ejected; the greens behind it
    // landed in this same run.
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "landed", "{report}");
    assert_eq!(status(&fx, 3), "landed", "{report}");
    assert!(
        report["stopped"]
            .as_str()
            .is_some_and(|stop| stop.contains("INFRA-BLOCKED")),
        "{report}"
    );
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    assert!(
        events.contains("landing blocked behind #1 (held)"),
        "{events}"
    );
}

/// A withdrawal of a change held earlier in the same run (no running
/// validation for it) applies at the next intake point while another
/// validation is still in flight, and the run continues behind it.
#[test]
fn withdrawing_a_held_change_applies_while_another_validation_is_in_flight() {
    let fx = Fixture::new("held-front-withdraw");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let started = fx.root.join("started");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    // Gated like `gated_setup`, except candidates carrying change 1's file
    // are INFRA-BLOCKED once their gate opens.
    let validator = format!(
        r#"set=$(ls tests | tr '\n' _); echo "$set" >> '{started}'
i=0; while [ ! -e '{gates}'/"$set" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
[ -e '{gates}'/"$set" ] || exit 1
if [ -f tests/a ]; then echo "INFRA-BLOCKED: a"; exit 6; fi
for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done
exit 0"#,
        started = started.display(),
        gates = gates.display()
    );
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, &validator).unwrap();
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("the window to fill", &started, |lines| lines.len() >= 3);
    // Change 1 comes back INFRA-BLOCKED and is held; the root (change 3)
    // keeps validating behind it.
    open(&gates, "a_t0_");
    let start = Instant::now();
    let mut withdrawn = false;
    // Record the withdrawal once the hold is visible on the item.
    while start.elapsed() < Duration::from_secs(30) {
        let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
        let held = items
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["pr"] == 1 && item["hold_verdict"] == "INFRA-BLOCKED");
        if held {
            let out = fx.mq_ok(&["withdraw", "1"]);
            assert!(out.contains("recorded"), "{out}");
            withdrawn = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(withdrawn, "change 1 was never held");
    // The withdrawal applies while change 3's validation is still gated:
    // change 1 must not wait for the run's end.
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if status(&fx, 1) == "withdrawn" {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        status(&fx, 1),
        "withdrawn",
        "the withdrawal must apply at the next cycle point, not the run's end"
    );
    assert_eq!(status(&fx, 3), "queued", "change 3 is still in flight");
    open(&gates, "c_t0_");
    open(&gates, "b_c_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
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
