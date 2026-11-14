//! End-to-end `ledger verdicts` through the real CLI path.
//!
//! A direct call to the predicate cannot catch a front door that stops
//! forwarding rows to it, so these tests run the binary over a temporary
//! git ledger holding synthetic rows and read the printed verdicts.
//! Temporary repositories only; no real ledger is read.

use std::path::Path;
use std::process::{Command, Output};

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "t@example.com")
        .env("GIT_CONFIG_KEY_1", "user.name")
        .env("GIT_CONFIG_VALUE_1", "t")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn hub(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_local-ci-hub"))
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("binary runs")
}

/// One synthetic count-based row with no cell evidence, no coverage
/// object, no base-evidence fields and no full-coverage claim: the
/// legacy shape. Only the legacy clause qualifies it.
fn legacy_row(commit: &str, executed: i64, passed: i64) -> String {
    format!(
        "{{\"run_id\":\"run-legacy\",\"commit\":\"{commit}\",\"commit_anchored\":true,\"tree_dirty\":false,\
         \"profile\":\"full\",\"selection_mode\":\"full\",\"result\":\"pass\",\"schema_version\":5,\
         \"failures\":0,\"executed_tests\":{executed},\"passed_tests\":{passed},\"filtered_tests\":0,\
         \"gates_run\":5,\"gates_expected\":5,\"host\":\"host-1\",\"finished_at\":\"2026-11-01T00:01:00Z\"}}"
    )
}

fn ledger_with_rows(tag: &str, rows: &[(&str, &str)]) -> std::path::PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "local-ci-hub-verdicts-e2e-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("ledger/team-a/host-1")).unwrap();
    git(&repo, &["init", "-q", "-b", "main", "."]);
    let mut lines = String::new();
    for (index, (commit, row)) in rows.iter().enumerate() {
        lines.push_str(&format!(
            "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e{index}\",\"event_type\":\"run.result\",\
             \"emitted_at\":\"2026-11-01T00:01:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"run-{index}\",\
             \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
             \"commit\":\"{commit}\",\"outcome\":\"pass\",\"legacy_row\":{row}}}\n"
        ));
    }
    std::fs::write(repo.join("ledger/team-a/host-1/2026-11.jsonl"), lines).unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: ledger"]);
    repo
}

fn verdict_for(out: &Output, commit: &str) -> (bool, String) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    for line in text.lines() {
        let value = local_ci_hub::pyjson::parse(line).expect("verdict line parses");
        if value.get("commit").and_then(|v| v.as_str()) == Some(commit) {
            return (
                matches!(
                    value.get("qualified"),
                    Some(local_ci_hub::pyjson::PyValue::Bool(true))
                ),
                value
                    .get("verdict")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned(),
            );
        }
    }
    panic!("no verdict for {commit} in {text}");
}

#[test]
fn ledger_verdicts_qualifies_legacy_count_row_and_refuses_failed_counts() {
    let good_commit = "a".repeat(40);
    let bad_commit = "b".repeat(40);
    let repo = ledger_with_rows(
        "legacy",
        &[
            (good_commit.as_str(), &legacy_row(&good_commit, 10, 10)),
            (bad_commit.as_str(), &legacy_row(&bad_commit, 10, 9)),
        ],
    );
    let out = hub(&repo, &["ledger", "verdicts"]);
    let (qualified, verdict) = verdict_for(&out, &good_commit);
    assert!(qualified, "legacy count row must qualify: {verdict}");
    assert_eq!(verdict, "qualified");
    let (qualified, verdict) = verdict_for(&out, &bad_commit);
    assert!(!qualified, "failed counts must stay refused: {verdict}");
    assert_eq!(verdict, "not-qualified");
    let _ = std::fs::remove_dir_all(&repo);
}
