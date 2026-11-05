//! Retries that never hold up the run. A batch whose validation admission refused starts again,
//! after its backoff, as a validation of its own beside the run's others, so the run goes on
//! taking requests meanwhile. And a `retry` recorded for a change the run holds (escalated to its
//! owner before the run began) applies at the run's next free slot, which plans the change again,
//! instead of waiting for the run to end.

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

/// A queue whose validations are refused (exit 75) while `<root>/refuse-all` exists, those of
/// candidates with N test files while `<root>/refuse-always/N` exists, and the next one of them
/// when `<root>/refuse-once/N` exists (which the refusal removes). Each refusal appends N to
/// `<root>/refused`. An admitted validation appends the candidate's test files to `<root>/started`,
/// waits (at most 60 s) for the gate named after how many test files the candidate has, and
/// passes. `extra` are more `init` arguments.
fn setup(tag: &str, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    for dir in ["gates", "refuse-once", "refuse-always"] {
        std::fs::create_dir_all(fx.root.join(dir)).unwrap();
    }
    let validator = format!(
        r#"echo validating $(ls tests) >&2; n=$(ls tests | wc -l | tr -d ' ')
if [ -e '{root}/refuse-all' ] || [ -e '{root}/refuse-always/'$n ] || rm '{root}/refuse-once/'$n 2>/dev/null; then
  echo $n >> '{root}/refused'; echo COULD_NOT_RUN; exit 75
fi
echo $(ls tests) >> '{root}/started'
i=0; while [ ! -e '{root}/gates/'$n ] && [ $i -lt 1200 ]; do sleep 0.05; i=$((i+1)); done
[ -e '{root}/gates/'$n ] || exit 1
for t in $(ls tests); do printf '{{"id":"%s","outcome":"pass"}}\n' "$t" >> "$MQ_RESULTS"; done"#,
        root = fx.root.display()
    );
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--refusal-backoff-secs",
        "0",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

/// Open the gate of candidates with `files` test files.
fn open(fx: &Fixture, files: usize) {
    std::fs::write(fx.root.join("gates").join(files.to_string()), "").unwrap();
}

/// How many validations were refused so far.
fn refused(fx: &Fixture) -> usize {
    std::fs::read_to_string(fx.root.join("refused"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// The test files of each admitted validation so far, in order.
fn started(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.root.join("started"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

struct Daemon(Child);

impl Daemon {
    /// One run (`--once`), two batches of one change each in flight at most.
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
            .args(["--depth", "2", "--max-batch", "1"])
            .current_dir(fx.queue())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(fx.root.join("serve.err")).unwrap());
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

/// Wait for `check`; on a timeout, fail with the events of `fx`'s newest run.
fn wait_for(fx: &Fixture, what: &str, timeout: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if check() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let runs = fx.queue().join(".git/merge-queue/runs");
    let any_run = std::fs::read_dir(&runs).is_ok_and(|mut dirs| dirs.next().is_some());
    let events = if any_run {
        std::fs::read_to_string(last_run(fx).join("events.jsonl")).unwrap_or_default()
    } else {
        String::new()
    };
    let serve = std::fs::read_to_string(fx.root.join("serve.err")).unwrap_or_default();
    panic!(
        "timed out waiting for {what}; started {:?}; newest run's events:\n{events}\nserve:\n{serve}",
        started(fx)
    );
}

/// The newest run's directory.
fn last_run(fx: &Fixture) -> PathBuf {
    let runs = fx.queue().join(".git/merge-queue/runs");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(runs)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    dirs.sort();
    dirs.pop().expect("a run")
}

fn report(run: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(run.join("report.json")).unwrap()).unwrap()
}

/// The details of `run`'s events of `kind`.
fn events(run: &Path, kind: &str) -> Vec<String> {
    std::fs::read_to_string(run.join("events.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["kind"] == kind)
        .map(|event| event["detail"].as_str().unwrap().to_owned())
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

fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr && item["status"] != "pending-intake")
        .cloned()
        .unwrap_or_else(|| panic!("#{pr} is not in the queue: {items}"))
}

#[test]
fn a_refused_batch_validates_again_beside_the_run_which_takes_requests_meanwhile() {
    let fx = setup("retry-beside-refused", &["--refusal-max-retries", "3"]);
    // Change 1's first validation (two test files) is refused.
    std::fs::write(fx.root.join("refuse-once/2"), "").unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let mut daemon = Daemon::start(&fx);
    wait_for(
        &fx,
        "change 1's retry to start",
        Duration::from_secs(30),
        || started(&fx).len() == 1,
    );
    // Its retry waits at the gate. A change submitted now is taken in and validates beside it.
    fx.branch("b", &[("tests/b", "ok\n")]);
    let out = fx.mq_ok(&["submit", "2", "--head", "b"]);
    assert!(out.contains("recorded #2"), "{out}");
    wait_for(
        &fx,
        "change 2 to validate beside change 1's retry",
        Duration::from_secs(30),
        || started(&fx).len() == 2,
    );
    assert_eq!(started(&fx), vec!["a t0".to_owned(), "a b t0".to_owned()]);
    open(&fx, 3);
    open(&fx, 2);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    let run = last_run(&fx);
    let report = report(&run);
    assert_eq!(landed(&report), vec![vec![1], vec![2]], "{report}");
    assert!(
        report["deferred"].as_array().is_none_or(Vec::is_empty),
        "{report}"
    );
    // The refused attempt is its own record: not green, not red.
    let validations = report["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 3, "{report}");
    assert_eq!(validations[0]["refused"], true, "{report}");
    assert_eq!(validations[0]["failed"], serde_json::json!([]), "{report}");
    let refused = events(&run, "validation-refused");
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert!(
        refused[0].contains("#1 refused")
            && refused[0].contains("retry 1 of 3 after 0s, beside the run"),
        "{refused:?}"
    );
    assert_eq!(item(&fx, 1)["status"], "landed");
    assert_eq!(item(&fx, 2)["status"], "landed");
}

#[test]
fn a_retry_of_a_change_held_since_the_run_began_applies_at_once_and_plans_it() {
    let fx = setup(
        "retry-beside-held",
        &["--refusal-max-retries", "0", "--max-deferrals", "1"],
    );
    // Change 1 is refused once and, with a bound of one deferral, escalated to its owner.
    std::fs::write(fx.root.join("refuse-all"), "").unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
    assert!(!item(&fx, 1)["escalated"]
        .as_str()
        .unwrap_or_default()
        .is_empty());
    std::fs::remove_file(fx.root.join("refuse-all")).unwrap();
    // The next run holds it, and validates change 2 alone.
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for(&fx, "change 2 to validate", Duration::from_secs(30), || {
        started(&fx).len() == 1
    });
    assert_eq!(started(&fx), vec!["b t0".to_owned()]);
    // Its owner's retry, recorded while the run holds the queue, applies at the free slot:
    // change 1 validates on top of change 2 in the same run.
    let out = fx.mq_ok(&["retry", "1"]);
    assert!(out.contains("recorded the retry of #1"), "{out}");
    wait_for(
        &fx,
        "change 1 to validate beside change 2",
        Duration::from_secs(30),
        || started(&fx).len() == 2,
    );
    assert_eq!(started(&fx), vec!["b t0".to_owned(), "a b t0".to_owned()]);
    open(&fx, 3);
    open(&fx, 2);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    let run = last_run(&fx);
    let report = report(&run);
    assert_eq!(landed(&report), vec![vec![2], vec![1]], "{report}");
    let intake = report["intake"].as_array().unwrap();
    assert!(
        intake
            .iter()
            .any(|line| line.as_str().unwrap().starts_with("applied retry #1")),
        "{report}"
    );
    let released = events(&run, "retry");
    assert_eq!(
        released,
        vec!["#1: its owner's retry applied; planned again in this run".to_owned()]
    );
    let one = item(&fx, 1);
    assert_eq!(one["status"], "landed", "{one}");
    assert!(one["escalated"].is_null(), "{one}");
}

#[test]
fn a_batch_whose_retries_ran_out_behind_the_head_is_retried_afresh_at_the_head() {
    let fx = setup("retry-beside-stale", &["--refusal-max-retries", "2"]);
    // Change 2, built on change 1 (three test files), is refused while change 1 waits at its
    // gate: its first validation and both retries.
    std::fs::write(fx.root.join("refuse-always/3"), "").unwrap();
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let mut daemon = Daemon::start(&fx);
    wait_for(
        &fx,
        "change 2's retries to run out",
        Duration::from_secs(30),
        || refused(&fx) == 3,
    );
    assert_eq!(started(&fx), vec!["a t0".to_owned()]);
    // Admission recovers. Once change 1 lands, change 2 is the head, and its old refusal does
    // not defer it: it is validated again and lands in the same run.
    std::fs::remove_file(fx.root.join("refuse-always/3")).unwrap();
    open(&fx, 3);
    open(&fx, 2);
    assert_eq!(daemon.exit_code(Duration::from_secs(60)), Some(0));
    let run = last_run(&fx);
    let report = report(&run);
    assert_eq!(landed(&report), vec![vec![1], vec![2]], "{report}");
    assert!(
        report["deferred"].as_array().is_none_or(Vec::is_empty),
        "{report}"
    );
    assert_eq!(started(&fx), vec!["a t0".to_owned(), "a b t0".to_owned()]);
    let retried: Vec<String> = events(&run, "validation-refused")
        .into_iter()
        .filter(|event| event.starts_with("#2 refused"))
        .collect();
    assert_eq!(retried.len(), 3, "{retried:?}");
    assert!(retried[1].contains("retry 2 of 2"), "{retried:?}");
    assert!(retried[2].contains("retry 1 of 2"), "{retried:?}");
}
