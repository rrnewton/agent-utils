//! Project config (`local-ci-hub.toml`): the committed example loads
//! verbatim, Python-edition defaults and refusals hold, and the
//! binary's read side runs from a TOML config end to end.

use local_ci_hub::project_config::ProjectConfig;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn example_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("local-ci-hub.example.toml")
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
fn example_config_loads_verbatim() {
    let config = ProjectConfig::load(&example_path()).expect("example config loads");
    assert!(config.loaded);
    assert_eq!(config.name, "example");
    assert_eq!(config.team, "team");
    assert_eq!(config.validate_command, vec!["make", "validate"]);
    assert_eq!(config.admission_slots, 2);
    assert_eq!(config.publish, "none");
    assert_eq!(config.provider, "none");
    assert_eq!(config.qualifying, vec!["local-ledger-pass"]);
    assert!(config.require_full_evidence);
    assert!(config.ledger_dir_explicit);
    assert!(config.ledger_dir.ends_with("ci-ledger"));
    assert!(config.counted_repos.is_empty());
}

#[test]
fn python_fixture_shape_parses() {
    // The shape py/tests/test_local_ci_hub.py writes for its toy repo,
    // including the inline location table.
    let text = "[project]\nname=\"toy\"\n[validate]\n\
        command=[\"/bin/sh\",\"-c\",\"true\"]\nadmission_slots=2\n\
        [ledger]\nteam=\"toy\"\npublish=\"none\"\nlocation={path=\"ci-ledger\"}\n\
        [remote]\nprovider=\"none\"\n\
        [receipt]\nqualifying=[\"local-ledger-pass\"]\nrequire_full_evidence=false\n\
        [land]\nstate_dir=\"ignored/local-ci-hub\"\n";
    let config = ProjectConfig::from_toml(text, Path::new("/proj")).expect("fixture parses");
    assert_eq!(config.name, "toy");
    assert_eq!(config.team, "toy");
    assert_eq!(config.ledger_dir, Path::new("/proj/ci-ledger"));
    assert!(!config.require_full_evidence);
    assert_eq!(config.state_dir, Path::new("/proj/ignored/local-ci-hub"));
    assert_eq!(config.producer, "local-ci-hub");
    assert_eq!(config.admission_kinds, vec!["host-limits-live"]);
}

#[test]
fn empty_document_keeps_python_defaults() {
    let config = ProjectConfig::from_toml("", Path::new("/proj")).expect("empty parses");
    assert_eq!(config.name, "example");
    assert_eq!(config.validate_command, vec!["make", "validate"]);
    assert_eq!(config.admission_slots, 2);
    assert_eq!(config.team, "team");
    assert_eq!(config.ledger_dir, Path::new("/proj/ci-ledger"));
    assert!(!config.ledger_dir_explicit);
    assert_eq!(config.qualifying, vec!["local-ledger-pass"]);
}

#[test]
fn refusals_are_named() {
    let root = Path::new("/proj");
    let unknown =
        ProjectConfig::from_toml("[bogus]\nx=1\n", root).expect_err("unknown section must refuse");
    assert!(unknown.contains("invalid project config"), "{unknown}");
    let unknown_key = ProjectConfig::from_toml("[project]\nsurprise=1\n", root)
        .expect_err("unknown key must refuse");
    assert!(
        unknown_key.contains("invalid project config"),
        "{unknown_key}"
    );
    let bad_rule = ProjectConfig::from_toml("[receipt]\nqualifying=[\"magic\"]\n", root)
        .expect_err("unknown qualifying rule must refuse");
    assert!(bad_rule.contains("unknown qualifying rules"), "{bad_rule}");
    let remote = ProjectConfig::from_toml("[receipt]\nqualifying=[\"remote-pass\"]\n", root)
        .expect_err("remote-pass without provider must refuse");
    assert!(remote.contains("remote.provider is none"), "{remote}");
    let digest = ProjectConfig::from_toml("[receipt]\ndigest_command=[\"sha256sum\"]\n", root)
        .expect_err("delegated digest must refuse");
    assert!(digest.contains("digest_command is refused"), "{digest}");
}

#[test]
fn discovery_order_matches_python() {
    let base = std::env::temp_dir().join(format!("local-ci-hub-discover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("ci")).unwrap();
    assert_eq!(ProjectConfig::discover(&base), None);
    std::fs::write(base.join("ci/local-ci-hub.toml"), "").unwrap();
    assert_eq!(
        ProjectConfig::discover(&base),
        Some(base.join("ci/local-ci-hub.toml"))
    );
    std::fs::write(base.join("local-ci-hub.toml"), "").unwrap();
    assert_eq!(
        ProjectConfig::discover(&base),
        Some(base.join("local-ci-hub.toml")),
        "the root file wins over ci/, as in the Python loader"
    );
    let _ = std::fs::remove_dir_all(&base);
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
fn toml_config_drives_receipt_verify_end_to_end() {
    let repo = std::env::temp_dir().join(format!("local-ci-hub-toml-{}", std::process::id()));
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
    // The project config steers --repo through its explicit ledger
    // location; no --repo flag is passed below.
    std::fs::write(
        repo.join("local-ci-hub.toml"),
        "[project]\nname=\"toy\"\n[ledger]\nteam=\"team-a\"\nlocation={path=\".\"}\n",
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: ledger"]);

    let config_arg = format!("--config={}", repo.join("local-ci-hub.toml").display());
    let out = hub(&[
        config_arg.clone(),
        "receipt".into(),
        "verify".into(),
        commit.clone(),
    ]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.starts_with("qualified: "), "{text}");

    let missing = "b".repeat(40);
    let out = hub(&[config_arg, "receipt".into(), "verify".into(), missing]);
    assert_eq!(out.status.code(), Some(2), "no-result exits 2");

    let out = hub(&["quickstart".into()]);
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("receipt verify HEAD"), "{text}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn config_extension_selects_the_format() {
    let out = hub(&[
        "--config=nowhere.yaml".into(),
        "ledger".into(),
        "rows".into(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    let text = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        text.contains(".toml project config or a .json deployment config"),
        "{text}"
    );
}
