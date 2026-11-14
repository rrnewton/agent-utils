//! End-to-end `ledger digest-rows` through the real CLI path.
//!
//! A direct call to the digest library cannot catch a front door that
//! stops forwarding the deployment head field (or otherwise leaves the
//! typed canonicalizer unreached), so these tests run the binary over a
//! temporary git ledger holding one synthetic nested record — typed cell
//! evidence, a coverage value with a sub-object, and gates carrying
//! attempt lists — and compare its output with the library digest of
//! the same row. Temporary repositories only; no real ledger is read.

use local_ci_hub::digest::receipt_digest_with;
use local_ci_hub::pyjson::parse;
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

fn hub(repo: &Path, global: &[&str], args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_local-ci-hub"))
        .arg("--repo")
        .arg(repo)
        .args(global)
        .args(args)
        .output()
        .expect("binary runs")
}

/// One nested record; `head_field` names the validated-head key inside
/// its typed `cell_results` evidence.
fn nested_row(head_field: &str) -> String {
    concat!(
        "{\"schema_version\":7,\"run_id\":\"run-1\",\"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",",
        "\"cell_results\":{\"cells\":[{\"backend\":\"b1\",\"mode\":\"m1\",\"test\":\"t1\",",
        "\"category\":\"c1\",\"lane\":\"l1\",\"cell_verdict\":{\"bitwise_parity\":true,",
        "\"compared_log_messages\":{\"right\":2,\"left\":1},\"comparison\":{",
        "\"skip_detlog\":false,\"skip_commit\":true,\"ignore_lines\":false,",
        "\"canonicalizations\":[],\"stripped_prefixes\":[\"p\"],\"exact_remainder\":true,",
        "\"full_trace\":false,\"canonicalize_addresses\":true,\"strip_lines\":false,",
        "\"log_scope\":\"info\",\"virtualize_time\":null,\"record_envelope\":\"all\",",
        "\"compare_io_buffers\":true,\"compare_logs\":true,\"display_name\":\"Spec\",",
        "\"strictness\":\"canonical\"},\"comparison_tier\":\"canonical-bitwise\",",
        "\"state\":\"compared-and-matched\"}}],",
        "\"selected\":[{\"test\":\"t1\",\"backend\":\"b1\",\"lane\":\"l1\",\"mode\":\"m1\",\"category\":\"c1\"}],",
        "\"artifact\":{\"row_count\":3,\"sha256\":\"s\",\"path\":\"a/b.jsonl\"},",
        "\"population_sha256\":\"pop\",\"recorded_count\":1,\"selected_count\":1,",
        "\"source_tree_dirty\":false,\"HEAD_FIELD\":\"hhhh\",\"run_id\":\"run-1\"},",
        "\"coverage\":{\"artifact\":{\"path\":\"cov.json\",\"sha256\":\"cs\",\"row_count\":9},",
        "\"planned_test_nodes\":20,\"executed_test_nodes\":20,\"zero_executed_nodes\":[],\"absent_nodes\":[]},",
        "\"gates\":[{\"name\":\"g1\",\"result\":\"pass\",\"attempts\":[{\"attempt\":1,\"real_seconds\":1.5,\"nested\":{\"y\":1,\"x\":\"café\"}}]}]}"
    )
    .replace("HEAD_FIELD", head_field)
}

fn ledger_with_row(tag: &str, row: &str) -> std::path::PathBuf {
    let repo = std::env::temp_dir().join(format!(
        "local-ci-hub-digest-e2e-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(repo.join("ledger/team-a/host-1")).unwrap();
    git(&repo, &["init", "-q", "-b", "main", "."]);
    let line = format!(
        "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"e1\",\"event_type\":\"run.result\",\
         \"emitted_at\":\"2026-11-01T00:01:00Z\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"run-1\",\
         \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
         \"commit\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"outcome\":\"pass\",\"legacy_row\":{row}}}"
    );
    std::fs::write(
        repo.join("ledger/team-a/host-1/2026-11.jsonl"),
        format!("{line}\n"),
    )
    .unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: ledger"]);
    repo
}

fn digest_of(out: &Output) -> String {
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let row = parse(text.lines().next().expect("one digest row")).unwrap();
    row.get("digest")
        .and_then(|value| value.as_str())
        .expect("digest is a string")
        .to_owned()
}

#[test]
fn digest_rows_reaches_typed_canonicalizer_with_default_head_field() {
    let row = nested_row("validated_head_sha");
    let repo = ledger_with_row("default", &row);
    let direct = receipt_digest_with(&parse(&row).unwrap(), "validated_head_sha")
        .expect("row canonicalizes");
    let out = hub(&repo, &[], &["ledger", "digest-rows"]);
    assert_eq!(digest_of(&out), direct, "CLI must reach the typed path");
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("generic sorted form"),
        "no head-field warning when the field is present: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn digest_rows_forwards_a_custom_head_field_and_warns_without_it() {
    let row = nested_row("custom_head_field");
    let repo = ledger_with_row("custom", &row);
    let typed = receipt_digest_with(&parse(&row).unwrap(), "custom_head_field")
        .expect("row canonicalizes with its deployment field");
    let fallback = receipt_digest_with(&parse(&row).unwrap(), "validated_head_sha")
        .expect("row canonicalizes in generic form");
    assert_ne!(typed, fallback, "fixture must separate the two forms");

    // With the deployment field, the CLI prints the typed digest and
    // stays quiet; without it, the same binary prints the generic
    // fallback — unchanged-digest output that looks like a canonicalizer
    // regression — and warns once that the field was never seen.
    let out = hub(
        &repo,
        &["--evidence-head-field", "custom_head_field"],
        &["ledger", "digest-rows"],
    );
    assert_eq!(digest_of(&out), typed);
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("generic sorted form"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = hub(&repo, &[], &["ledger", "digest-rows"]);
    assert_eq!(digest_of(&out), fallback);
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        stderr.contains("evidence-head field 'validated_head_sha' absent from all"),
        "wrong-field run must warn once: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
