//! The per-PR signal log: submits record heads, runs record validated signals with
//! attribution, external producers add their kinds, backfill is idempotent, and
//! export sends each line once.

mod support;

use serde_json::Value;
use support::{git, Fixture};

fn signals(fx: &Fixture, pr: i64) -> Vec<Value> {
    let out = fx.mq_ok(&["signal", "list", &pr.to_string(), "--json"]);
    out.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn submit_run_add_backfill_and_export() {
    let fx = Fixture::new("signal");
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
        "for t in $(ls tests); do printf '{\"id\":\"%s\",\"outcome\":\"pass\"}\n' \"$t\" >> \"$MQ_RESULTS\"; done",
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    let after_submit = signals(&fx, 1);
    assert_eq!(after_submit.len(), 1, "{after_submit:?}");
    assert_eq!(after_submit[0]["kind"], "head");

    let out = fx.mq(&["run"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let after_run = signals(&fx, 1);
    let validated: Vec<&Value> = after_run
        .iter()
        .filter(|s| s["kind"] == "validated")
        .collect();
    assert_eq!(validated.len(), 1, "{after_run:?}");
    assert_eq!(validated[0]["verdict"], "green");
    assert_eq!(validated[0]["attribution"], "cleared");
    assert_eq!(validated[0]["batch"], serde_json::json!([1]));

    let out = fx.mq_ok(&[
        "signal",
        "add",
        "1",
        "--kind",
        "focused-tests",
        "--json",
        "{\"actor\":\"dev\",\"head\":\"abc\",\"tests\":[\"a\"],\"outcome\":\"pass\"}",
    ]);
    assert!(out.contains("recorded focused-tests for #1"), "{out}");
    // The queue's own kinds cannot be forged by external producers.
    let out = fx.mq(&["signal", "add", "1", "--kind", "validated", "--json", "{}"]);
    assert!(!out.status.success(), "{out:?}");

    let out = fx.mq_ok(&["signal", "backfill"]);
    assert!(
        out.contains("backfilled 0"),
        "backfill must be idempotent here: {out}"
    );
    let export_dir = fx.root.join("exported");
    let out = fx.mq_ok(&["signal", "export", "--to", export_dir.to_str().unwrap()]);
    assert!(out.contains("exported 3 signal(s)"), "{out}");
    let out = fx.mq_ok(&["signal", "export", "--to", export_dir.to_str().unwrap()]);
    assert!(out.contains("exported 0 signal(s)"), "{out}");
}
