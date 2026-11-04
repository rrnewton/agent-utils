//! End-to-end `run`: speculative batches, failed-test-only bisection, ejection, landing drivers,
//! shadow and dry runs, and widening obligations, against a local bare remote.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// A tiny test runner: each file under `tests/` is one test, failing when it contains FAIL, or
/// failing once (then passing) when it contains FLAKY and `$FLAKY_MARKER` does not exist yet.
const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; fi
  if [ -f "tests/$t" ] && grep -q FLAKY "tests/$t" && [ ! -e "$FLAKY_MARKER" ]; then o=fail; touch "$FLAKY_MARKER"; fi
  [ $o = fail ] && rc=1
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
sleep 0.5
exit $rc"#;

/// Path-routing policy: the tests whose files changed.
const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn setup(tag: &str, extra: &[&str]) -> Fixture {
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
        RUNNER,
        "--select",
        SELECT,
        "--policy",
        "path-routing",
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
}

fn landed_prs(report: &Value) -> Vec<Vec<i64>> {
    report["landed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            l["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_i64().unwrap())
                .collect()
        })
        .collect()
}

fn purposes(report: &Value) -> Vec<String> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["purpose"].as_str().unwrap().to_owned())
        .collect()
}

fn remote_files(fx: &Fixture) -> String {
    git(&fx.remote(), &["ls-tree", "-r", "--name-only", "main"])
}

fn setup_validator(tag: &str, validator_run: &str, extra: &[&str]) -> Fixture {
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
        validator_run,
    ];
    args.extend_from_slice(extra);
    fx.mq_ok(&args);
    fx
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
        .unwrap()
}

#[test]
fn green_flaky_batch_is_held_for_a_ruling_not_landed() {
    // Green only because a test passed after a retry inside the run: the
    // validator says so with the GREEN_FLAKY marker. The batch must be held,
    // never landed on the plain pass and never ejected as red.
    let fx = setup_validator("run-green-flaky", "echo \"GREEN_FLAKY: a\"; exit 0", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(
        r["stopped"].as_str().unwrap().contains("GREEN_FLAKY"),
        "{}",
        r["stopped"]
    );
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_eq!(item["flaky_nodes"], serde_json::json!(["a"]), "{item}");
    assert!(
        !item["flaky_hold_until"].as_str().unwrap().is_empty(),
        "{item}"
    );
    let history_kinds: Vec<&str> = item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        history_kinds.contains(&"held-green-flaky"),
        "{history_kinds:?}"
    );
}

#[test]
fn green_flaky_hold_expires_and_requeues() {
    // The bounded hold: once it expires the change is re-queued for a fresh
    // validation (which re-holds if it is GREEN_FLAKY again), never landed
    // unruled and never stuck silently.
    let fx = setup_validator(
        "run-green-flaky-expiry",
        "echo \"GREEN_FLAKY: a\"; exit 0",
        &["--green-flaky-hold-secs", "1"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    fx.mq(&["run", "--json"]);
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(fx.remote_main(), before);
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    let history_kinds: Vec<&str> = item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        history_kinds.contains(&"green-flaky-hold-expired"),
        "{history_kinds:?}"
    );
    // Re-validated after release (and held again by the fresh GREEN_FLAKY).
    assert_eq!(
        history_kinds
            .iter()
            .filter(|k| **k == "held-green-flaky")
            .count(),
        2,
        "{history_kinds:?}"
    );
}

#[test]
fn refusal_exit75_keeps_change_queued_and_is_neither_red_nor_pass() {
    // Admission refuses every validation: exit 75 with the COULD_NOT_RUN marker.
    // No within-run retries, long cross-run backoff, so the deferral is observable.
    let fx = setup_validator(
        "run-refusal",
        "echo COULD_NOT_RUN; exit 75",
        &["--refusal-max-retries", "0", "--refusal-backoff-secs", "60"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "refused run must not report success"
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    // Never a pass: nothing landed. Never red: nothing ejected, no culprit search.
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["deferred"][0]["prs"], serde_json::json!([1]), "{r}");
    assert!(
        r["stopped"]
            .as_str()
            .unwrap()
            .contains("validation refused"),
        "{}",
        r["stopped"]
    );
    // Every validation is refused, not green and not red.
    assert!(!r["validations"].as_array().unwrap().is_empty());
    for v in r["validations"].as_array().unwrap() {
        assert_eq!(v["refused"], true, "{v}");
        assert_eq!(v["green"], false, "{v}");
        assert_eq!(v["failed"], serde_json::json!([]), "{v}");
    }
    assert_eq!(fx.remote_main(), before);
    // The distinct non-terminal state: still queued, with refusal bookkeeping.
    assert_eq!(status(&fx, 1), "queued");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_eq!(item["validation_refusals"], 1, "{item}");
    assert!(
        !item["next_retry_at"].as_str().unwrap().is_empty(),
        "{item}"
    );
    assert!(
        item["last_refusal"].as_str().unwrap().contains("75"),
        "{item}"
    );
    let history_kinds: Vec<&str> = item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        history_kinds.contains(&"validation-refused"),
        "{history_kinds:?}"
    );
    assert!(!history_kinds.contains(&"ejected"), "{history_kinds:?}");
    // While the backoff runs, the plan holds the change as validation-refused.
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["batches"].as_array().unwrap().is_empty(), "{plan}");
    assert_eq!(plan["held"][0]["kind"], "validation-refused", "{plan}");
}

