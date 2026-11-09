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

fn config_path(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
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
        Self::start_with_depth(fx, 3)
    }

    fn start_with_depth(fx: &Fixture, depth: usize) -> Self {
        let depth = depth.to_string();
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
            .args(["--depth", &depth, "--max-batch", "1"])
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
    config["keep_stale_roots"] = serde_json::json!(true);
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

/// The plan-intake event names each planned batch's real parent: changes
/// taken in while the stack head validates are disjoint roots on the base,
/// never "on top of" the in-flight tip their candidates do not contain.
#[test]
fn plan_intake_names_disjoint_roots_on_the_base() {
    let (fx, started, gates) = gated_setup("intake-root-names");
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(3);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("change 1 to validate", &started, |lines| {
        lines.iter().any(|line| line.contains("a_t0_"))
    });
    // Two disjoint changes arrive while #1 is in flight: both plan as roots.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    wait_for("both roots to validate beside #1", &started, |lines| {
        lines.iter().any(|line| line.contains("b_t0_"))
            && lines.iter().any(|line| line.contains("c_t0_"))
    });
    // Every candidate shape the roots may rebuild in once change 1 lands.
    for set in [
        "a_t0_",
        "b_t0_",
        "c_t0_",
        "a_b_t0_",
        "a_c_t0_",
        "a_b_c_t0_",
        "b_c_t0_",
    ] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    let report = latest_report(&fx);
    for pr in [2, 3] {
        // First built as roots on the starting base; after change 1 lands
        // they may rebuild on the moved base and validate again.
        let runs = batch_validations(&report, pr);
        assert!(!runs.is_empty(), "{report}");
        assert_eq!(runs[0]["parent"], report["base_start"], "{report}");
    }
    let runs_dir = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    let planned: Vec<String> = events
        .lines()
        .filter_map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (event["kind"] == "plan-intake").then(|| event["detail"].as_str().unwrap().to_owned())
        })
        .collect();
    assert!(
        planned.iter().any(|detail| detail.contains("#2 (root on")),
        "{planned:?}"
    );
    assert!(
        planned.iter().any(|detail| detail.contains("#3 (root on")),
        "{planned:?}"
    );
    assert!(
        planned.iter().all(|detail| !detail.contains("on top of")),
        "{planned:?}"
    );
}

