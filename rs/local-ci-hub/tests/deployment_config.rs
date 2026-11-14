//! Deployment config: the committed example parses and maps to flags.

use local_ci_hub::deployment::DeploymentConfig;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("deployment.example.json")
}

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

fn hub(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_local-ci-hub"))
        .args(args)
        .output()
        .expect("binary runs")
}

#[test]
fn example_config_parses_and_maps_to_documented_flags() {
    let text = std::fs::read_to_string(example_path()).expect("example config is readable");
    let config = DeploymentConfig::from_json(&text).expect("example config parses");
    // The file on disk loads through the same entry point the binary uses.
    let loaded = DeploymentConfig::load(&example_path()).expect("example config loads");
    assert_eq!(config, loaded);
    assert_eq!(
        config.flags(),
        vec![
            "--repo",
            "<ledger-repo-path>",
            "--git-ref",
            "<ledger-head-ref>",
            "--evidence-root",
            "<evidence-archive-root>",
            "--binding-tree-field",
            "<binding-tree-field>",
            "--base-field",
            "<base-field-1>",
            "--base-field",
            "<base-field-2>",
            "--fixture-producer",
            "<fixture-producer-1>",
            "--evidence-head-field",
            "<evidence-head-field>",
        ],
        "example must map to the documented flags in help order; \
         a null index emits no --index flag"
    );
    // Placeholders only: no value is a real path, host or field name.
    for value in [
        config.repo.as_deref(),
        config.git_ref.as_deref(),
        config.evidence_root.as_deref(),
        config.binding_tree_field.as_deref(),
        config.evidence_head_field.as_deref(),
    ] {
        let value = value.expect("example sets every scalar placeholder");
        assert!(
            value.starts_with('<') && value.ends_with('>'),
            "example value {value} is not a placeholder"
        );
    }
    assert_eq!(config.index, None);
}

#[test]
fn empty_config_keeps_defaults_and_emits_no_flags() {
    let config = DeploymentConfig::from_json("{}").expect("empty config parses");
    assert!(config.flags().is_empty());
    assert_eq!(config, DeploymentConfig::default());
}

#[test]
fn config_refuses_unknown_empty_and_wrong_typed_values() {
    let unknown = DeploymentConfig::from_json(r#"{"repo":".","surprise":true}"#)
        .expect_err("unknown key must refuse");
    assert!(unknown.contains("invalid deployment config"), "{unknown}");
    let empty = DeploymentConfig::from_json(r#"{"binding_tree_field":""}"#)
        .expect_err("empty field name must refuse");
    assert!(empty.contains("binding_tree_field"), "{empty}");
    let wrong_type = DeploymentConfig::from_json(r#"{"base_fields":"one"}"#)
        .expect_err("wrong-typed list must refuse");
    assert!(
        wrong_type.contains("invalid deployment config"),
        "{wrong_type}"
    );
}

#[test]
fn binary_reads_config_file_and_flag_overrides_config() {
    // Temporary git ledger only; no real ledger is touched.
    let repo = std::env::temp_dir().join(format!("local-ci-hub-deploy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("ledger/team-a/host-1")).unwrap();
    git(&repo, &["init", "-q", "-b", "main", "."]);
    let commit = "a".repeat(40);
    let row = format!(
        "{{\"run_id\":\"run-1\",\"commit\":\"{commit}\",\"commit_anchored\":true,\"tree_dirty\":false,\
         \"profile\":\"full\",\"selection_mode\":\"full\",\"result\":\"pass\",\"schema_version\":5,\
         \"failures\":0,\"executed_tests\":10,\"passed_tests\":10,\"filtered_tests\":0,\
         \"gates_run\":5,\"gates_expected\":5,\"checks\":5,\"host\":\"host-1\",\
         \"finished_at\":\"2026-11-01T00:01:00Z\",\"base_sha\":\"{commit}\",\"base_tree\":\"{commit}\",\
         \"coverage\":{{\"planned_test_nodes\":3,\"executed_test_nodes\":3,\"zero_executed_nodes\":[],\"absent_nodes\":[]}}}}"
    );
    let line = format!(
        "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e1\",\"event_type\":\"run.result\",\
         \"emitted_at\":\"2026-11-01T00:01:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"run-1\",\
         \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
         \"commit\":\"{commit}\",\"outcome\":\"pass\",\"legacy_row\":{row}}}"
    );
    std::fs::write(
        repo.join("ledger/team-a/host-1/2026-11.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: ledger"]);

    // The config alone selects the temporary repository and head.
    let config_path = repo.join("deployment.json");
    std::fs::write(
        &config_path,
        format!(
            "{{\"repo\":{},\"git_ref\":\"HEAD\"}}",
            serde_json::to_string(&repo.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    let out = hub(&[
        "--config".to_owned(),
        config_path.display().to_string(),
        "ledger".to_owned(),
        "rows".to_owned(),
        "--terminals".to_owned(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);

    // An explicit flag wins over the config: a config naming a missing
    // repository still succeeds when --repo names the real one.
    let missing = repo.join("does-not-exist");
    std::fs::write(
        &config_path,
        format!(
            "{{\"repo\":{},\"git_ref\":\"HEAD\"}}",
            serde_json::to_string(&missing.display().to_string()).unwrap()
        ),
    )
    .unwrap();
    let out = hub(&[
        "--config".to_owned(),
        config_path.display().to_string(),
        "--repo".to_owned(),
        repo.display().to_string(),
        "ledger".to_owned(),
        "rows".to_owned(),
        "--terminals".to_owned(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // A malformed config is a usage error (exit 2), never a silent run.
    std::fs::write(&config_path, "{\"unknown_key\":true}").unwrap();
    let out = hub(&[
        "--config".to_owned(),
        config_path.display().to_string(),
        "ledger".to_owned(),
        "verdicts".to_owned(),
    ]);
    assert_eq!(out.status.code(), Some(2));

    let _ = std::fs::remove_dir_all(&repo);
}
