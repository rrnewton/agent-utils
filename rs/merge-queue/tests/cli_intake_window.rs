//! A run takes in requests recorded while it holds the queue as soon as its speculative window
//! has a free slot, instead of at its end: a change submitted while a batch validates is planned
//! on top of that batch and validates beside it, and still lands only after it. A batch beneath
//! that fails rebuilds the new change without it, and a change stacked on the failed one is set
//! aside with it.

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

/// A queue whose validations each append the candidate's test files to `started`, wait (at
/// most 60 s) for the gate named after how many test files the candidate has, and then fail
/// exactly when a test file says FAIL. Returns the fixture, `started` and the gates directory.
fn setup(tag: &str) -> (Fixture, PathBuf, PathBuf) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let started = fx.root.join("started");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    let validator = format!(
        r#"echo $(ls tests) >> '{started}'; gate='{gates}'/$(ls tests | wc -l | tr -d ' ')
i=0; while [ ! -e "$gate" ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done; [ -e "$gate" ] || exit 1
rc=0
for t in $(ls tests); do
  if grep -q FAIL "tests/$t"; then o=fail; rc=1; else o=pass; fi
  printf '{{"id":"%s","outcome":"%s"}}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#,
        started = started.display(),
        gates = gates.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
    ]);
    (fx, started, gates)
}

/// Open the gate of candidates with `files` test files.
fn open(gates: &Path, files: usize) {
    std::fs::write(gates.join(files.to_string()), "").unwrap();
}

struct Daemon(Child);

impl Daemon {
    /// One run (`--once`), two batches of one change each in flight at most.
    fn start(fx: &Fixture) -> Self {
        Self::start_with(fx, None)
    }

    // As `start`, with the run's intake window shortened to `secs` seconds
    // (`--intake-for-secs`) so a test can get past it.
    fn start_with_intake_for(fx: &Fixture, secs: u64) -> Self {
        Self::start_with(fx, Some(secs))
    }