/// With root stacking configured, plan-intake must not forecast a root on
/// the base for batches the fill loop will stack on a prioritized head:
/// it leaves their parents decided at launch, and the actual builds stack.
#[test]
fn plan_intake_leaves_likely_stacked_roots_undecided() {
    let (fx, started, gates) = gated_setup("intake-root-stack-names");
    let config_path = fx.queue().join(".git/merge-queue/config.json");
    let mut config: Value =
        serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(3);
    config["root_stack_min_pass_percent"] = serde_json::json!(50);
    std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--prioritized"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("change 1 to validate", &started, |lines| {
        lines.iter().any(|line| line == "a_t0_")
    });
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    wait_for("both arrivals to stack beside #1", &started, |lines| {
        lines.iter().any(|line| line == "a_b_t0_") && lines.iter().any(|line| line == "a_b_c_t0_")
    });
    assert!(
        started_lines(&started)
            .iter()
            .all(|line| line != "b_t0_" && line != "c_t0_"),
        "neither arrival validated as a root: {:?}",
        started_lines(&started)
    );
    for set in ["a_t0_", "a_b_t0_", "a_b_c_t0_"] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    for pr in [1, 2, 3] {
        assert_eq!(status(&fx, pr), "landed");
    }
    let report = latest_report(&fx);
    for pr in [2, 3] {
        let runs = batch_validations(&report, pr);
        assert_eq!(runs.len(), 1, "{report}");
        assert_ne!(runs[0]["parent"], report["base_start"], "{report}");
    }
    let runs_dir = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    let events = std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap();
    let planned: Vec<String> = events
        .lines()
        .filter_map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (event["kind"] == "plan-intake").then(|| event["detail"].as_str().unwrap().to_owned())
        })
        .collect();
    assert!(
        planned
            .iter()
            .any(|detail| detail.contains("#2 (parent decided at launch)")),
        "{planned:?}"
    );
    assert!(
        planned
            .iter()
            .all(|detail| { !detail.contains("#2 (root on") && !detail.contains("#3 (root on") }),
        "{planned:?}"
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

#[test]
fn a_root_stacks_on_a_prioritized_head_when_configured() {
    // #1071: with root_stack_min_pass_percent on, the would-be root over a
    // prioritized head stacks on the stack top instead of rooting on the
    // base that head is about to move, so it validates once and lands
    // behind it. At the default 0 the same run roots (see
    // a_disjoint_root_is_cancelled_the_moment_the_head_lands).
    let (fx, started, gates) = gated_setup("disjoint-root-stacked-likely");
    let path = config_path(&fx);
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["root_stack_min_pass_percent"] = serde_json::json!(50);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--prioritized"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("the window to fill", &started, |lines| lines.len() >= 3);
    assert_eq!(
        started_lines(&started)[..3],
        ["a_t0_", "a_b_t0_", "a_b_c_t0_"],
        "change 3 composed on the stack, never as a root"
    );
    open(&gates, "a_t0_");
    open(&gates, "a_b_t0_");
    open(&gates, "a_b_c_t0_");
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
    let report = latest_report(&fx);
    let root_runs = batch_validations(&report, 3);
    assert_eq!(root_runs.len(), 1, "{report}");
    assert_ne!(root_runs[0]["parent"], report["base_start"], "{report}");
}

#[test]
fn an_after_change_is_composed_on_its_dependency_not_the_stack() {
    // Change 3 must land after change 2 (--after): it must also be
    // COMPOSED on change 2's candidate. Change 2 validates as a disjoint
    // root, so the stack top is change 1's candidate; composing change 3
    // there would validate it without its declared dependency.
    let fx = Fixture::new("after-composed-on-dependency");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validator = r#"ok=1; for f in tests/*; do [ -f "$f" ] || continue; if grep -qx FAIL "$f"; then echo "{\"id\":\"x\",\"outcome\":\"fail\"}" >> "$MQ_RESULTS"; ok=0; fi; done; [ $ok -eq 1 ]"#;
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator,
    ]);
    // Two root slots, so change 2 can validate as a disjoint root.
    let path = config_path(&fx);
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(2);
    config["keep_stale_roots"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("u", &[("tests/u", "ok\n")]);
    fx.branch("x", &[("tests/x", "ok\n")]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "u"]);
    fx.mq_ok(&["submit", "2", "--head", "x"]);
    fx.mq_ok(&["submit", "3", "--head", "a", "--after", "2"]);
    let out = fx.mq(&["run", "--json", "--depth", "3", "--max-batch", "1"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let report = latest_report(&fx);
    let x_tip = batch_validations(&report, 2)[0]["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    let a_runs = batch_validations(&report, 3);
    assert!(!a_runs.is_empty(), "{report}");
    // Every candidate change 3 validated on contains a change-2 candidate
    // (its parent is one, or it descends from one): change 2 may itself
    // have been rebuilt, so any of its candidates counts.
    let x_tips: Vec<String> = batch_validations(&report, 2)
        .iter()
        .map(|run| run["commit"].as_str().unwrap().to_owned())
        .collect();
    let is_ancestor = |ancestor: &str, tip: &str| {
        Command::new("git")
            .arg("-C")
            .arg(fx.queue())
            .args(["merge-base", "--is-ancestor", ancestor, tip])
            .output()
            .unwrap()
            .status
            .success()
    };
    for run in &a_runs {
        let tip = run["commit"].as_str().unwrap();
        assert!(
            x_tips.iter().any(|x| is_ancestor(x, tip)),
            "change 3 composed without its dependency: {report}"
        );
    }
    assert_eq!(
        a_runs[0]["parent"], x_tip,
        "change 3 first composed on its dependency's candidate: {report}"
    );
    let landed: Vec<i64> = report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|batch| {
            batch["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        landed.iter().position(|pr| *pr == 2) < landed.iter().position(|pr| *pr == 3),
        "change 3 lands after change 2: {landed:?}"
    );
}

fn latest_events(fx: &Fixture) -> String {
    let runs_dir = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs_dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    std::fs::read_to_string(dirs.pop().unwrap().join("events.jsonl")).unwrap()
}

fn wait_for_status(fx: &Fixture, pr: i64, wanted: &str) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if status(fx, pr) == wanted {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!(
        "timed out waiting for #{pr} to be {wanted}; it is {}",
        status(fx, pr)
    );
}

/// The exact stale-root shape from #1100: a slow earlier root, a stale
/// root A with dependent D stacked on it, and B stacked on fresh root C.
/// A requeues and drops only D on its own green arrival; C and B land
/// without waiting for the slow root; D's dropped green is recorded.
#[test]
fn a_stale_root_requeues_at_once_and_drops_only_its_descendants() {
    let (fx, started, gates) = gated_setup("stale-root-prompt-decisions");
    let path = config_path(&fx);
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["roots"] = serde_json::json!(3);
    config["keep_stale_roots"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("l", &[("tests/l", "ok\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.branch("s", &[("tests/s", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "l"]);
    fx.mq_ok(&["submit", "2", "--head", "c", "--after", "1"]);
    fx.mq_ok(&["submit", "3", "--head", "s"]);
    fx.mq_ok(&["submit", "4", "--head", "a"]);
    fx.mq_ok(&["submit", "5", "--head", "d", "--after", "4"]);
    fx.mq_ok(&["submit", "6", "--head", "b", "--after", "2"]);
    let mut daemon = Daemon::start_with_depth(&fx, 6);
    // B waits in pending: its transitive dependencies are in two flights
    // (L and C) until L lands, then it stacks on C.
    wait_for("the first five batches to validate", &started, |lines| {
        ["l_t0_", "c_l_t0_", "s_t0_", "a_t0_", "a_d_t0_"]
            .iter()
            .all(|want| lines.iter().any(|line| line == want))
    });
    open(&gates, "l_t0_");
    wait_for_status(&fx, 1, "landed");
    wait_for("B to stack on C after L lands", &started, |lines| {
        lines.iter().any(|line| line == "b_c_l_t0_")
    });
    open(&gates, "c_l_t0_");
    wait_for_status(&fx, 2, "landed");
    open(&gates, "a_d_t0_");
    let start = Instant::now();
    while latest_events(&fx)
        .lines()
        .filter(|line| line.contains("\"kind\":\"result\""))
        .count()
        < 3
    {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "results did not arrive"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // A now returns stale green while slow root #3 is still gated and B
    // is still validating on C: A requeues at once and drops only D.
    open(&gates, "a_t0_");
    wait_for("the stale root to rebuild", &started, |lines| {
        lines
            .iter()
            .any(|line| line.starts_with("a_") && line != "a_t0_" && line != "a_d_t0_")
    });
    assert_eq!(status(&fx, 3), "queued", "the slow root is still in flight");
    assert_eq!(status(&fx, 6), "queued", "B is still validating on C");
    open(&gates, "b_c_l_t0_");
    wait_for_status(&fx, 6, "landed");
    assert_eq!(status(&fx, 3), "queued", "B landed behind the slow root");
    open(&gates, "s_t0_");
    // Stale rebuilds can take several shapes as later batches land; open
    // every candidate that actually starts until the daemon finishes.
    let start = Instant::now();
    loop {
        for set in started_lines(&started) {
            open(&gates, &set);
        }
        if [1, 2, 3, 4, 5, 6]
            .iter()
            .all(|pr| status(&fx, *pr) == "landed")
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(90),
            "rebuilds did not land; started: {:?}",
            started_lines(&started)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    for pr in [1, 2, 3, 4, 5, 6] {
        assert_eq!(status(&fx, pr), "landed");
    }
    assert_eq!(
        started_lines(&started)
            .iter()
            .filter(|line| *line == "b_c_l_t0_")
            .count(),
        1,
        "B validated once"
    );
    let report = latest_report(&fx);
    let b_runs = batch_validations(&report, 6);
    assert_eq!(b_runs.len(), 1, "{report}");
    assert_eq!(
        b_runs[0]["parent"],
        batch_validations(&report, 2)[0]["commit"],
        "{report}"
    );
    let d_runs = batch_validations(&report, 5);
    assert!(d_runs.len() >= 2, "{report}");
    assert_eq!(
        d_runs[0]["parent"],
        batch_validations(&report, 4)[0]["commit"],
        "{report}"
    );
    let events = latest_events(&fx);
    let event_values: Vec<Value> = events
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        event_values.iter().any(|event| {
            event["kind"] == "stale-root-green" && event["detail"].as_str().unwrap().contains("#4")
        }),
        "{events}"
    );
    assert!(
        event_values.iter().any(|event| {
            event["kind"] == "flight-discarded"
                && event["detail"].as_str().unwrap().contains("#5")
                && event["detail"].as_str().unwrap().contains("green result")
        }),
        "{events}"
    );
    for pr in [2, 6] {
        assert!(
            event_values.iter().all(|event| {
                event["kind"] != "flight-discarded"
                    || !event["detail"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("#{pr} discarded"))
            }),
            "{events}"
        );
    }
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let history = |pr: i64| -> Vec<Value> {
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["pr"] == pr)
            .and_then(|item| item["history"].as_array().cloned())
            .unwrap_or_default()
    };
    assert!(
        history(5)
            .iter()
            .any(|event| event["kind"] == "flight-discarded"),
        "item #5 must record its discarded green"
    );
    assert!(
        history(6)
            .iter()
            .all(|event| event["kind"] != "flight-discarded"),
        "item #6 was never discarded"
    );
}

/// Gated setup whose validator records the MQ_BASE/MQ_PARENT it was
/// handed (and its checkout HEAD) per validation, and whose select
/// command records the base it was measured against (#1111). With
/// `roots`, that many disjoint-root slots are configured.
fn env_setup(tag: &str, roots: Option<u64>) -> (Fixture, PathBuf, PathBuf, PathBuf, String) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let base_start = fx.remote_main();
    let started = fx.root.join("started");
    let selects = fx.root.join("selects");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    let validator = format!(
        r#"set=$(ls tests | tr '\n' _); echo "$set base=$MQ_BASE parent=$MQ_PARENT head=$(git rev-parse HEAD)" >> '{started}'
i=0; while [ ! -e '{gates}'/"$set" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
[ -e '{gates}'/"$set" ] || exit 1
for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done
exit 0"#,
        started = started.display(),
        gates = gates.display()
    );
    let script = fx.root.join("validator.sh");
    std::fs::write(&script, validator).unwrap();
    let select = format!(
        "echo \"head=$MQ_HEAD base=$MQ_BASE\" >> '{}'; git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'",
        selects.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &format!("sh '{}'", script.display()),
        "--select",
        &select,
        "--policy",
        "path-routing",
    ]);
    if let Some(roots) = roots {
        let config_path = fx.queue().join(".git/merge-queue/config.json");
        let mut config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
        config["roots"] = serde_json::json!(roots);
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    }
    (fx, started, selects, gates, base_start)
}

/// One recorded validation's (base, parent, head) by its test-set name.
fn env_of(started: &Path, set: &str) -> (String, String, String) {
    let line = started_lines(started)
        .into_iter()
        .find(|line| line.starts_with(&format!("{set} ")))
        .unwrap_or_else(|| panic!("no started line for {set}: {:?}", started_lines(started)));
    let field = |name: &str| {
        line.split(' ')
            .find_map(|part| part.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("no {name} in {line:?}"))
            .to_owned()
    };
    (field("base"), field("parent"), field("head"))
}

/// A root planned after main moved mid-run is measured against the
/// base it was planned on, not the run's starting base (#1111): its
/// validator sees MQ_BASE == MQ_PARENT == the new base, and the
/// selection command is measured against the same base.
#[test]
fn a_mid_run_root_is_measured_against_the_base_it_was_planned_on() {
    let (fx, started, selects, gates, base_start) = env_setup("mid-run-root-base", Some(3));
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("both roots to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_t0_ "))
            && lines.iter().any(|line| line.starts_with("b_t0_ "))
    });
    let (base, parent, _) = env_of(&started, "a_t0_");
    assert_eq!(
        (base.as_str(), parent.as_str()),
        (base_start.as_str(), base_start.as_str())
    );
    // Change 1 lands alone; main moves while change 2 is in flight.
    open(&gates, "a_t0_");
    let start = Instant::now();
    while status(&fx, 1) != "landed" {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "change 1 never landed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let new_base = fx.remote_main();
    assert_ne!(new_base, base_start);
    // A new root planned now stands on the moved base, and must be
    // measured against it, not against where the run started.
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    // Its candidate tree carries the landed change 1 as well.
    wait_for("the mid-run root to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_c_t0_ "))
    });
    let (base, parent, head3) = env_of(&started, "a_c_t0_");
    assert_eq!(parent, new_base, "started: {:?}", started_lines(&started));
    assert_eq!(base, new_base, "started: {:?}", started_lines(&started));
    let select_lines = std::fs::read_to_string(&selects).unwrap_or_default();
    assert!(
        select_lines
            .lines()
            .any(|line| line == format!("head={head3} base={new_base}")),
        "selection not measured against the new base: {select_lines}"
    );
    for set in [
        "a_t0_",
        "b_t0_",
        "c_t0_",
        "a_b_t0_",
        "a_c_t0_",
        "b_c_t0_",
        "a_b_c_t0_",
        "t0_",
    ] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    for pr in 1..=3 {
        assert_eq!(status(&fx, pr), "landed", "#{pr}");
    }
}

/// A stacked batch keeps today's bases (#1111 does not move it):
/// MQ_BASE is the base its stack stands on — here the run's starting
/// base — and MQ_PARENT is the candidate beneath it.
#[test]
fn a_stacked_batch_keeps_the_stack_bottom_as_its_base() {
    let (fx, started, _selects, gates, base_start) = env_setup("stacked-base", None);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("change 1 to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_t0_ "))
    });
    let (_, _, head1) = env_of(&started, "a_t0_");
    fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--after", "1"]);
    wait_for("the stacked batch to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_b_t0_ "))
    });
    let (base, parent, _) = env_of(&started, "a_b_t0_");
    assert_eq!(base, base_start, "started: {:?}", started_lines(&started));
    assert_eq!(parent, head1, "started: {:?}", started_lines(&started));
    for set in ["a_t0_", "b_t0_", "a_b_t0_", "t0_"] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
}

