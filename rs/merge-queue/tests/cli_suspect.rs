//! Suspect-flake reruns (`init --suspect-flake-reruns`). With same-commit confirmation off, a
//! single change whose red run failed only on suspect tests (a timeout, or a run that reached no
//! verdict, as the runner shapes its result rows) is held instead of ejected; those tests run
//! again at its commit beside the run, and it goes back to the queue if they pass or is ejected,
//! seen twice, if they fail again. Once per head: a second suspect red is blamed as any red is.

mod support;

use std::path::{Path, PathBuf};

use serde_json::Value;
use support::Fixture;

/// Each file under `tests/` is one test, and says how it fails: `FAIL` always (an assertion),
/// `TIMEOUT-ALWAYS` with a timeout shape on every run, and `TIMEOUT-ONCE` or `TIMEOUT-TWICE`
/// with a timeout shape on that many complete runs (a focused run passes). Every run appends its
/// selection to `$CALLS`.
const RUNNER: &str = r#"echo "$MQ_SELECTION ${MQ_TESTS:-}" >> "$CALLS"
if [ "$MQ_SELECTION" = tests ]; then set -- $MQ_TESTS; else set -- $(ls tests); fi
rc=0
for t in "$@"; do
  o=pass; s=""
  f="tests/$t"
  if [ -f "$f" ] && grep -q FAIL "$f"; then o=fail; rc=1; fi
  if [ -f "$f" ] && grep -q TIMEOUT-ALWAYS "$f"; then o=fail; s=',"shape":"timeout"'; rc=1; fi
  for n in ONCE:1 TWICE:2; do
    if [ -f "$f" ] && grep -q "TIMEOUT-${n%:*}" "$f" && [ "$MQ_SELECTION" = all ]; then
      seen=$(cat "$MARKS/$t" 2>/dev/null || echo 0)
      if [ "$seen" -lt "${n#*:}" ]; then
        echo $((seen + 1)) > "$MARKS/$t"; o=fail; s=',"shape":"timeout"'; rc=1
      fi
    fi
  done
  printf '{"id":"%s","outcome":"%s"%s}\n' "$t" "$o" "$s" >> "$MQ_RESULTS"
done
exit $rc"#;

const UNCONFIRMED: &str = "(seen once: same-commit confirmation is off (validator.confirm_reruns))";

fn setup(tag: &str, extra: &[&str]) -> Fixture {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    support::git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let mut args = vec![
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--max-batch",
        "1",
        "--trust-base",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    std::fs::create_dir_all(fx.root.join("marks")).unwrap();
    fx
}

/// `run --json`; the report.
fn run(fx: &Fixture) -> Value {
    let calls = fx.root.join("calls");
    let marks = fx.root.join("marks");
    let out = fx.mq_env(
        &["run", "--json"],
        &[
            ("CALLS", calls.to_str().unwrap()),
            ("MARKS", marks.to_str().unwrap()),
        ],
    );
    serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|e| {
        panic!(
            "bad report JSON ({e}): {}",
            String::from_utf8_lossy(&out.stderr)
        )
    })
}

/// Every selection the runner was asked for, in order.
fn calls(fx: &Fixture) -> Vec<String> {
    std::fs::read_to_string(fx.root.join("calls"))
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().to_owned())
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

/// The details of run `report`'s events of `kind`.
fn events(fx: &Fixture, report: &Value, kind: &str) -> Vec<String> {
    let path: PathBuf = fx
        .queue()
        .join(".git/merge-queue/runs")
        .join(report["run"].as_str().unwrap())
        .join("events.jsonl");
    read_events(&path, kind)
}

fn read_events(path: &Path, kind: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|event| event["kind"] == kind)
        .map(|event| event["detail"].as_str().unwrap().to_owned())
        .collect()
}

fn landed(report: &Value) -> Vec<i64> {
    report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|batch| batch["prs"].as_array().unwrap().clone())
        .map(|pr| pr.as_i64().unwrap())
        .collect()
}

