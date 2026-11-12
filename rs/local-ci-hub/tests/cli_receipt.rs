//! CLI receipt fixtures over a synthetic git ledger repository.

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

fn qualifying_row(commit: &str) -> String {
    format!(
        "{{\"run_id\":\"run-1\",\"commit\":\"{commit}\",\"commit_anchored\":true,\"tree_dirty\":false,\
         \"profile\":\"full\",\"selection_mode\":\"full\",\"result\":\"pass\",\"schema_version\":5,\
         \"failures\":0,\"executed_tests\":10,\"passed_tests\":10,\"filtered_tests\":0,\
         \"gates_run\":5,\"gates_expected\":5,\"checks\":5,\"host\":\"host-1\",\
         \"finished_at\":\"2026-11-01T00:01:00Z\",\"base_sha\":\"{commit}\",\"base_tree\":\"{commit}\",\
         \"coverage\":{{\"planned_test_nodes\":3,\"executed_test_nodes\":3,\"zero_executed_nodes\":[],\"absent_nodes\":[]}}}}"
    )
}

#[test]
fn receipt_show_qualified_and_no_result() {
    let repo = std::env::temp_dir().join(format!("local-ci-hub-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("ledger/team-a/host-1")).unwrap();
    git(&repo, &["init", "-q", "-b", "main", "."]);
    let commit = "a".repeat(40);
    let line = [
        "{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e1\",\"event_type\":\"run.result\",",
        "\"emitted_at\":\"2026-11-01T00:01:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"run-1\",",
        "\"producer\":{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"},",
        "\"commit\":\"",
        commit.as_str(),
        "\",\"outcome\":\"pass\",\"legacy_row\":",
        qualifying_row(&commit).as_str(),
        "}",
    ]
    .concat();
    std::fs::write(
        repo.join("ledger/team-a/host-1/2026-11.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: ledger"]);

    let out = hub(&repo, &["--json", "receipt", "show", &commit]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("\"verdict\":\"qualified\""), "{text}");
    assert!(text.contains("\"authority\":\"git-lcd\""), "{text}");

    let missing = "b".repeat(40);
    let out = hub(&repo, &["receipt", "show", &missing]);
    assert_eq!(out.status.code(), Some(2), "no-result exits 2");

    let out = hub(&repo, &["ledger", "rows", "--terminals"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("\"result\":\"pass\""), "{text}");
    assert_eq!(text.lines().count(), 1);

    let out = hub(&repo, &["ledger", "digest-rows"]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("\"run_id\":\"run-1\""), "{text}");

    let _ = std::fs::remove_dir_all(&repo);
}
