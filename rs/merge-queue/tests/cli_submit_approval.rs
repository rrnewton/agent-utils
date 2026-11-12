//! The submit approval gate (#1381 mq-submit-approval-gate): a configured
//! approval command runs on every direct submit with MQ_PR/MQ_HEAD/MQ_BY;
//! a refusal queues nothing and is never cached, an approval queues and
//! is recorded, and a command that cannot run refuses.

mod support;

use serde_json::Value;
use support::{git, Fixture};

/// Every test passes.
const RUNNER: &str = r#"printf '{"id":"t0","outcome":"pass"}\n' >> "$MQ_RESULTS"
exit 0"#;

fn status(fx: &Fixture, pr: i64) -> Option<String> {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["pr"] == pr)
        .map(|i| i["status"].as_str().unwrap().to_owned())
}

fn setup(tag: &str, approval: &str) -> Fixture {
    let fx = Fixture::new(tag);
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
        RUNNER,
        "--submit-approval-command",
        approval,
    ]);
    fx
}

#[test]
fn refusing_approval_queues_nothing_and_is_not_cached() {
    // The gate approves only by=good-lane: a refused submit queues
    // nothing, and resubmitting the same head as good-lane succeeds.
    let fx = setup(
        "submit-approval-refuse",
        "test \"$MQ_BY\" = good-lane || { echo 'no review verdict for this submitter'; exit 1; }",
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    let out = fx.mq(&["submit", "1", "--head", "a", "--by", "bad-lane"]);
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stderr.contains("no review verdict for this submitter")
            || stdout.contains("no review verdict for this submitter"),
        "refusal names the command's reason: stdout={stdout} stderr={stderr}"
    );
    assert_eq!(status(&fx, 1), None, "a refused submit queues nothing");
    // Same head, approved submitter: no cached refusal blocks it.
    fx.mq_ok(&["submit", "1", "--head", "a", "--by", "good-lane"]);
    assert_eq!(status(&fx, 1).as_deref(), Some("queued"));
}

#[test]
fn approving_approval_queues_and_records_the_grant() {
    let fx = setup("submit-approval-grant", "exit 0");
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a", "--by", "lane-1"]);
    assert_eq!(status(&fx, 1).as_deref(), Some("queued"));
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    let item = &items.as_array().unwrap()[0];
    let history = serde_json::to_string(&item["history"]).unwrap();
    assert!(
        history.contains("approval granted"),
        "the grant is in the submission record: {history}"
    );
}

#[test]
fn approval_command_that_cannot_run_refuses() {
    let fx = setup("submit-approval-cannot-run", "exec /no/such/command-xyz");
    fx.branch("a", &[("tests/a", "ok\n")]);
    let out = fx.mq(&["submit", "1", "--head", "a", "--by", "lane-1"]);
    assert!(!out.status.success(), "{out:?}");
    assert_eq!(
        status(&fx, 1),
        None,
        "a gate that cannot run queues nothing"
    );
}

#[test]
fn sync_file_submission_bypasses_the_gate() {
    // Sync submissions answer to the approval-at-head rule, not to this
    // gate: even a refusing command does not block them.
    let fx = setup(
        "submit-approval-sync-exempt",
        "echo 'never for direct submits'; exit 1",
    );
    fx.branch("a", &[("tests/a", "ok\n")]);
    let file = fx.root.join("ready.txt");
    std::fs::write(&file, "1 head=a\n").unwrap();
    let out = fx.mq_ok(&["sync", "--file", file.to_str().unwrap(), "--json"]);
    let r: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(r["submitted"], serde_json::json!([1]), "{r}");
    assert_eq!(status(&fx, 1).as_deref(), Some("queued"));
}

#[test]
fn no_approval_command_keeps_todays_behaviour() {
    let fx = Fixture::new("submit-approval-unset");
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
        RUNNER,
    ]);
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    assert_eq!(status(&fx, 1).as_deref(), Some("queued"));
}
