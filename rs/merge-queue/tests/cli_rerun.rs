//! Same-commit reruns of failed tests through a runner handle (`MQ_HANDLE_FILE` /
//! `MQ_RERUN_FROM`), mapped onto flaky passes, with a trusted base and the exact-commit rule.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// A full run fails `x` and writes a handle; a run given `MQ_RERUN_FROM` reruns only what that
/// run failed, and passes (a flaky failure), recording which handle it was given.
const RUNNER: &str = r#"n=$(cat "$COUNTER" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$COUNTER"
echo "run-$n" > "$MQ_HANDLE_FILE"
if [ -n "$MQ_RERUN_FROM" ]; then
  echo "$MQ_RERUN_FROM $MQ_SELECTION $MQ_TESTS" >> "$RERUNS"
  for t in $MQ_TESTS; do echo "[$t] ✓ PASS   rerun (0s)"; done
  exit 0
fi
echo "[x] ✗ FAIL   x (1s, exit 1)"
echo "[y] ✓ PASS   y (1s)"
exit 1"#;

#[test]
fn a_failure_that_passes_on_a_same_commit_rerun_lands_as_flaky() {
    let fx = Fixture::new("rerun");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--exact-sha",
        "--trust-base",
        "--max-batch",
        "1",
    ]);
    fx.branch("a", &[("a", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let counter = fx.root.join("counter");
    let reruns = fx.root.join("reruns");
    let out = fx.mq_env(
        &["run", "--json"],
        &[
            ("COUNTER", counter.to_str().unwrap()),
            ("RERUNS", reruns.to_str().unwrap()),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(r["landed"][0]["prs"], serde_json::json!([1]));
    assert_eq!(r["landed"][0]["flaky"], serde_json::json!(["x"]));
    // Two runs only: the full batch run, and a rerun of just `x` at the same commit, handed the
    // first run's handle. The trusted base was not probed.
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 2, "{r}");
    assert_eq!(validations[0]["commit"], validations[1]["commit"]);
    assert_eq!(
        std::fs::read_to_string(&reruns).unwrap().trim(),
        "run-1 tests x"
    );
    // The landed commit is the exact commit validated (full run plus same-commit rerun).
    assert_eq!(r["landed"][0]["tip"], validations[0]["commit"]);
    assert_eq!(r["landed"][0]["claim"]["scope"], "complete");
}