/// A validator that cannot execute: the change stays queued with no verdict, the refusal
/// names the command and PATH, and it is retried with backoff (#814).
fn assert_cannot_execute(tag: &str, validator_run: &str, files: &[(&str, &str)], names: &str) {
    let fx = Fixture::new(tag);
    fx.write("tests/t0", "ok\n");
    for (path, text) in files {
        fx.write(path, text);
    }
    fx.commit_all("seed");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        validator_run,
        "--refusal-max-retries",
        "1",
        "--refusal-backoff-secs",
        "1",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    // Retried within the run after the backoff, refused both times, never red.
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 2, "{r}");
    for v in validations {
        assert_eq!(v["refused"], true, "{v}");
        assert_eq!(v["failed"], serde_json::json!([]), "{v}");
    }
    let reason = r["deferred"][0]["reason"].as_str().unwrap().to_owned();
    for part in [
        "the validator could not execute",
        "exit 127: a command was not found",
        names,
        "PATH=",
    ] {
        assert!(reason.contains(part), "{part:?} missing from {reason}");
    }
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_eq!(item["last_refusal"].as_str().unwrap(), reason, "{item}");
    assert!(
        !item["next_retry_at"].as_str().unwrap().is_empty(),
        "{item}"
    );
    assert!(
        !item["history"].to_string().contains("failed-tests"),
        "{item}"
    );
}

#[test]
fn a_nonexistent_validator_binary_is_an_infrastructure_refusal_not_red() {
    assert_cannot_execute(
        "run-exec-missing",
        "/nonexistent/mq-validator --check",
        &[],
        "/nonexistent/mq-validator",
    );
}

#[test]
fn a_shell_wrapper_whose_tool_is_missing_is_an_infrastructure_refusal_not_red() {
    assert_cannot_execute(
        "run-exec-wrapper",
        "sh ./validate.sh",
        &[("validate.sh", "echo checking\nmq-missing-tool-xyz --all\n")],
        "mq-missing-tool-xyz",
    );
}

/// The validator for #816: a full run (`MQ_SELECTION=all`) is red, attributing `t0`, when
/// the checkout holds `bad.txt`; any probe (a test selection) is refused with exit 75.
const RED_UNLESS_PROBE: &str = r#"if [ "$MQ_SELECTION" = tests ]; then echo COULD_NOT_RUN; exit 75; fi
if [ -f bad.txt ]; then echo '{"id":"t0","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1; fi
echo '{"id":"t0","outcome":"pass"}' >> "$MQ_RESULTS"; exit 0"#;

fn items(fx: &Fixture) -> Vec<Value> {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items.as_array().unwrap().clone()
}

