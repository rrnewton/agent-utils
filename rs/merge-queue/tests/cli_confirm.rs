//! Same-commit confirmation is off unless a queue opts in (`init --confirm-reruns`): the first
//! red is the verdict. A red change on its own is ejected on that one sighting, and is never
//! validated again at the same commit. A red batch of several changes is not searched: each
//! change goes back, in order, as a batch of its own, and stays alone in every later plan of the
//! run, requests taken in included.

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

/// Each file under `tests/` is one test, failing when it contains FAIL; a selection of tests
/// runs only those. Every run writes a handle, as a runner that can rerun its own failures does,
/// and appends the `$MQ_RERUN_FROM` it is handed, if any, to `$RERUNS`.
const RUNNER: &str = r#"echo "run-$$" > "$MQ_HANDLE_FILE"
[ -n "$MQ_RERUN_FROM" ] && echo "$MQ_RERUN_FROM" >> "$RERUNS"
if [ "$MQ_SELECTION" = tests ]; then set -- $MQ_TESTS; else set -- $(ls tests); fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

const UNCONFIRMED: &str = "(seen once: same-commit confirmation is off (validator.confirm_reruns))";

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

/// `run --json` with `$RERUNS` set; the report and the `$RERUNS` path.
fn run(fx: &Fixture) -> (Value, PathBuf) {
    let reruns = fx.root.join("reruns");
    let out = fx.mq_env(&["run", "--json"], &[("RERUNS", reruns.to_str().unwrap())]);
    let report = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|e| {
        panic!(
            "bad report JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (report, reruns)
}

fn purposes(report: &Value) -> Vec<String> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["purpose"].as_str().unwrap().to_owned())
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

/// The `(kind, detail)` of every event of run `run_id`.
fn events(fx: &Fixture, run_id: &str) -> Vec<(String, String)> {
    let path = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(run_id)
        .join("events.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .lines()
        .map(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            (
                event["kind"].as_str().unwrap().to_owned(),
                event["detail"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn config_validator(fx: &Fixture) -> Value {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    let text = std::fs::read_to_string(common.join("merge-queue").join("config.json")).unwrap();
    serde_json::from_str::<Value>(&text).unwrap()["validator"].clone()
}

#[test]
fn a_red_change_is_ejected_on_its_first_sighting_without_a_same_commit_rerun() {
    let fx = setup("confirm-off-single", RUNNER, &["--max-batch", "1"]);
    // Off is the default, and the key is not even written.
    assert!(config_validator(&fx).get("confirm_reruns").is_none());
    let base = fx.remote_main();
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let (r, reruns) = run(&fx);
    // The batch, then the bare base on the failed test, so that a broken base never ejects an
    // innocent change. The red commit itself is never validated again.
    assert_eq!(purposes(&r), vec!["batch", "probe"], "{r}");
    assert_eq!(r["validations"][1]["commit"], base.as_str(), "{r}");
    assert!(!reruns.exists(), "no run was handed MQ_RERUN_FROM");
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 1, "{r}");
    assert_eq!(ejected["failed"], serde_json::json!(["a"]));
    let reason = ejected["reason"].as_str().unwrap();
    assert!(reason.contains(UNCONFIRMED), "{reason}");
    assert_eq!(status(&fx, 1), "ejected");
}

#[test]
fn a_red_batch_goes_back_change_by_change_and_only_the_culprit_is_ejected() {
    let fx = setup(
        "confirm-off-split",
        RUNNER,
        &["--max-batch", "3", "--trust-base"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let (r, reruns) = run(&fx);
    assert_eq!(landed(&r), vec![vec![1], vec![3]], "{r}");
    let ejected = r["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{r}");
    assert_eq!(ejected[0]["pr"], 2, "{r}");
    assert!(
        ejected[0]["reason"].as_str().unwrap().contains(UNCONFIRMED),
        "{r}"
    );
    // The red batch is not searched: no probe and no remainder, only batches, the first of them
    // the red one with all three changes.
    assert_eq!(r["validations"][0]["prs"], serde_json::json!([1, 2, 3]));
    assert!(purposes(&r).iter().all(|purpose| purpose == "batch"), "{r}");
    assert!(!reruns.exists(), "no run was handed MQ_RERUN_FROM");
    let split: Vec<String> = events(&fx, r["run"].as_str().unwrap())
        .into_iter()
        .filter(|(kind, _)| kind == "split")
        .map(|(_, detail)| detail)
        .collect();
    assert_eq!(
        split,
        vec!["v1 #1, #2, #3 red (b); each change goes back as a batch of its own, in order"],
        "{r}"
    );
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 3), "landed");
}

struct Daemon(Child);

impl Daemon {
    /// One run (`--once`): two batches in flight at first, up to three changes each.
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
            .args(["--depth", "2", "--max-batch", "3"])
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

fn started_count(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .count()
}

#[test]
fn a_split_change_stays_alone_when_a_request_is_taken_in() {
    // Each validation records that it started, then waits (at most 60 s) for the gate named
    // after how many test files its candidate has: every candidate below has its own count
    // while it matters.
    let fx = Fixture::new("confirm-off-split-intake");
    let started = fx.root.join("started");
    let gates = fx.root.join("gates");
    std::fs::create_dir_all(&gates).unwrap();
    let validator = format!(
        r#"echo x >> '{started}'; gate='{gates}'/$(ls tests | wc -l | tr -d ' ')
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
        &validator,
        "--trust-base",
    ]);
    let open = |files: usize| std::fs::write(gates.join(files.to_string()), "").unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let mut daemon = Daemon::start(&fx);
    // #1, #2, #3 together (4 test files) is red: each goes back alone. #1 (2 files) and #2 on
    // top of it (3 files) validate side by side; #3 waits.
    wait_for("the batch's validation", Duration::from_secs(30), || {
        started_count(&started) == 1
    });
    open(4);
    wait_for("#1 and #2 to validate", Duration::from_secs(30), || {
        started_count(&started) == 3
    });
    fx.branch("d", &[("tests/d", "ok\n")]);
    let out = fx.mq_ok(&["submit", "4", "--head", "d"]);
    assert!(out.contains("recorded #4"), "{out}");
    // #1 lands and frees a slot: the run takes #4 in and plans #3 and #4 again. #3 stays alone.
    open(2);
    wait_for("#4 to be planned", Duration::from_secs(30), || {
        status(&fx, 4) == "queued"
    });
    open(3);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 3), "landed");
    assert_eq!(status(&fx, 4), "landed");
    let runs = fx.queue().join(".git/merge-queue/runs");
    let run_id = std::fs::read_dir(&runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .max()
        .unwrap();
    let events = events(&fx, &run_id);
    let planned: Vec<&String> = events
        .iter()
        .filter(|(kind, _)| kind == "plan-intake")
        .map(|(_, detail)| detail)
        .collect();
    assert!(
        !planned.is_empty() && planned.iter().all(|detail| detail.contains("#3 then #4")),
        "{planned:?}"
    );
    assert!(
        events
            .iter()
            .all(|(kind, detail)| kind != "validate" || !detail.contains("#3, #4")),
        "{events:?}"
    );
}

/// Like [`RUNNER`], and a test file that says FLAKY fails unless the run is a same-commit rerun.
const FLAKY_RUNNER: &str = r#"echo "run-$$" > "$MQ_HANDLE_FILE"
[ -n "$MQ_RERUN_FROM" ] && echo "$MQ_RERUN_FROM" >> "$RERUNS"
if [ "$MQ_SELECTION" = tests ]; then set -- $MQ_TESTS; else set -- $(ls tests); fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  if [ -f "tests/$t" ] && grep -q FLAKY "tests/$t" && [ -z "$MQ_RERUN_FROM" ]; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
exit $rc"#;

#[test]
fn split_red_batches_splits_a_red_batch_and_confirms_each_single_before_ejecting_it() {
    let fx = setup(
        "confirm-split",
        FLAKY_RUNNER,
        &[
            "--confirm-reruns",
            "--split-red-batches",
            "--max-batch",
            "3",
            "--trust-base",
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "FLAKY\n")]);
    for (pr, head) in [("1", "a"), ("2", "b"), ("3", "c")] {
        fx.mq_ok(&["submit", pr, "--head", head]);
    }
    let (report, reruns) = run(&fx);
    // Split, not searched: every probe is a single's same-commit rerun (the base is trusted, so
    // it is never probed), none a prefix of several changes of the red batch.
    let validations = report["validations"].as_array().unwrap();
    assert!(
        validations
            .iter()
            .all(|v| v["purpose"] != "probe" || v["prs"].as_array().unwrap().len() == 1),
        "{report}"
    );
    assert!(
        purposes(&report).iter().filter(|p| *p == "batch").count() >= 4,
        "the red batch and a batch for each of its changes: {report}"
    );
    // #2 failed again on its rerun at its own commit: ejected, seen twice.
    let ejected = report["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{report}");
    assert_eq!(ejected[0]["pr"], 2, "{report}");
    let reason = ejected[0]["reason"].as_str().unwrap();
    assert!(reason.contains("(seen twice)"), "{reason}");
    assert!(!reason.contains(UNCONFIRMED), "{reason}");
    // #3 failed once and passed on its rerun: it lands, its failure recorded as flaky.
    let landed: Vec<i64> = landed(&report).into_iter().flatten().collect();
    assert!(landed.contains(&1) && landed.contains(&3), "{report}");
    // Each single was confirmed by one rerun of the same commit, handed the red run's handle.
    let handed: Vec<String> = std::fs::read_to_string(&reruns)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(handed.len(), 2, "{handed:?}");
}