#[test]
fn a_change_that_timed_out_once_is_rerun_beside_the_run_and_lands_after_it_passes() {
    let fx = setup("suspect-pass", &["--suspect-flake-reruns"]);
    fx.branch("a", &[("tests/a", "TIMEOUT-ONCE\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let first = run(&fx);
    // Held, not ejected; the rerun of just that test passed beside the run, which re-queued it.
    assert!(first["ejected"].as_array().unwrap().is_empty(), "{first}");
    assert!(landed(&first).is_empty(), "{first}");
    let held = events(&fx, &first, "suspect-flake");
    assert!(
        held.iter()
            .any(|e| e.contains("#1 red only on suspect tests (a), seen once: held")),
        "{held:?}"
    );
    assert!(
        held.iter()
            .any(|e| e.contains("GREEN_FLAKY: a passed on a rerun")),
        "{held:?}"
    );
    let one = item(&fx, 1);
    assert_eq!(one["status"], "queued", "{one}");
    assert!(one.get("hold_verdict").is_none(), "{one}");
    assert_eq!(one["suspect_reruns"], 1, "{one}");
    // The next run validates it again on the base and lands it.
    let second = run(&fx);
    assert_eq!(landed(&second), vec![1], "{second}");
    assert_eq!(calls(&fx), vec!["all", "tests a", "all"]);
}

#[test]
fn a_change_whose_rerun_times_out_again_is_ejected_seen_twice() {
    let fx = setup("suspect-fail", &["--suspect-flake-reruns"]);
    fx.branch("b", &[("tests/b", "TIMEOUT-ALWAYS\n")]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let report = run(&fx);
    let ejected = report["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{report}");
    assert_eq!(ejected[0]["pr"], 2, "{report}");
    let reason = ejected[0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("failed again on a rerun of that commit beside the run (seen twice"),
        "{reason}"
    );
    assert!(!reason.contains(UNCONFIRMED), "{reason}");
    assert_eq!(item(&fx, 2)["status"], "ejected");
    assert_eq!(calls(&fx), vec!["all", "tests b"]);
}

#[test]
fn a_second_suspect_red_of_the_same_head_is_blamed_as_any_red_is() {
    let fx = setup("suspect-twice", &["--suspect-flake-reruns"]);
    fx.branch("e", &[("tests/e", "TIMEOUT-TWICE\n")]);
    fx.mq_ok(&["submit", "5", "--head", "e"]);
    run(&fx);
    assert_eq!(item(&fx, 5)["status"], "queued");
    let second = run(&fx);
    let ejected = second["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{second}");
    assert!(
        ejected[0]["reason"].as_str().unwrap().contains(UNCONFIRMED),
        "{second}"
    );
    // Run two adds a probe of the base itself (#1187: the seed base is no
    // queue landing, so it is measured before the blame).
    assert_eq!(calls(&fx), vec!["all", "tests e", "all", "tests e"]);
}

#[test]
fn any_failure_that_is_not_suspect_is_blamed_at_once() {
    let fx = setup("suspect-mixed", &["--suspect-flake-reruns"]);
    fx.branch(
        "c",
        &[("tests/c", "FAIL\n"), ("tests/c2", "TIMEOUT-ALWAYS\n")],
    );
    fx.mq_ok(&["submit", "3", "--head", "c"]);
    let report = run(&fx);
    let ejected = report["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{report}");
    assert!(
        ejected[0]["reason"].as_str().unwrap().contains(UNCONFIRMED),
        "{report}"
    );
    // Plus the base probe (#1187: the seed base is no queue landing).
    assert_eq!(calls(&fx), vec!["all", "tests c c2"]);
}

#[test]
fn without_the_flag_a_timeout_is_blamed_at_once() {
    let fx = setup("suspect-off", &[]);
    fx.branch("d", &[("tests/d", "TIMEOUT-ALWAYS\n")]);
    fx.mq_ok(&["submit", "4", "--head", "d"]);
    let report = run(&fx);
    assert_eq!(report["ejected"].as_array().unwrap().len(), 1, "{report}");
    // Plus the base probe (#1187: the seed base is no queue landing).
    assert_eq!(calls(&fx), vec!["all", "tests d"]);
}
