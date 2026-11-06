//! An unparseable results row is could-not-run (host scope), never an error
//! that stops the run: no strike, bounded by the refusal/deferral counts,
//! parsed rows kept for diagnosis, other flights unaffected.

mod support;

use serde_json::Value;
use support::{git, Fixture};

const BAD_ALWAYS: &str =
    r#"echo '{"id":"x","outcome":"fail","duration_ms":-1}' >> "$MQ_RESULTS"; exit 1"#;

const MIXED: &str = r#"echo '{"id":"t0","outcome":"fail"}' >> "$MQ_RESULTS"; echo 'not json at all' >> "$MQ_RESULTS"; exit 1"#;

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
fn a_bad_row_is_could_not_run_bounded_and_never_strikes() {
    let fx = setup_validator(
        "bad-row-bounded",
        BAD_ALWAYS,
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
    let before = fx.remote_main();

    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["landed"].as_array().unwrap().is_empty(), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["deferred"][0]["prs"], serde_json::json!([1]), "{r}");
    let reason = r["deferred"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("bad line"), "{reason}");
    assert!(reason.contains("results"), "{reason}");
    // Not an Err stop: validations are refused, none green or red.
    for v in r["validations"].as_array().unwrap() {
        assert_eq!(v["refused"], true, "{v}");
        assert_eq!(v["failed"], serde_json::json!([]), "{v}");
    }
    assert!(!r["stopped"].as_str().unwrap().starts_with("error:"), "{r}");
    assert_eq!(fx.remote_main(), before);
    assert_eq!(status(&fx, 1), "queued");
    assert_eq!(item(&fx, 1)["validation_refusals"], 1);

    // Deterministic bad row: second round reaches the bound and escalates.
    let out = fx.mq(&["run", "--json"]);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("ALARM: #1 escalated"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(r["deferred"][0]["escalated"], serde_json::json!([1]), "{r}");
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    // Parked: the next run validates nothing (bounded, not forever).
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    assert!(r["validations"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn a_good_row_beside_a_bad_row_is_diagnosis_not_a_verdict() {
    let fx = setup_validator(
        "bad-row-mixed",
        MIXED,
        &["--refusal-max-retries", "0", "--refusal-backoff-secs", "0"],
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let out = fx.mq(&["run", "--json"]);
    let r = report(&String::from_utf8_lossy(&out.stdout));
    // Could-not-run, not red: nothing ejected, validation refused.
    assert!(r["ejected"].as_array().unwrap().is_empty(), "{r}");
    let v = &r["validations"][0];
    assert_eq!(v["refused"], true, "{v}");
    assert_eq!(v["failed"], serde_json::json!([]), "{v}");
    // The parsed good row is visible in the record for diagnosis.
    assert_eq!(
        v["results"]["results"]["t0"],
        serde_json::json!("fail"),
        "{v}"
    );
    assert_eq!(status(&fx, 1), "queued");
}