/// Root stacking judges the batch the new build depends on (#1127):
/// with a non-likely stacked flight in the window and a prioritized
/// root beside it, a new would-be root roots on the base — a likely
/// root outside the stack never justifies stacking on the stack top.
#[test]
fn a_likely_root_outside_the_stack_does_not_make_the_new_root_stack() {
    let (fx, started, _selects, gates, base_start) = env_setup("root-stack-mismatch", Some(3));
    let path = config_path(&fx);
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["root_stack_min_pass_percent"] = serde_json::json!(70);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    // Depth 4: a window slot stays free for the new root's decision.
    let mut daemon = Daemon::start_with_depth(&fx, 4);
    wait_for("change 1 to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_t0_ "))
    });
    // A plain stacked flight on change 1: the stack top, not likely
    // (gated, plain, predicted 0.5 below the 70 threshold). The
    // --after dependency composes it on change 1's candidate.
    fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--after", "1"]);
    wait_for("change 2 to stack on change 1", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_b_t0_ "))
    });
    // A prioritized root beside the stack: likely, but outside it.
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c", "--prioritized"]);
    wait_for("the prioritized root to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("c_t0_ "))
    });
    let (base, parent, _) = env_of(&started, "c_t0_");
    assert_eq!(
        (base.as_str(), parent.as_str()),
        (base_start.as_str(), base_start.as_str())
    );
    // The new would-be root must root on the base too, not stack on
    // the unjudged stack top.
    fx.branch("d", &[("tests/d", "ok\n")]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    wait_for("change 4 to validate", &started, |lines| {
        lines
            .iter()
            .any(|line| line.split(' ').next().is_some_and(|set| set.contains("d_")))
    });
    let sets: Vec<String> = started_lines(&started)
        .iter()
        .map(|line| line.split(' ').next().unwrap().to_owned())
        .collect();
    assert!(
        sets.iter().any(|set| set == "d_t0_"),
        "change 4 did not root on the base: {sets:?} in {:?}",
        started_lines(&started)
    );
    let (base, parent, _) = env_of(&started, "d_t0_");
    assert_eq!(parent, base_start, "started: {:?}", started_lines(&started));
    assert_eq!(base, base_start, "started: {:?}", started_lines(&started));
    let events = latest_events(&fx);
    assert!(
        events.contains("\"kind\":\"disjoint-root\""),
        "no disjoint-root event: {events}"
    );
    assert!(
        !events.contains("disjoint-root-stacked"),
        "change 4 stacked on the unjudged top: {events}"
    );
    for set in [
        "a_t0_",
        "b_t0_",
        "c_t0_",
        "d_t0_",
        "a_b_t0_",
        "a_c_t0_",
        "a_d_t0_",
        "b_c_t0_",
        "b_d_t0_",
        "c_d_t0_",
        "a_b_c_t0_",
        "a_b_d_t0_",
        "a_c_d_t0_",
        "b_c_d_t0_",
        "a_b_c_d_t0_",
        "t0_",
    ] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    for pr in 1..=4 {
        assert_eq!(status(&fx, pr), "landed", "#{pr}");
    }
    let report = latest_report(&fx);
    let runs = batch_validations(&report, 4);
    // The build this test judges is the first: a disjoint root on the
    // base. (It may validate again stacked once the roots ahead of it
    // have landed and rebuilt; that is ordinary root mechanics.)
    assert!(!runs.is_empty(), "{report}");
    assert_eq!(runs[0]["parent"], report["base_start"], "{report}");
}

