//! A red run that recorded no test result (no `$MQ_RESULTS` lines, no verdict
//! lines, nothing ran) is, under `validator.empty_red = could-not-run`, a
//! could-not-run refusal: no strike, requeued at the same head, counted against
//! the deferral bound. Under the default `red` it ejects, but named as a red
//! with no recorded test result, never with an empty failed-test list. A probe
//! that is an empty red can name no culprit.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Nothing runs when `bad.txt` is present (empty red); otherwise `t0` passes.
const EMPTY_OR_PASS: &str = r#"if [ -f bad.txt ]; then exit 1; fi
echo '{"id":"t0","outcome":"pass"}' >> "$MQ_RESULTS"; exit 0"#;

/// Every validation records nothing and exits red.
const EMPTY_ALWAYS: &str = "exit 1";

/// `t0` fails (recorded) when `bad.txt` is present; otherwise it passes.
const RED_WITH_RESULTS: &str = r#"if [ -f bad.txt ]; then echo '{"id":"t0","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1; fi
echo '{"id":"t0","outcome":"pass"}' >> "$MQ_RESULTS"; exit 0"#;

/// Empty red when `empty.txt` is present; recorded `t0` failure for `bad.txt`.
const EMPTY_OR_RED: &str = r#"if [ -f empty.txt ]; then exit 1; fi
if [ -f bad.txt ]; then echo '{"id":"t0","outcome":"fail"}' >> "$MQ_RESULTS"; exit 1; fi
echo '{"id":"t0","outcome":"pass"}' >> "$MQ_RESULTS"; exit 0"#;

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

fn report(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| panic!("bad report JSON ({e}):\n{stdout}"))
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

fn item(fx: &Fixture, pr: i64) -> Value {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .unwrap()
        .clone()
}

#[test]
fn empty_red_could_not_run_defers_at_same_head_and_escalates_at_the_bound() {
    let fx = setup_validator(
        "empty-red-could-not-run",
        EMPTY_ALWAYS,
        &[
            "--empty-red",
            "could-not-run",
            "--refusal-max-retries",
            "0",
            "--refusal-backoff-secs",
            "0",
            "--max-deferrals",
            "2",
        ],
    );
    let head = fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let before = fx.remote_main();

    // First empty red: no strike, nothing ejected or landed, still queued at
    // the same head, counted once toward escalation.
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["deferred"][0]["prs"], serde_json::json!([1]), "{r}");
    assert!(
        r["deferred"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("red with no recorded test result"),
        "{r}"
    );
    for v in r["validations"].as_array().unwrap() {
        assert_eq!(v["refused"], true, "{v}");
        assert_eq!(v["failed"], serde_json::json!([]), "{v}");
    }
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    let it = item(&fx, 1);
    assert_eq!(it["head"], serde_json::json!(head), "{it}");
    assert_eq!(it["validation_refusals"], 1, "{it}");
    assert!(!it["history"].to_string().contains("ejected"), "{it}");

    // Second empty red reaches the bound: escalated to the owner, held.
    let out = fx.mq(&["run", "--json"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("ALARM: #1 escalated to the owner"),
        "{stderr}"
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["deferred"][0]["escalated"], serde_json::json!([1]), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(status(&fx, 1), "queued");
    assert!(
        !item(&fx, 1)["escalated"].as_str().unwrap().is_empty(),
        "{}",
        item(&fx, 1)
    );

    // Held: the next run validates nothing, and `retry` releases it.
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["validations"].as_array().unwrap().is_empty(), "{r}");
    fx.mq_ok(&["retry", "1"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["validations"].as_array().unwrap().len(), 1, "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn empty_red_under_red_ejects_with_the_named_event() {
    // Default (`red`): the empty red is the verdict, but its ejection names
    // the empty red instead of an empty failed-test list.
    let fx = setup_validator("empty-red-red", EMPTY_OR_PASS, &[]);
    fx.branch("a", &[("bad.txt", "x\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 1, "{r}");
    assert_eq!(ejected["kind"], "failed-tests", "{r}");
    // No failed tests were recorded, so none are listed.
    assert!(
        ejected
            .get("failed")
            .is_none_or(|f| f.as_array().is_some_and(|f| f.is_empty())),
        "{ejected}"
    );
    let reason = ejected["reason"].as_str().unwrap();
    assert!(
        reason.contains("red with no recorded test result"),
        "{reason}"
    );
    assert!(!reason.contains("failed-tests:"), "{reason}");
    assert_eq!(status(&fx, 1), "ejected");
    let history = item(&fx, 1)["history"].to_string();
    assert!(
        history.contains("red with no recorded test result"),
        "{history}"
    );
}

#[test]
fn red_with_results_ejects_normally_under_both_settings() {
    for (tag, extra) in [
        ("empty-red-results-red", Vec::new()),
        (
            "empty-red-results-cnr",
            vec!["--empty-red", "could-not-run"],
        ),
    ] {
        let fx = setup_validator(tag, RED_WITH_RESULTS, &extra);
        fx.branch("a", &[("bad.txt", "x\n")]);
        fx.mq_ok(&["submit", "1", "--head", "a"]);
        let out = fx.mq(&["run", "--json"]);
        let r = report(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(r["ejected"].as_array().unwrap().len(), 1, "{r}");
        let ejected = &r["ejected"][0];
        assert_eq!(ejected["kind"], "failed-tests", "{r}");
        assert_eq!(ejected["failed"], serde_json::json!(["t0"]), "{r}");
        assert!(ejected["reason"].as_str().unwrap().contains("t0"), "{r}");
        assert_eq!(status(&fx, 1), "ejected");
    }
}

#[test]
fn an_empty_red_probe_names_no_culprit_in_a_multi_change_batch() {
    // With confirmation on, the batch is searched: the prefix carrying
    // `empty.txt` probes as an empty red, which observes nothing, so the
    // search is inconclusive and ejects nobody.
    let fx = setup_validator(
        "empty-red-bisect",
        EMPTY_OR_RED,
        &["--confirm-reruns", "--max-batch", "2"],
    );
    fx.branch("a", &[("empty.txt", "x\n")]);
    fx.branch("b", &[("b.txt", "b\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    let stopped = r["stopped"].as_str().unwrap_or_default().to_owned()
        + &String::from_utf8_lossy(&out.stderr);
    assert!(
        stopped.contains("red with no recorded test result"),
        "{stopped} / {r}"
    );
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(status(&fx, 2), "queued");
}
