//! The golden vectors, and `change-classify` end to end.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "change-classify-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn classify_bin(args: &[&str], cwd: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_change-classify"))
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The sample policy from the vectors, written to a file.
fn sample_policy(dir: &Path) -> PathBuf {
    let vectors: Value = serde_json::from_str(change_classifier::VECTORS).unwrap();
    let path = dir.join("policy.json");
    std::fs::write(&path, vectors["policies"]["sample"].to_string()).unwrap();
    path
}

#[test]
fn the_shipped_vectors_all_pass() {
    let (checked, failures) = change_classifier::check_vectors(change_classifier::VECTORS).unwrap();
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(checked >= 40, "only {checked} cases");
    let out = classify_bin(&["check-vectors"], Path::new("."));
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(", 0 failed"));
}

#[test]
fn a_changed_expectation_fails_by_name() {
    let dir = temp("vectors");
    let tampered = change_classifier::VECTORS.replace(
        r#"{"pattern": "*.md", "path": "docs/x.md", "match": false"#,
        r#"{"pattern": "*.md", "path": "docs/x.md", "match": true"#,
    );
    assert_ne!(tampered, change_classifier::VECTORS);
    let file = dir.join("vectors.json");
    std::fs::write(&file, tampered).unwrap();
    let out = classify_bin(&["check-vectors", file.to_str().unwrap()], &dir);
    assert_eq!(out.status.code(), Some(1));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains(r#"FAIL glob "*.md" vs "docs/x.md""#),
        "{text}"
    );
}

#[test]
fn glob_answers_with_its_exit_status() {
    let here = Path::new(".");
    assert!(classify_bin(&["glob", "docs/**", "docs/a/b.md"], here)
        .status
        .success());
    assert_eq!(
        classify_bin(&["glob", "*.md", "docs/x.md"], here)
            .status
            .code(),
        Some(1)
    );
}

#[test]
fn classify_reads_a_change_from_git_and_counts_renames_at_both_ends() {
    let dir = temp("git");
    let policy = sample_policy(&dir);
    let repo = dir.join("repo");
    std::fs::create_dir_all(repo.join("core/src")).unwrap();
    std::fs::write(repo.join("core/src/old.rs"), "fn old() {}\n".repeat(20)).unwrap();
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    git(&repo, &["checkout", "-q", "-b", "change"]);
    std::fs::create_dir_all(repo.join("engine-fast/src")).unwrap();
    git(&repo, &["mv", "core/src/old.rs", "engine-fast/src/new.rs"]);
    git(&repo, &["commit", "-q", "-m", "move"]);
    let out = classify_bin(
        &[
            "classify",
            "--policy",
            policy.to_str().unwrap(),
            "--repo",
            repo.to_str().unwrap(),
            "--base",
            "main",
            "--head",
            "change",
            "--json",
        ],
        &dir,
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["verdict"], "selective", "{got}");
    assert_eq!(
        got["components"],
        serde_json::json!(["backend:fast", "core"])
    );
    let paths: Vec<&str> = got["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["core/src/old.rs", "engine-fast/src/new.rs"]);
}

#[test]
fn classify_reads_paths_from_arguments_or_standard_input() {
    let dir = temp("paths");
    let policy = sample_policy(&dir);
    let out = classify_bin(
        &[
            "classify",
            "--policy",
            policy.to_str().unwrap(),
            "--paths",
            "docs/a.md",
            "README.md",
        ],
        &dir,
    );
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("skip\n"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_change-classify"))
        .args([
            "classify",
            "--policy",
            policy.to_str().unwrap(),
            "--paths",
            "-",
            "--json",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"M\tcli/src/main.rs\nA\tscripts/new.sh\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["verdict"], "full");
    assert_eq!(got["unknown"], serde_json::json!(["scripts/new.sh"]));
    // A bare path is a path whatever letter it starts with: `Cargo.lock` is not a copy status,
    // and dropping it would select only the command-line tests.
    let mut child = Command::new(env!("CARGO_BIN_EXE_change-classify"))
        .args([
            "classify",
            "--policy",
            policy.to_str().unwrap(),
            "--paths",
            "-",
            "--json",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Cargo.lock\ncli/src/main.rs\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let got: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(got["verdict"], "full", "{got}");
    assert!(
        got["reasons"]
            .to_string()
            .contains("Cargo.lock → force_full"),
        "{got}"
    );
    // No paths at all is a usage error, not a verdict.
    let out = classify_bin(&["classify", "--policy", policy.to_str().unwrap()], &dir);
    assert_eq!(out.status.code(), Some(2));
}