#[test]
fn a_red_validation_is_a_verdict_even_when_the_probe_cannot_run() {
    let fx = setup_validator(
        "run-red-probe-refused",
        RED_UNLESS_PROBE,
        &["--refusal-max-retries", "0", "--refusal-backoff-secs", "0"],
    );
    fx.branch("a", &[("bad.txt", "x\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["ejected"][0]["pr"], 1, "{r}");
    assert_eq!(r["ejected"][0]["kind"], "failed-tests", "{r}");
    let reason = r["ejected"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("narrowing and the flaky check were skipped"),
        "{reason}"
    );
    assert!(r["deferred"].as_array().is_none_or(|d| d.is_empty()), "{r}");
    // One full validation and one refused probe: the red commit is never validated again.
    let purposes = purposes(&r);
    assert_eq!(
        purposes.iter().filter(|p| *p != "probe").count(),
        1,
        "{purposes:?}"
    );
    assert_eq!(status(&fx, 1), "ejected");
    let history = items(&fx)[0]["history"].to_string();
    assert_eq!(history.matches("\"ejected\"").count(), 1, "{history}");
}

#[test]
fn a_red_batch_whose_probe_cannot_run_is_validated_change_by_change() {
    let fx = setup_validator(
        "run-red-batch-probe-refused",
        RED_UNLESS_PROBE,
        &[
            "--refusal-max-retries",
            "0",
            "--refusal-backoff-secs",
            "0",
            "--max-batch",
            "2",
        ],
    );
    fx.branch("a", &[("a.txt", "a\n")]);
    fx.branch("b", &[("bad.txt", "x\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(landed_prs(&r), vec![vec![1]], "{r}");
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    assert_eq!(r["ejected"][0]["pr"], 2, "{r}");
    assert_eq!(status(&fx, 1), "landed");
    assert_eq!(status(&fx, 2), "ejected");
}

#[test]
fn hold_mode_escalates_a_red_whose_probe_cannot_run_and_retry_releases_it() {
    let fx = setup_validator(
        "run-red-probe-hold",
        RED_UNLESS_PROBE,
        &[
            "--refusal-max-retries",
            "0",
            "--refusal-backoff-secs",
            "0",
            "--red-without-probe",
            "hold",
        ],
    );
    fx.branch("a", &[("bad.txt", "x\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ALARM: #1 escalated to the owner"),
        "{stderr}"
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(status(&fx, 1), "queued");
    assert!(!items(&fx)[0]["escalated"].as_str().unwrap().is_empty());
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert_eq!(plan["held"][0]["kind"], "escalated", "{plan}");
    fx.mq_ok(&["retry", "1"]);
    assert!(items(&fx)[0]["escalated"].is_null());
    let plan: Value = serde_json::from_str(&fx.mq_ok(&["plan", "--json"])).unwrap();
    assert!(plan["held"].as_array().unwrap().is_empty(), "{plan}");
}

/// Deferrals are bounded: after `--max-deferrals` the change is escalated, held, and no
/// longer validated until `retry`.
fn assert_deferrals_bounded(tag: &str, run: &[&str]) {
    let fx = setup_validator(
        tag,
        "echo COULD_NOT_RUN; exit 75",
        &[
            "--refusal-max-retries",
            "0",
            "--refusal-backoff-secs",
            "0",
            "--max-deferrals",
            "2",
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let first = fx.mq(run);
    assert!(!String::from_utf8_lossy(&first.stderr).contains("ALARM"));
    let second = fx.mq(run);
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("ALARM: #1 escalated to the owner"),
        "{stderr}"
    );
    assert!(
        stderr.contains("did not run 2 time(s) (the bound is 2)"),
        "{stderr}"
    );
    let r = report(&String::from_utf8_lossy(&second.stdout));
    assert_eq!(r["deferred"][0]["escalated"], serde_json::json!([1]), "{r}");
    // Held: the next run validates nothing.
    let third = fx.mq(run);
    let r = report(&String::from_utf8_lossy(&third.stdout));
    assert!(r["validations"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(status(&fx, 1), "queued");
    // The owner returns it once the cause is fixed.
    fx.mq_ok(&["retry", "1"]);
    let fourth = fx.mq(run);
    let r = report(&String::from_utf8_lossy(&fourth.stdout));
    assert_eq!(r["validations"].as_array().unwrap().len(), 1, "{r}");
}

#[test]
fn deferrals_are_bounded_then_escalated_to_the_owner() {
    assert_deferrals_bounded("run-deferral-bound", &["run", "--json"]);
}

#[test]
fn shadow_runs_bound_deferrals_too() {
    assert_deferrals_bounded("run-deferral-bound-shadow", &["run", "--json", "--shadow"]);
}

#[test]
fn refusal_is_retried_with_backoff_and_then_lands() {
    // Refuse twice (exit 75), then pass: the same run retries and lands.
    let counter = "refusal-counter";
    let script = "c=$(cat \"$REFUSAL_COUNTER\" 2>/dev/null || echo 0); c=$((c+1)); echo $c > \"$REFUSAL_COUNTER\"; if [ $c -le 2 ]; then echo COULD_NOT_RUN; exit 75; fi; exit 0";
    let fx = setup_validator(
        "run-refusal-retry",
        script,
        &["--refusal-max-retries", "3", "--refusal-backoff-secs", "0"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let counter_path = fx.root.join(counter);
    let out = fx.mq_env(
        &["run", "--json"],
        &[("REFUSAL_COUNTER", counter_path.to_str().unwrap())],
    );
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(landed_prs(&r), vec![vec![1]], "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["deferred"].as_array().is_none_or(|d| d.is_empty()), "{r}");
    // Two refused attempts, then the green attempt: refusals visible, not red.
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(validations.len(), 3, "{r}");
    assert_eq!(validations[0]["refused"], true);
    assert_eq!(validations[1]["refused"], true);
    assert_eq!(validations[2]["green"], true);
    assert_eq!(status(&fx, 1), "landed");
}

#[test]
fn green_batches_land_in_order_with_speculation() {
    let fx = setup("run-green", &["--max-batch", "1", "--depth", "2"]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.branch(name, &[(&format!("tests/{name}"), "ok\n")]);
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1], vec![2], vec![3]]);
    assert!(r["ejected"].as_array().unwrap().is_empty());
    // Batches 1 and 2 validated concurrently: their intervals overlap.
    let span = |v: &Value| {
        let start = v["start_ms"].as_u64().unwrap();
        (start, start + v["elapsed_ms"].as_u64().unwrap())
    };
    let (s1, e1) = span(&r["validations"][0]);
    let (s2, e2) = span(&r["validations"][1]);
    assert!(s1 < e2 && s2 < e1, "no overlap: {s1}-{e1} vs {s2}-{e2}");
    let files = remote_files(&fx);
    assert!(files.contains("tests/a") && files.contains("tests/b") && files.contains("tests/c"));
    assert_eq!(status(&fx, 2), "landed");
    // Each batch ran only the tests its change touched (focused), so each owes a complete run.
    let first = &r["validations"][0];
    assert_eq!(first["selection"]["kind"], "tests");
    let obligations: Value = serde_json::from_str(&fx.mq_ok(&["obligations", "--json"])).unwrap();
    assert_eq!(obligations.as_array().unwrap().len(), 3);
    let w = report(&fx.mq_ok(&["widen", "--json"]));
    assert_eq!(purposes(&w), vec!["widen"]);
    assert_eq!(w["validations"][0]["selection"]["kind"], "all");
    let open: Value = serde_json::from_str(&fx.mq_ok(&["obligations", "--json"])).unwrap();
    assert!(open.as_array().unwrap().is_empty());
}

#[test]
fn culprit_is_found_with_failed_tests_only_and_the_rest_lands() {
    let fx = setup("run-culprit", &["--max-batch", "4"]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c"), (4, "d")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1, 3, 4]]);
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 2);
    assert_eq!(ejected["kind"], "failed-tests");
    assert_eq!(ejected["failed"], serde_json::json!(["b"]));
    // Every probe and confirmation ran only the failed test.
    for v in r["validations"].as_array().unwrap() {
        if v["purpose"] == "probe" || v["purpose"] == "confirm" {
            assert_eq!(
                v["selection"],
                serde_json::json!({"kind": "tests", "tests": ["b"]}),
                "{v}"
            );
        }
    }
    let p = purposes(&r);
    assert_eq!(p[0], "batch");
    assert!(
        p.contains(&"probe".to_owned()) && p.contains(&"remainder".to_owned()),
        "{p:?}"
    );
    // The remainder was not rerun in full: it ran the failed test plus what #2 touched.
    let remainder = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["purpose"] == "remainder")
        .unwrap();
    assert_eq!(remainder["selection"]["tests"], serde_json::json!(["b"]));
    assert_eq!(r["landed"][0]["claim"]["scope"], "focused");
    assert!(r["landed"][0]["claim"]["policy"]
        .as_str()
        .unwrap()
        .starts_with("carried-forward"));
    let files = remote_files(&fx);
    assert!(files.contains("tests/a") && files.contains("tests/d") && !files.contains("tests/b"));
    assert_eq!(status(&fx, 2), "ejected");
    assert_eq!(status(&fx, 3), "landed");
}

#[test]
fn red_head_of_window_discards_the_candidate_behind_it() {
    let fx = setup("run-window", &["--max-batch", "1", "--depth", "2"]);
    fx.branch("a", &[("tests/a", "FAIL\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![2]]);
    assert_eq!(r["ejected"][0]["pr"], 1);
    // #2 was validated twice: speculatively on top of #1, then rebuilt on the real base.
    let batches_for_2 = r["validations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["purpose"] == "batch" && v["prs"] == serde_json::json!([2]))
        .count();
    assert_eq!(batches_for_2, 2);
    assert_eq!(r["base_end"], fx.remote_main());
}

#[test]
fn broken_base_stops_without_ejecting() {
    let fx = setup("run-base", &[]);
    // The base already fails t0; a change touching t0 cannot be blamed for it.
    fx.write("tests/t0", "FAIL\n");
    fx.commit_all("break t0");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.branch("a", &[("tests/t0", "FAIL\nmore\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(3));
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(
        r["stopped"].as_str().unwrap().contains("base"),
        "{}",
        r["stopped"]
    );
    assert!(r["ejected"].as_array().unwrap().is_empty());
    assert_eq!(status(&fx, 1), "queued");
}

#[test]
fn flaky_failure_is_detected_and_reported_not_blamed() {
    let fx = setup("run-flaky", &[]);
    fx.branch("a", &[("tests/a", "FLAKY\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let marker = fx.root.join("flaky-marker");
    let out = fx.mq_env(
        &["run", "--json"],
        &[("FLAKY_MARKER", marker.to_str().unwrap())],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(landed_prs(&r), vec![vec![1]]);
    assert_eq!(r["landed"][0]["flaky"], serde_json::json!(["a"]));
}

#[test]
fn command_driver_lands_change_by_change_and_checks_trees() {
    let fx = setup(
        "run-command",
        &[
            "--land",
            "command",
            "--land-command",
            "git push -q \"$MQ_REMOTE\" \"$MQ_PREFIX:refs/heads/$MQ_BASE\"",
            "--max-batch",
            "2",
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    // A per-change driver lands one change per batch unless batches are explicitly allowed.
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1], vec![2]]);
    assert_eq!(r["base_end"], fx.remote_main());
}

#[test]
fn command_driver_with_batches_allowed_lands_the_batch() {
    let fx = setup(
        "run-command-batch",
        &[
            "--land",
            "command",
            "--land-command",
            "git push -q \"$MQ_REMOTE\" \"$MQ_PREFIX:refs/heads/$MQ_BASE\"",
            "--allow-batches",
            "--max-batch",
            "2",
        ],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1, 2]]);
}

#[test]
fn shadow_and_dry_runs_change_nothing() {
    let fx = setup("run-shadow", &[]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    let dry = fx.mq_ok(&["run", "--dry-run", "--json"]);
    assert!(report(&dry)["validations"].as_array().unwrap().is_empty());
    let r = report(&fx.mq_ok(&["run", "--shadow", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1]]);
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    assert!(fx.mq_ok(&["run", "--shadow"]).contains("Landed"));
}

#[test]
fn adaptive_window_grows_on_landings_and_halves_on_red() {
    let fx = setup(
        "run-aimd",
        &["--max-batch", "1", "--depth", "2", "--max-depth", "4"],
    );
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    for (i, name) in names.iter().enumerate() {
        let body = if *name == "e" { "FAIL\n" } else { "ok\n" };
        fx.branch(name, &[(&format!("tests/{name}"), body)]);
        fx.mq_ok(&["submit", &(i + 1).to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(r["ejected"][0]["pr"], 5);
    let windows: Vec<u64> = r["windows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_u64().unwrap())
        .collect();
    assert_eq!(windows[0], 2, "{windows:?}");
    assert!(
        windows.iter().any(|w| *w >= 3),
        "window never grew: {windows:?}"
    );
    let peak = windows.iter().position(|w| *w >= 3).unwrap();
    assert!(
        windows[peak..].iter().any(|w| *w < 3),
        "window never shrank after red: {windows:?}"
    );
    assert!(r["max_concurrent"].as_u64().unwrap() >= 2);
    assert_eq!(landed_prs(&r).len(), 6);
    // Cancelled speculative work is visible in the report, never counted as red.
    for v in r["validations"].as_array().unwrap() {
        if v["cancelled"] == true {
            assert_eq!(v["failed"], serde_json::json!([]));
        }
    }
}

fn validated(report: &Value) -> Vec<(String, Vec<i64>)> {
    report["validations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            let prs = v["prs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p.as_i64().unwrap())
                .collect();
            (v["purpose"].as_str().unwrap().to_owned(), prs)
        })
        .collect()
}

#[test]
fn a_priority_change_queued_behind_three_ordinary_ones_is_validated_first() {
    let fx = setup("run-priority", &["--max-batch", "4", "--depth", "1"]);
    for name in ["a", "b", "c", "ci"] {
        fx.branch(name, &[(&format!("tests/{name}"), "ok\n")]);
    }
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let out = fx.mq_ok(&["submit", "4", "--head", "ci", "--prioritized"]);
    assert!(out.contains("front of the queue"), "{out}");
    let r = report(&fx.mq_ok(&["run", "--json"]));
    // Validated first, alone, then the ordinary batch.
    assert_eq!(
        validated(&r),
        vec![
            ("batch".to_owned(), vec![4]),
            ("batch".to_owned(), vec![1, 2, 3])
        ]
    );
    assert_eq!(landed_prs(&r), vec![vec![4], vec![1, 2, 3]]);
    // It landed on its own green at that exact commit.
    let first = &r["validations"][0];
    assert_eq!(r["landed"][0]["claim"]["commit"], first["commit"]);
    assert!(first["results"].to_string().contains("pass"), "{first}");
}

#[test]
fn a_red_priority_change_ejects_only_itself_and_never_lands() {
    let fx = setup("run-priority-red", &["--max-batch", "4", "--depth", "2"]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c")] {
        fx.branch(name, &[(&format!("tests/{name}"), "ok\n")]);
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    fx.branch("ci", &[("tests/ci", "FAIL\n")]);
    fx.mq_ok(&["submit", "4", "--head", "ci", "--prioritized"]);
    let r = report(&fx.mq_ok(&["run", "--json"]));
    let runs = validated(&r);
    assert_eq!(runs[0], ("batch".to_owned(), vec![4]));
    // No validation combined the red priority change with an ordinary one.
    for (_, prs) in &runs {
        assert!(!(prs.contains(&4) && prs.len() > 1), "{runs:?}");
    }
    assert_eq!(landed_prs(&r), vec![vec![1, 2, 3]]);
    let ejected = r["ejected"].as_array().unwrap();
    assert_eq!(ejected.len(), 1, "{ejected:?}");
    assert_eq!(ejected[0]["pr"], 4);
    assert_eq!(status(&fx, 4), "ejected");
    assert!(!remote_files(&fx).contains("tests/ci"));
}

fn decision_records(fx: &Fixture) -> Vec<Value> {
    fx.mq_ok(&["stats", "--records"])
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn every_validation_leaves_one_decision_record_and_stats_summarise_them() {
    let fx = setup("run-decisions", &["--max-batch", "4", "--depth", "1"]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "FAIL\n")]);
    fx.branch("c", &[("tests/c", "ok\n")]);
    fx.branch("d", &[("tests/d", "ok\n")]);
    for (pr, name) in [(1, "a"), (2, "b"), (3, "c"), (4, "d")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1, 3, 4]]);
    let records = decision_records(&fx);
    let validations = r["validations"].as_array().unwrap();
    assert_eq!(records.len(), validations.len(), "{records:?}");
    let batch = &records[0];
    assert_eq!(batch["purpose"], "batch");
    assert_eq!(batch["width"], 4);
    assert_eq!(batch["outcome"], "red");
    assert_eq!(batch["culprit"], 2, "{batch}");
    assert_eq!(batch["speculation"]["depth"], 1, "{batch}");
    assert_eq!(batch["base"], r["base_start"], "{batch}");
    assert!(batch["predicted_pass"].is_null());
    assert_eq!(batch["validator"]["kind"], "local");
    assert!(batch["cost"]["wall_s"].as_f64().unwrap() > 0.0);
    #[cfg(target_os = "linux")]
    assert!(batch["cost"]["cpu_s"].is_number(), "{batch}");
    let change = &batch["changes"][1];
    assert_eq!(change["pr"], 2);
    assert_eq!(change["features"]["files"], 1, "{change}");
    assert_eq!(change["features"]["lines_added"], 1, "{change}");
    assert_eq!(
        change["features"]["components"],
        serde_json::json!(["tests"])
    );
    assert_eq!(change["features"]["trigger"], "submit");
    let remainder = records
        .iter()
        .find(|d| d["purpose"] == "remainder")
        .unwrap();
    assert_eq!(remainder["landed"]["prs"], serde_json::json!([1, 3, 4]));
    assert!(remainder["landed"]["at"].is_string());
    for change in remainder["changes"].as_array().unwrap() {
        assert!(
            change["ready_to_land_s"].as_f64().unwrap() >= 0.0,
            "{change}"
        );
    }
    let stats: Value = serde_json::from_str(&fx.mq_ok(&["stats", "--json"])).unwrap();
    assert_eq!(stats["widths"]["batches"], 1, "{stats}");
    assert_eq!(stats["widths"]["counts"]["4"], 1, "{stats}");
    assert_eq!(stats["success_by_width"]["4"]["red"], 1, "{stats}");
    assert_eq!(stats["landed"], 3, "{stats}");
    assert_eq!(stats["culprits"], 1, "{stats}");
    assert_eq!(stats["ready_to_land_s"]["count"], 3, "{stats}");
    assert!(stats["landed_per_validate_hour"].as_f64().unwrap() > 0.0);
    let text = fx.mq_ok(&["stats"]);
    assert!(text.contains("width 4: 0 green, 1 red"), "{text}");
    // A window before the run holds nothing.
    let empty: Value = serde_json::from_str(&fx.mq_ok(&[
        "stats",
        "--json",
        "--since",
        "2020-01-01T00:00:00Z",
        "--until",
        "2020-01-02T00:00:00Z",
    ]))
    .unwrap();
    assert_eq!(empty["landed"], 0);
    assert!(fx
        .mq_ok(&["stats", "--mode", "shadow", "--records"])
        .is_empty());

    // Reconstruction from the run report alone, as for a run before the log existed: drop the
    // log and the fields older reports lack.
    let state = fx.queue().join(".git/merge-queue");
    std::fs::remove_dir_all(state.join("decisions")).unwrap();
    for entry in std::fs::read_dir(state.join("runs")).unwrap().flatten() {
        let path = entry.path().join("report.json");
        let mut old: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for v in old["validations"].as_array_mut().unwrap() {
            for key in ["parent", "speculation", "cpu_ms", "donor"] {
                v.as_object_mut().unwrap().remove(key);
            }
        }
        for e in old["ejected"].as_array_mut().unwrap() {
            e.as_object_mut().unwrap().remove("found_by");
        }
        for l in old["landed"].as_array_mut().unwrap() {
            l.as_object_mut().unwrap().remove("at");
        }
        std::fs::write(&path, serde_json::to_string(&old).unwrap()).unwrap();
    }
    assert!(fx.mq_ok(&["stats", "--records"]).is_empty());
    let rebuilt: Value =
        serde_json::from_str(&fx.mq_ok(&["stats", "--reconstruct", "--json"])).unwrap();
    for key in ["widths", "success_by_width", "landed", "culprits"] {
        assert_eq!(rebuilt[key], stats[key], "{key}: {rebuilt}");
    }
    assert_eq!(rebuilt["ready_to_land_s"]["count"], 3, "{rebuilt}");
    assert_eq!(rebuilt["reconstructed"], true);
    assert_eq!(rebuilt["cpu_known"], 0);
    let missing = rebuilt["not_reconstructable"].to_string();
    assert!(
        missing.contains("cpu_s") && missing.contains("speculation"),
        "{missing}"
    );
    let text = fx.mq_ok(&["stats", "--reconstruct"]);
    assert!(text.contains("Not reconstructable:"), "{text}");
}

#[test]
fn decision_features_use_the_shared_classifier_policy_at_the_plan_base() {
    let fx = Fixture::new("run-classifier");
    fx.write("tests/t0", "ok\n");
    fx.write(
        "policy.json",
        r#"{"version": 2,
            "force_full": ["ci/**"],
            "ci_irrelevant": ["docs/**"],
            "footprints": [
              {"paths": ["tests/**"], "nodes": ["test.suite"], "component": "suite"},
              {"paths": ["src/**"], "nodes": ["test.lib"]}
            ]}"#,
    );
    fx.commit_all("seed tests and policy");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        RUNNER,
        "--max-batch",
        "4",
        "--ci-path",
        "ci/",
        "--classifier-policy",
        "policy.json",
    ]);
    fx.branch("ci", &[("ci/run.sh", "echo\n")]);
    fx.branch(
        "multi",
        &[
            ("tests/m", "ok\n"),
            ("src/lib.rs", "x\n"),
            ("docs/m.md", "m\n"),
        ],
    );
    fx.branch("edit-a", &[("shared.txt", "one\n2\n3\n4\n5\n")]);
    fx.branch("edit-b", &[("shared.txt", "uno\n2\n3\n4\n5\n")]);
    for (pr, name) in [(1, "ci"), (2, "multi"), (3, "edit-a"), (4, "edit-b")] {
        fx.mq_ok(&["submit", &pr.to_string(), "--head", name]);
    }
    let r = report(&fx.mq_ok(&["run", "--json"]));
    assert_eq!(landed_prs(&r), vec![vec![1, 2, 3]], "{r}");
    let records = decision_records(&fx);
    let features = |pr: i64| -> Value {
        records[0]["changes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["pr"] == pr)
            .map(|c| c["features"].clone())
            .unwrap()
    };
    let ci = features(1);
    assert_eq!(ci["ci_only"], true, "{ci}");
    assert_eq!(ci["classification"]["verdict"], "full", "{ci}");
    assert_eq!(ci["classification"]["only_force_full"], true, "{ci}");
    assert_eq!(ci["components"], serde_json::json!([]), "{ci}");
    let multi = features(2);
    assert_eq!(multi["ci_only"], false);
    assert_eq!(multi["files"], 3, "{multi}");
    assert_eq!(
        multi["components"],
        serde_json::json!(["src", "suite"]),
        "{multi}"
    );
    assert_eq!(multi["classification"]["verdict"], "selective", "{multi}");
    assert_eq!(multi["classification"]["unknown"], 0, "{multi}");
    // #3 and #4 rewrite the same line: #3 lands and recorded its conflict.
    let conflicting = features(3);
    assert_eq!(conflicting["conflict_degree"], 1, "{conflicting}");
    assert_eq!(
        conflicting["classification"]["verdict"], "full",
        "{conflicting}"
    );
    assert_eq!(conflicting["classification"]["unknown"], 1, "{conflicting}");
}

#[test]
fn infra_blocked_batch_is_held_for_a_ruling_neither_landed_nor_ejected() {
    // The validator says the host refused every measurement of `a` on a run and its rerun
    // (INFRA-BLOCKED, exit 6): no result, so the batch is held, never landed and never
    // ejected as red.
    let fx = setup_validator(
        "run-infra-blocked",
        "echo \"INFRA-BLOCKED: a\"; exit 6",
        &[],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(
        r["stopped"].as_str().unwrap().contains("INFRA-BLOCKED"),
        "{}",
        r["stopped"]
    );
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    assert_eq!(item["flaky_nodes"], serde_json::json!(["a"]), "{item}");
    assert_eq!(item["hold_verdict"], "INFRA-BLOCKED", "{item}");
    let history_kinds: Vec<&str> = item["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(
        history_kinds.contains(&"held-infra-blocked"),
        "{history_kinds:?}"
    );
}
