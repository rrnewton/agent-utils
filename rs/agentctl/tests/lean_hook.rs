//! `agentctl lean-hook` as Claude Code runs it (#2322): the PreToolUse input
//! on stdin, a deny decision (or nothing) on stdout, exit 0; an input it
//! cannot read exits non-zero, which the rendered settings' `|| exit 2`
//! turns into a blocked call.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn scratch_registry() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "agentctl-lean-hook-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn hook(registry: &PathBuf, input: &str) -> (Option<i32>, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .arg("--registry")
        .arg(registry)
        .args([
            "--herdr-bin",
            "/nonexistent/herdr",
            "lean-hook",
            "--agent",
            "lead",
            "--bash-timeout",
            "30",
            "--lane",
            "lead-hands",
        ])
        .env("HOME", registry)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn the_hook_passes_reads_denies_writes_and_fails_on_bad_input() {
    let registry = scratch_registry();
    let (code, stdout, stderr) = hook(
        &registry,
        &json!({"tool_name": "Bash", "tool_input": {"command": "git log -1 && cat x"}}).to_string(),
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.trim().is_empty(), "a pass prints nothing: {stdout}");

    let (code, stdout, stderr) = hook(
        &registry,
        &json!({"tool_name": "Bash", "tool_input": {"command": "rm -rf build"}}).to_string(),
    );
    assert_eq!(code, Some(0), "{stderr}");
    let decision: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(decision["hookSpecificOutput"]["permissionDecision"], "deny");
    let reason = decision["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap();
    assert!(
        reason.contains("agentctl send lead-hands --queue-only"),
        "{reason}"
    );

    // A send to a lane the hook lists passes without reading any record; one to an agent
    // no managed_by edge ties to the lead is refused (the scratch registry has no records).
    let (code, stdout, _) = hook(
        &registry,
        &json!({"tool_name": "Bash",
                "tool_input": {"command": "agentctl send lead-hands --queue-only 'H1: build'"}})
        .to_string(),
    );
    assert_eq!(code, Some(0));
    assert!(stdout.trim().is_empty(), "{stdout}");
    let (code, stdout, _) = hook(
        &registry,
        &json!({"tool_name": "Bash",
                "tool_input": {"command": "agentctl send stranger --queue-only hi"}})
        .to_string(),
    );
    assert_eq!(code, Some(0));
    assert!(stdout.contains("is not one of your lanes"), "{stdout}");

    let (code, _, stderr) = hook(&registry, "not json");
    assert_ne!(code, Some(0), "bad input must not pass");
    assert!(stderr.contains("not JSON"), "{stderr}");
    std::fs::remove_dir_all(&registry).unwrap();
}
