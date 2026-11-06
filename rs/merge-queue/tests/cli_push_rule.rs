//! A GH013 repository-rule push rejection is permanent for that candidate:
//! one attempt, ejected quoting the rule, other flights unaffected, never
//! retried. `submit --exact` of a merge commit is refused early when the
//! branch rules require linear history, unless `exact_merge_allowed`.

mod support;

use std::path::PathBuf;

use serde_json::Value;
use support::{git, Fixture};

fn config_path(fx: &Fixture) -> PathBuf {
    let common = git(&fx.queue(), &["rev-parse", "--git-common-dir"]);
    let common = if std::path::Path::new(&common).is_absolute() {
        PathBuf::from(common)
    } else {
        fx.queue().join(common)
    };
    common.join("merge-queue").join("config.json")
}

fn edit_config(fx: &Fixture, edit: impl FnOnce(&mut Value)) {
    let path = config_path(fx);
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut value);
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn status(fx: &Fixture, pr: i64) -> String {
    let items: Value = serde_json::from_str(&fx.mq_ok(&["list", "--all", "--json"])).unwrap();
    items
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["pr"] == pr)
        .map(|item| item["status"].as_str().unwrap().to_owned())
        .unwrap_or_else(|| "missing".to_owned())
}

#[test]
fn a_gh013_push_is_permanent_one_attempt_and_the_run_continues() {
    let fx = Fixture::new("push-rule-gh013");
    fx.write("tests/t0", "ok\n");
    fx.commit_all("seed tests");
    git(&fx.dev(), &["push", "-q", "origin", "HEAD:main"]);
    let pushes = fx.root.join("pushes");
    fx.mq_ok(&[
        "init",
        "--remote",
        "origin",
        "--validator",
        "command",
        "--validator-run",
        "exit 0",
        "--max-batch",
        "1",
    ]);
    // First push (change a's candidate) is rejected GH013; later pushes land.
    edit_config(&fx, |value| {
        value["net_wrapper"] = serde_json::json!([
            "sh",
            "-c",
            format!(
                r#"for a in "$@"; do if [ "$a" = push ]; then echo push >> '{p}'; n=$(wc -l < '{p}'); if [ $n -le 1 ]; then echo 'remote: error: GH013: Repository rule violations found for refs/heads/main' >&2; echo 'remote: - This branch must not contain merge commits. Found 1 violation' >&2; echo ' ! [remote rejected] main (push declined due to repository rule violations)' >&2; exit 1; fi; fi; done; exec "$@""#,
                p = pushes.display()
            ),
            "wrap"
        ]);
        value["land"]["push_retry_backoff_secs"] = serde_json::json!(1);
    });
    fx.branch("a", &[("tests/a", "ok\n")]);
    fx.branch("b", &[("tests/b", "ok\n")]);
    fx.mq_ok(&["submit", "1", "--head", "a"]);
    fx.mq_ok(&["submit", "2", "--head", "b"]);
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let r: Value = serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(status(&fx, 1), "ejected");
    assert_eq!(status(&fx, 2), "landed", "other flights unaffected: {r}");
    let ejected = &r["ejected"][0];
    assert_eq!(ejected["pr"], 1, "{r}");
    assert_eq!(ejected["kind"], "push-rule-violation", "{r}");
    assert!(
        ejected["reason"]
            .as_str()
            .unwrap()
            .contains("must not contain merge commits"),
        "{ejected}"
    );
    let count = std::fs::read_to_string(&pushes).map_or(0, |t| t.lines().count());
    assert_eq!(
        count, 2,
        "one GH013 attempt for #1, one landing push for #2"
    );
    // Never retried in a later run.
    let out = fx.mq(&["run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let count = std::fs::read_to_string(&pushes).map_or(0, |t| t.lines().count());
    assert_eq!(count, 2, "no retry in a later run");
}

/// Create a merge-commit branch `mergebr` in the fixture and return its head.
fn merge_branch(fx: &Fixture) -> String {
    git(&fx.dev(), &["checkout", "-q", "-B", "side", "main"]);
    fx.write("side.txt", "side\n");
    fx.commit_all("side");
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:side"]);
    git(&fx.dev(), &["checkout", "-q", "-B", "mergebr", "main"]);
    git(
        &fx.dev(),
        &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
    );
    git(&fx.dev(), &["push", "-q", "-f", "origin", "HEAD:mergebr"]);
    let head = git(&fx.dev(), &["rev-parse", "HEAD"]);
    git(&fx.dev(), &["checkout", "-q", "main"]);
    head
}

fn setup_exact(tag: &str) -> Fixture {
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
        "exit 0",
        "--github-repo",
        "o/r",
    ]);
    // A fake `gh` whose rules answer requires linear history.
    edit_config(&fx, |value| {
        value["github"]["gh"] = serde_json::json!([
            "sh",
            "-c",
            "echo '[{\"type\":\"required_linear_history\",\"ruleset_id\":20307165}]'",
            "gh"
        ]);
    });
    fx
}

#[test]
fn submit_exact_of_a_merge_is_refused_when_rules_require_linear_history() {
    let fx = setup_exact("exact-rules-refuse");
    merge_branch(&fx);
    let out = fx.mq(&["submit", "1", "--head", "mergebr", "--exact"]);
    assert!(!out.status.success(), "{out:?}");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.contains("required_linear_history"), "{text}");
    assert!(text.contains("must not contain merge commits"), "{text}");
    assert_eq!(status(&fx, 1), "missing", "refused before queueing: {text}");
    // The operator escape allows it while a ruleset exception is in place.
    edit_config(&fx, |value| {
        value["exact_merge_allowed"] = serde_json::json!(true);
    });
    fx.mq_ok(&["submit", "1", "--head", "mergebr", "--exact"]);
    assert_eq!(status(&fx, 1), "queued");
}

#[test]
fn submit_exact_warns_when_branch_rules_cannot_be_read() {
    let fx = setup_exact("exact-rules-warn");
    edit_config(&fx, |value| {
        value["github"]["gh"] = serde_json::json!(["sh", "-c", "exit 9", "gh"]);
    });
    merge_branch(&fx);
    let output = fx.mq(&["submit", "1", "--head", "mergebr", "--exact"]);
    assert!(output.status.success(), "{output:?}");
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out.contains("warn: exact merge"), "{out}");
    assert!(out.contains("branch rules not read"), "{out}");
    assert_eq!(status(&fx, 1), "queued");
}