    fn start_with(fx: &Fixture, intake_for_secs: Option<u64>) -> Self {
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
            .args(["--depth", "2", "--max-batch", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(secs) = intake_for_secs {
            command.args(["--intake-for-secs", &secs.to_string()]);
        }
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

/// The test files of each validation started so far, in order.
fn started_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
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

/// The reports of every run, oldest first.
fn events(fx: &Fixture) -> Vec<String> {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("events.jsonl").is_file())
        .collect();
    dirs.sort();
    dirs.iter()
        .flat_map(|dir| {
            std::fs::read_to_string(dir.join("events.jsonl"))
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn reports(fx: &Fixture) -> Vec<Value> {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("report.json").is_file())
        .collect();
    dirs.sort();
    dirs.iter()
        .map(|dir| {
            serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap())
                .unwrap()
        })
        .collect()
}

fn landed(report: &Value) -> Vec<Vec<i64>> {
    report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| {
            batch["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect()
        })
        .collect()
}

/// Start the daemon with change 1 (`first`) queued, wait for its validation, then submit
/// change 2 (`second`, pushed by `push`) while it validates and wait until change 2 validates
/// too: two validations in flight, neither finished. Returns the daemon.
fn submit_while_validating(
    fx: &Fixture,
    started: &Path,
    second: &[&str],
    push: impl FnOnce(),
) -> Daemon {
    let daemon = Daemon::start(fx);
    wait_for("change 1 to validate", Duration::from_secs(30), || {
        started_lines(started).len() == 1
    });
    push();
    let mut args = vec!["submit", "2"];
    args.extend_from_slice(second);
    let out = fx.mq_ok(&args);
    // The run holds the writer lock, so the request is recorded rather than applied.
    assert!(out.contains("recorded #2"), "{out}");
    wait_for(
        "change 2 to validate beside change 1",
        Duration::from_secs(30),
        || started_lines(started).len() == 2,
    );
    daemon
}

#[test]
fn a_change_submitted_while_a_batch_validates_starts_in_the_free_slot_and_lands_after_it() {
    let (fx, started, gates) = setup("intake-window");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = submit_while_validating(&fx, &started, &["--head", "b"], || {
        fx.branch("b", &[("tests/b", "ok\n")]);
    });
    // Change 2 was built on top of change 1's candidate, which it carries.
    assert_eq!(
        started_lines(&started),
        vec!["a t0".to_owned(), "a b t0".to_owned()]
    );
    // Change 2's validation passes first; it still lands only after change 1's.
    open(&gates, 3);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "queued");
    open(&gates, 2);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    // One run did both: the daemon ran once.
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = &reports[0];
    assert_eq!(landed(report), vec![vec![1], vec![2]], "{report}");
    assert!(report["stopped"].is_null(), "{report}");
    let intake = report["intake"].as_array().unwrap();
    assert_eq!(intake.len(), 1, "{report}");
    assert!(
        intake[0].as_str().unwrap().starts_with("applied submit #2"),
        "{report}"
    );
    assert_eq!(report["windows"], serde_json::json!([2, 2]), "{report}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    // Landed in order, each validated tree exactly.
    let subjects = git(&fx.remote(), &["log", "--format=%s", "main"]);
    let a = subjects.find("a: change").expect("change 1 landed");
    let b = subjects.find("b: change").expect("change 2 landed");
    assert!(b < a, "change 2 must land after change 1:\n{subjects}");
    assert_eq!(started_lines(&started).len(), 2);
}

#[test]
fn a_change_taken_in_on_top_of_a_failing_batch_is_rebuilt_without_it() {
    let (fx, started, gates) = setup("intake-window-red");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = submit_while_validating(&fx, &started, &["--head", "b"], || {
        fx.branch("b", &[("tests/b", "ok\n")]);
    });
    assert_eq!(
        started_lines(&started),
        vec!["a t0".to_owned(), "a b t0".to_owned()]
    );
    // Every gate, the base's (one test file) included: the culprit search probes it.
    for files in 1..=3 {
        open(&gates, files);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    assert_eq!(status(&fx, 1), "ejected");
    assert_eq!(status(&fx, 2), "landed");
    // Change 2 was validated again on the base, without change 1, and that tree landed.
    assert!(
        started_lines(&started).contains(&"b t0".to_owned()),
        "{:?}",
        started_lines(&started)
    );
    let files = git(&fx.remote(), &["ls-tree", "--name-only", "main", "tests/"]);
    assert_eq!(files, "tests/b\ntests/t0", "{files}");
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(landed(&reports[0]), vec![vec![2]], "{}", reports[0]);
}

#[test]
fn a_change_stacked_on_a_failing_batch_in_flight_is_set_aside_with_it() {
    let (fx, started, gates) = setup("intake-window-stacked");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon =
        submit_while_validating(&fx, &started, &["--head", "b", "--after", "1"], || {
            fx.branch_from("b", "a", &[("tests/b", "ok\n")]);
        });
    assert_eq!(
        started_lines(&started),
        vec!["a t0".to_owned(), "a b t0".to_owned()]
    );
    for files in 1..=3 {
        open(&gates, files);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    assert_eq!(status(&fx, 1), "ejected");
    // Change 2 carries change 1's commit: rebuilt without change 1 it would land it anyway.
    // It is set aside instead, and never validated again.
    assert_eq!(status(&fx, 2), "queued");
    let lines = started_lines(&started);
    assert_eq!(
        lines.iter().filter(|line| line.contains('b')).count(),
        1,
        "{lines:?}"
    );
    let files = git(&fx.remote(), &["ls-tree", "--name-only", "main", "tests/"]);
    assert_eq!(files, "tests/t0", "{files}");
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        reports[0]["requeued"],
        serde_json::json!([2]),
        "{}",
        reports[0]
    );
}

#[test]
fn a_change_a_failing_batch_sends_back_is_planned_again_with_a_request_taken_in_after_it() {
    let (fx, started, gates) = setup("intake-window-replan");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = submit_while_validating(&fx, &started, &["--head", "b"], || {
        fx.branch("b", &[("tests/b", "ok\n")]);
    });
    // The window is full (two batches in flight): change 3 waits, recorded.
    fx.branch("c", &[("tests/c", "ok\n")]);
    let out = fx.mq_ok(&["submit", "3", "--head", "c"]);
    assert!(out.contains("recorded #3"), "{out}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(started_lines(&started).len(), 2);
    // Change 1 fails: change 2's batch, built on it, goes back to be built again, and the plan
    // that takes change 3 in must still hold change 2.
    for files in 1..=4 {
        open(&gates, files);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    assert_eq!(status(&fx, 1), "ejected");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "landed");
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        landed(&reports[0]),
        vec![vec![2], vec![3]],
        "{}",
        reports[0]
    );
    let files = git(&fx.remote(), &["ls-tree", "--name-only", "main", "tests/"]);
    assert_eq!(files, "tests/b\ntests/c\ntests/t0", "{files}");
}

/// An ejected change resubmitted at a NEW head while another batch is still
/// in flight applies at the run's next intake point: the stale exclusion
/// for the old head is dropped, the new head is planned on top of the
/// in-flight batch, and it lands in the same run.
#[test]
fn an_ejected_change_resubmitted_at_a_new_head_is_validated_in_the_same_run() {
    // The waits below are liveness bounds on event-based oracles, not
    // relative-timing assertions: the whole test takes ~3s solo, but under
    // full-gate CPU contention one daemon cycle chain (probe, plan, intake,
    // validation spawns) has been observed taking over the old 60s bound.
    // 240s is ~4x that observed worst case; the sequencing itself is gated
    // on statuses and validator starts throughout.
    let (fx, started, gates) = setup("intake-resubmit-new-head");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("both batches to validate", Duration::from_secs(240), || {
        started_lines(&started).len() == 2
    });
    // Change 1 fails and is ejected; change 2 (stacked on it) stays gated.
    open(&gates, 1);
    open(&gates, 2);
    wait_for("change 1 to be ejected", Duration::from_secs(240), || {
        status(&fx, 1) == "ejected"
    });
    // Resubmit change 1 at a fixed head while change 2 is still in flight.
    fx.branch("a2", &[("tests/a", "ok\n")]);
    let out = fx.mq_ok(&["submit", "1", "--head", "a2"]);
    assert!(out.contains("recorded #1"), "{out}");
    // It validates again beside change 2 in this same run.
    wait_for(
        "change 1 to validate at its new head",
        Duration::from_secs(240),
        || started_lines(&started).len() >= 3,
    );
    open(&gates, 3);
    assert_eq!(daemon.exit_code(Duration::from_secs(240)), Some(0));
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    let intake = reports[0]["intake"].as_array().unwrap();
    assert!(
        intake
            .iter()
            .any(|line| line.as_str().unwrap().starts_with("applied submit #1")),
        "{intake:?}"
    );
}

/// A resubmission at the SAME head a run ejected is taken in once per
/// run (#1050): the operator who resubmits an ejected head has usually
/// fixed the validator, not the change, so the run plans it again
/// instead of making them wait for the run to end. The ejection stays
/// on record.
#[test]
fn a_resubmission_at_the_ejected_head_is_taken_in_once_in_the_same_run() {
    let (fx, started, gates) = setup("intake-resubmit-same-head");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    // Two files, so change 2's rebuilt candidate (3 files) has a gate of
    // its own, distinct from change 1's (2 files).
    fx.branch("b", &[("tests/b", "ok\n"), ("tests/b2", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("both batches to validate", Duration::from_secs(30), || {
        started_lines(&started).len() == 2
    });
    open(&gates, 1);
    open(&gates, 2);
    wait_for("change 1 to be ejected", Duration::from_secs(60), || {
        status(&fx, 1) == "ejected"
    });
    // Change 2, stacked on the ejected batch, is rebuilt without it and
    // validates again; wait for that rebuild so the count is stable.
    wait_for(
        "change 2 to be rebuilt without change 1",
        Duration::from_secs(60),
        || started_lines(&started).iter().any(|line| line == "b b2 t0"),
    );
    let out = fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(out.contains("recorded #1"), "{out}");
    // The same-head resubmission validates change 1 again in this run,
    // stacked on change 2's rebuilt batch (3 test files, gate already
    // open), so it fails the same way at once. The initial stacked batch
    // produced the same started line and the first ejection left the
    // status at ejected, so both waits count second occurrences.
    wait_for(
        "change 1 to validate again at the same head",
        Duration::from_secs(60),
        || {
            started_lines(&started)
                .iter()
                .filter(|line| *line == "a b b2 t0")
                .count()
                >= 2
        },
    );
    // Its candidate has four test files; open that gate so it fails the
    // same way. A second same-head resubmission, recorded while that
    // revalidation is still in flight, is not taken in again this run:
    // the one-per-run allowance is already spent.
    open(&gates, 4);
    let out = fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(out.contains("recorded #1"), "{out}");
    // No further validation of change 1 starts while the run goes on:
    // the one-per-run allowance is spent. Change 2 is still gated, so
    // the run is alive and could take change 1 in if the rule allowed.
    let count_a = |lines: &[String]| {
        lines
            .iter()
            .filter(|line| line.split(' ').any(|f| f == "a"))
            .count()
    };
    let before = count_a(&started_lines(&started));
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        count_a(&started_lines(&started)),
        before,
        "started: {:?}\nevents: {:?}",
        started_lines(&started),
        events(&fx)
    );
    // The second resubmission is recorded for the next run: the change
    // is queued again, but this run does not plan it.
    assert_eq!(status(&fx, 1), "queued");
    open(&gates, 3);
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    // The revalidation failed the same way and was ejected again.
    assert!(
        events(&fx)
            .iter()
            .filter(|line| line.contains("\"kind\":\"eject\""))
            .count()
            >= 2,
        "events: {:?}",
        events(&fx)
    );
    // The second resubmission applies at the next run: queued again.
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "landed");
    // The ejection and the same-run revalidation are both on record.
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert!(
        !reports[0]["ejected"].as_array().unwrap().is_empty(),
        "{reports:?}"
    );
}

/// Late resubmission while one long batch drains (#1087): past its intake
/// window a run normally takes no recorded requests in, however long its
/// last batch runs. When exactly one batch is left in flight it takes the
/// recorded requests in once more, planned on top of that batch, and then
/// no more: a further submission waits for the next run even when the
/// window has a free slot again.
#[test]
fn a_late_resubmission_while_one_long_batch_drains_is_taken_in_once_more() {
    let (fx, started, gates) = setup("intake-window-late");
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    // Two files, so change 2's rebuilt candidate (3 files) has a gate of
    // its own, distinct from change 1's (2 files).
    fx.branch("b", &[("tests/b", "ok\n"), ("tests/b2", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    // A six-second intake window: the run is past it long before the
    // gated batches below finish.
    let mut daemon = Daemon::start_with_intake_for(&fx, 6);
    wait_for("both batches to validate", Duration::from_secs(30), || {
        started_lines(&started).len() == 2
    });
    // The run's intake clock started before its first validation did, so
    // nine seconds after validations are seen running the run is past
    // its six-second window for sure.
    let validating_at = Instant::now();
    // Change 1 fails and is ejected; change 2 (stacked on it) stays gated.
    open(&gates, 1);
    open(&gates, 2);
    wait_for("change 1 to be ejected", Duration::from_secs(60), || {
        status(&fx, 1) == "ejected"
    });
    // Change 2, stacked on the ejected batch, is rebuilt without it and
    // validates again: exactly one batch now left in flight, still gated.
    wait_for(
        "change 2 to be rebuilt without change 1",
        Duration::from_secs(60),
        || started_lines(&started).iter().any(|line| line == "b b2 t0"),
    );
    while validating_at.elapsed() < Duration::from_secs(9) {
        std::thread::sleep(Duration::from_millis(100));
    }
    // Resubmit change 1 at a fixed head, past the window, while change 2
    // drains alone: the one extra intake round plans it on top of
    // change 2's batch in this same run. The initial stacked batch
    // produced the same started line, so the wait counts second
    // occurrences.
    fx.branch("a2", &[("tests/a", "ok\n")]);
    let out = fx.mq_ok(&["submit", "1", "--head", "a2"]);
    assert!(out.contains("recorded #1"), "{out}");
    wait_for(
        "change 1 to validate again past the intake window",
        Duration::from_secs(60),
        || {
            started_lines(&started)
                .iter()
                .filter(|line| *line == "a b b2 t0")
                .count()
                >= 2
        },
    );
    assert!(
        events(&fx)
            .iter()
            .any(|line| line.contains("taking recorded requests in once more")),
        "events: {:?}",
        events(&fx)
    );
    // A second submission after that one extra round is not taken in.
    fx.branch("c", &[("tests/c", "ok\n")]);
    let out = fx.mq_ok(&["submit", "3", "--head", "c"]);
    assert!(out.contains("recorded #3"), "{out}");
    // Change 2 lands; change 1's late batch is still gated, so exactly
    // one batch is in flight again with a free slot beside it. Change 3
    // still does not start: the one extra round is spent.
    open(&gates, 3);
    wait_for("change 2 to land", Duration::from_secs(60), || {
        status(&fx, 2) == "landed"
    });
    let count_c = |lines: &[String]| {
        lines
            .iter()
            .filter(|line| line.split(' ').any(|f| f == "c"))
            .count()
    };
    let before = started_lines(&started);
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(
        count_c(&started_lines(&started)),
        count_c(&before),
        "started: {:?}\nevents: {:?}",
        started_lines(&started),
        events(&fx)
    );
    assert_eq!(count_c(&started_lines(&started)), 0);
    assert_eq!(status(&fx, 1), "queued");
    // The late batch finishes and lands after change 2; change 3 was
    // applied at the run's end and waits queued for the next run.
    open(&gates, 4);
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(
        landed(&reports[0]),
        vec![vec![2], vec![1]],
        "{}",
        reports[0]
    );
    let intake = reports[0]["intake"].as_array().unwrap();
    assert!(
        intake
            .iter()
            .any(|line| line.as_str().unwrap().starts_with("applied submit #1")),
        "{intake:?}"
    );
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "landed");
    assert_eq!(status(&fx, 3), "queued");
    assert_eq!(count_c(&started_lines(&started)), 0);
}

/// A resubmission recorded while its change is in flight is dropped
/// when the change lands (#1103), not applied afterwards: the landed
/// change is neither queued nor validated again, in this run or at
/// its end.
#[test]
fn a_resubmission_recorded_in_flight_is_dropped_when_its_change_lands() {
    let (fx, started, gates) = setup("intake-landed-resubmit-drop");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("change 1 to validate", Duration::from_secs(30), || {
        started_lines(&started).len() == 1
    });
    // The same head submitted again while it validates: recorded, and
    // still waiting when the batch lands.
    let out = fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert!(out.contains("recorded #1"), "{out}");
    open(&gates, 2);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(landed(&reports[0]), vec![vec![1]], "{}", reports[0]);
    assert_eq!(status(&fx, 1), "landed");
    // Validated once: the recorded resubmission never started another.
    assert_eq!(started_lines(&started), vec!["a t0".to_owned()]);
    assert!(
        events(&fx)
            .iter()
            .any(|line| line.contains("dropped 1 recorded request")),
        "events: {:?}",
        events(&fx)
    );
}

/// Prioritizing a change whose batch is in flight (#1104) applies at
/// the run's next intake point without disturbing the batch, and the
/// next plan in the same run orders by the new priority: a pending
/// change prioritized mid-run is validated ahead of an earlier one.
#[test]
fn prioritizing_in_flight_applies_at_the_next_intake_point_and_reorders_the_plan() {
    let (fx, started, gates) = setup("intake-prioritize-in-flight");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    // Two files of its own, so change 4's candidates have gate counts
    // of their own wherever they are planned.
    fx.branch("d", &[("tests/d", "ok\n"), ("tests/d2", "ok\n")]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    let mut daemon = Daemon::start(&fx);
    wait_for("two batches to validate", Duration::from_secs(30), || {
        started_lines(&started).len() == 2
    });
    let prioritized = |pr: i64| {
        let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
        items
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["pr"] == pr && item["status"] != "pending-intake")
            .is_some_and(|item| item["prioritized"] == true)
    };
    // Change 2 is in flight: its prioritization is recorded, then
    // applied in this run while its batch keeps validating.
    let out = fx.mq_ok(&["prioritize", "2"]);
    assert!(out.contains("recorded the prioritization of #2"), "{out}");
    wait_for(
        "change 2 to show prioritized while in flight",
        Duration::from_secs(30),
        || prioritized(2),
    );
    // The batches in flight are undisturbed: nothing restarted.
    assert_eq!(started_lines(&started).len(), 2);
    // Change 4 jumps the pending queue ahead of change 3.
    let out = fx.mq_ok(&["prioritize", "4"]);
    assert!(out.contains("recorded the prioritization of #4"), "{out}");
    wait_for(
        "change 4 to show prioritized",
        Duration::from_secs(30),
        || prioritized(4),
    );
    assert_eq!(started_lines(&started).len(), 2);
    // Change 1 lands; the freed slot takes the next planned batch,
    // which is now change 4's, not change 3's.
    open(&gates, 2);
    wait_for(
        "a third validation to start",
        Duration::from_secs(60),
        || started_lines(&started).len() == 3,
    );
    let third = started_lines(&started)[2].clone();
    assert!(
        third.split(' ').any(|f| f == "d") && !third.split(' ').any(|f| f == "c"),
        "the prioritized change did not go first: {third:?} in {:?}",
        started_lines(&started)
    );
    for files in 1..=6 {
        open(&gates, files);
    }
    assert_eq!(daemon.exit_code(Duration::from_secs(90)), Some(0));
    for pr in 1..=4 {
        assert_eq!(status(&fx, pr), "landed", "#{pr}");
    }
    let reports = reports(&fx);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let intake = reports[0]["intake"].as_array().unwrap();
    for pr in [2, 4] {
        assert!(
            intake.iter().any(|line| line
                .as_str()
                .unwrap()
                .starts_with(&format!("applied prioritize #{pr}"))),
            "{intake:?}"
        );
    }
}
