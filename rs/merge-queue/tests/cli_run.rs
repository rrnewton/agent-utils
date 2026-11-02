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
