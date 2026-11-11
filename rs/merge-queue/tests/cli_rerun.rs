//! Same-commit reruns of failed tests through a runner handle (`MQ_HANDLE_FILE` /
//! `MQ_RERUN_FROM`), mapped onto flaky passes, with a trusted base and the exact-commit rule.
//! Same-commit confirmation is opt-in (`init --confirm-reruns`); every queue here turns it on.

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
        "--confirm-reruns",
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
    // The full batch run, a rerun of just `x` at the same commit (handed the
    // first run's handle), and a probe of the base itself: the seed base is
    // no queue landing, so `trust_base` does not spare it (#1187
    // mq-singleton-eject).
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 3, "{r}");
    // Probe order in the report is completion order, so match by commit:
    // one probe reruns the batch commit, one probes the base itself.
    let batch_commit = validations[0]["commit"].as_str().unwrap();
    assert!(
        validations[1..]
            .iter()
            .any(|v| v["purpose"] == "probe" && v["commit"] == batch_commit),
        "{r}"
    );
    assert!(
        validations[1..]
            .iter()
            .any(|v| v["purpose"] == "probe" && v["commit"] != batch_commit),
        "{r}"
    );
    assert_eq!(
        std::fs::read_to_string(&reruns).unwrap().trim(),
        "run-1 tests x"
    );
    // The landed commit is the exact commit validated (full run plus same-commit rerun).
    assert_eq!(r["landed"][0]["tip"], validations[0]["commit"]);
    assert_eq!(r["landed"][0]["claim"]["scope"], "complete");
}

/// Like `RUNNER`, but the rerun's own verdict comes from `$RERUN_SAYS`: `green-flaky` passes and
/// names the rerun tests on a `GREEN_FLAKY:` line, `infra-blocked` names them on an
/// `INFRA-BLOCKED:` line and exits 6, as a validator that composes the two runs does.
const COMPOSING_RUNNER: &str = r#"n=$(cat "$COUNTER" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$COUNTER"
echo "run-$n" > "$MQ_HANDLE_FILE"
if [ -n "$MQ_RERUN_FROM" ]; then
  if [ "$RERUN_SAYS" = infra-blocked ]; then
    for t in $MQ_TESTS; do echo "[$t] ✗ FAIL   rerun (0s, exit 1)"; done
    echo "INFRA-BLOCKED: $MQ_TESTS"
    exit 6
  fi
  for t in $MQ_TESTS; do echo "[$t] ✓ PASS   rerun (0s)"; done
  echo "GREEN_FLAKY: $MQ_TESTS"
  exit 0
fi
echo "[x] ✗ FAIL   x (1s, exit 1)"
echo "[y] ✓ PASS   y (1s)"
exit 1"#;

/// Submit one change, run the queue with `COMPOSING_RUNNER` answering `says` on the rerun, and
/// return the run report and the change's item record.
fn run_with_rerun_verdict(tag: &str, says: &str) -> (Fixture, Value, Value) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--validator",
        "command",
        "--validator-run",
        COMPOSING_RUNNER,
        "--exact-sha",
        "--trust-base",
        "--max-batch",
        "1",
        "--confirm-reruns",
    ]);
    fx.branch("a", &[("a", "a\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let counter = fx.root.join("counter");
    let out = fx.mq_env(
        &["run", "--json"],
        &[("COUNTER", counter.to_str().unwrap()), ("RERUN_SAYS", says)],
    );
    // A held change stops the run (a non-zero exit, as for any held batch); the report says why.
    let r: Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap_or_else(|e| {
            panic!(
                "bad report JSON ({e}): {}",
                String::from_utf8_lossy(&out.stderr)
            )
        });
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = items.as_array().unwrap()[0].clone();
    (fx, r, item)
}

fn history_kinds(item: &Value) -> Vec<String> {
    item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_failure_the_validator_calls_green_flaky_on_rerun_is_held_not_landed() {
    let (fx, r, item) = run_with_rerun_verdict("rerun-green-flaky", "green-flaky");
    // The rerun passed only because it was a rerun: held for a ruling, not landed as flaky.
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(
        r["stopped"].as_str().unwrap().contains("GREEN_FLAKY rerun"),
        "{}",
        r["stopped"]
    );
    // Batch, same-commit rerun, and the base probe (#1187: the seed base
    // is no queue landing, so it is probed too).
    assert_eq!(r["validations"].as_array().unwrap().len(), 3, "{r}");
    assert_eq!(item["status"], "queued", "{item}");
    assert_eq!(item["flaky_nodes"], serde_json::json!(["x"]), "{item}");
    assert!(item.get("hold_verdict").is_none(), "{item}");
    assert!(
        history_kinds(&item).contains(&"held-green-flaky".to_owned()),
        "{item}"
    );
    assert_eq!(fx.remote_main(), git(&fx.dev(), &["rev-parse", "main"]));
}

#[test]
fn a_failure_the_host_refuses_again_on_rerun_is_held_infra_blocked() {
    let (fx, r, item) = run_with_rerun_verdict("rerun-infra-blocked", "infra-blocked");
    // No result for `x` on either run: neither landed nor ejected, held for a ruling.
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(
        r["stopped"]
            .as_str()
            .unwrap()
            .contains("INFRA-BLOCKED rerun"),
        "{}",
        r["stopped"]
    );
    assert_eq!(item["status"], "queued", "{item}");
    assert_eq!(item["flaky_nodes"], serde_json::json!(["x"]), "{item}");
    assert_eq!(item["hold_verdict"], "INFRA-BLOCKED", "{item}");
    assert!(
        history_kinds(&item).contains(&"held-infra-blocked".to_owned()),
        "{item}"
    );
    // The plan shows the hold and its reason; `retry` returns the change to the queue.
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    let held = &plan["held"][0];
    assert_eq!(held["kind"], "held-infra-blocked", "{plan}");
    assert!(
        held["reason"].as_str().unwrap().contains("INFRA-BLOCKED"),
        "{plan}"
    );
    fx.mq_ok(&["retry", "1"]);
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    assert!(item.get("hold_verdict").is_none(), "{item}");
    assert!(item.get("flaky_hold_until").is_none(), "{item}");
    let last = item["history"].as_array().unwrap().last().unwrap().clone();
    assert!(
        last["detail"]
            .as_str()
            .unwrap()
            .contains("INFRA-BLOCKED hold"),
        "{last}"
    );
}