/// When the stack top itself is likely, the new batch still stacks
/// on it (#1127): the likely batch and the parent are the same
/// batch, and it validates after the top.
#[test]
fn a_likely_stack_top_still_stacks() {
    let (fx, started, _selects, gates, base_start) = env_setup("root-stack-top-likely", Some(3));
    let path = config_path(&fx);
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["root_stack_min_pass_percent"] = serde_json::json!(70);
    std::fs::write(&path, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("change 1 to validate", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_t0_ "))
    });
    let (_, _, head1) = env_of(&started, "a_t0_");
    // The stack top is prioritized: itself likely.
    fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b", "--prioritized"]);
    wait_for("change 2 to stack on change 1", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_b_t0_ "))
    });
    // The new batch stacks on that top.
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    wait_for("change 3 to stack on the likely top", &started, |lines| {
        lines.iter().any(|line| line.starts_with("a_b_c_t0_ "))
    });
    let (base, parent, _) = env_of(&started, "a_b_c_t0_");
    assert_eq!(base, base_start, "started: {:?}", started_lines(&started));
    assert_ne!(parent, head1, "started: {:?}", started_lines(&started));
    let events = latest_events(&fx);
    assert!(
        events.contains("stacks on #2 (likely to land first: #2"),
        "the event must name the top as both: {events}"
    );
    assert!(
        !started_lines(&started)
            .iter()
            .any(|line| line.starts_with("c_t0_ ")),
        "change 3 rooted instead of stacking: {:?}",
        started_lines(&started)
    );
    for set in ["a_t0_", "a_b_t0_", "a_b_c_t0_", "t0_"] {
        open(&gates, set);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(120)), Some(0));
    for pr in 1..=3 {
        assert_eq!(status(&fx, pr), "landed", "#{pr}");
    }
    let report = latest_report(&fx);
    let runs = batch_validations(&report, 3);
    assert_eq!(runs.len(), 1, "{report}");
    assert_ne!(runs[0]["parent"], report["base_start"], "{report}");
}
