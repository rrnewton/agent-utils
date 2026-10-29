//! Build provenance: `agentctl capabilities` reports `build = {edition, source_fp, commit, dirty}`
//! while both executables keep their exact first `--version` line, which the cross-edition differential compares.

use std::process::Command;

use serde_json::{json, Value};

#[test]
fn capabilities_report_the_build_edition_and_fingerprint() {
    let home = std::env::temp_dir().join(format!("agentctl-provenance-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .arg("capabilities")
        .current_dir(&home)
        .env("HOME", &home)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&home);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let document: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document["build"],
        json!({
            "edition": "rust",
            "source_fp": option_env!("AGENTCTL_BUILD_SOURCE_FP"),
            "commit": option_env!("AGENTCTL_BUILD_SOURCE_COMMIT"),
            "dirty": option_env!("AGENTCTL_BUILD_SOURCE_DIRTY").map(|value| value == "1"),
        })
    );
}

#[test]
fn version_lines_are_unchanged() {
    if option_env!("AGENTCTL_BUILD_SOURCE_COMMIT").is_some() {
        // A build that names its commit adds provenance lines; the first line still matches.
        let output = Command::new(env!("CARGO_BIN_EXE_agentctl"))
            .arg("--version")
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.starts_with("agentctl 0.1.0\ncommit "), "{text}");
        return;
    }
    for (binary, expected) in [
        (env!("CARGO_BIN_EXE_agentctl"), "agentctl 0.1.0\n"),
        (env!("CARGO_BIN_EXE_herdr-agent"), "herdr-agent 0.1.0\n"),
    ] {
        let output = Command::new(binary).arg("--version").output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        assert!(output.stderr.is_empty(), "{binary}");
    }
}
