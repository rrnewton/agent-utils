//! Refill after refusal: when the head batch's admission refusal is deferred, the run does
//! not end with the other window slots idle. The batches behind it are rebuilt on the base
//! and every slot is filled again in the same run, so one run attempts and defers each
//! refused batch instead of deciding only the head per cycle.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

/// A queue whose validator always refuses admission (exit 75) and appends the candidate's
/// test-file count to `<root>/refused` on each attempt.
fn setup(tag: &str) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let validator = format!(
        r#"n=$(ls tests | wc -l | tr -d ' '); echo $n >> '{root}/refused'; echo COULD_NOT_RUN; exit 75"#,
        root = fx.root.display()
    );
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        &validator,
        "--refusal-backoff-secs",
        "0",
        "--refusal-max-retries",
        "0",
    ]);
    fx
}

fn refused_attempts(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.root.join("refused"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

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
fn a_refused_head_defers_and_the_run_refills_every_slot() {
    let fx = setup("refill-after-refusal");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    // One run, three slots, one change per batch: every candidate is refused, so the head
    // defers first; the run must then refill and attempt the other two in the same run.
    let _ = fx.mq(&["run", "--depth", "3", "--max-batch", "1"]);
    let run = last_run(&fx);
    let report: Value =
        serde_json::from_str(&std::fs::read_to_string(run.join("report.json")).unwrap()).unwrap();
    let mut deferred: Vec<i64> = report["deferred"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|entry| {
            entry["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|pr| pr.as_i64().unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    deferred.sort_unstable();
    assert_eq!(deferred, vec![1, 2, 3], "{report}");
    assert!(
        report["landed"].as_array().is_none_or(Vec::is_empty),
        "{report}"
    );
    // Each change was attempted on its own base candidate (two test files) after refill,
    // not only as part of a stack built on the refused head.
    let attempts = refused_attempts(&fx);
    assert!(
        attempts.len() >= 3,
        "expected at least 3 refused attempts, got {attempts:?}"
    );
    assert!(
        attempts.iter().filter(|n| *n == "2").count() >= 3,
        "expected each refilled slot to validate a single change on the base, got {attempts:?}"
    );
    for pr in [1, 2, 3] {
        let one = item(&fx, pr);
        assert_eq!(one["status"], "queued", "{one}");
        assert_eq!(one["validation_refusals"], 1, "{one}");
    }
}
