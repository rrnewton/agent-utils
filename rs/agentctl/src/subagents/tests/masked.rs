//! `#2163 agentctl-profile-masked-paths`: masked launches refuse when
//! the namespace cannot be entered (fail closed), the per-agent record
//! override wins over the profile, and status names both lists. The
//! hermetic namespace proof itself lives in `crate::masked::tests`
//! (a temporary directory stands in for the masked path).

use super::*;

fn write_masked_profile(fixture: &Fixture) {
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&fixture.root)
        .status()
        .unwrap()
        .success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test launch profiles", false, false).unwrap();
    agent::atomic_json(
        &directory.join("profiles.json"),
        &json!({
            "schema": "agentctl-profiles/v1",
            "profiles": {"masked": {
                "harness": "codex",
                "mode": "interactive",
                "reasoning_effort": "high",
                "argv": ["--yolo"],
                "masked_paths": ["/off/limits"],
                "unset_env": ["EXAMPLE_ROOT"],
            }},
        }),
    )
    .unwrap();
}

#[test]
fn masked_start_refuses_when_the_namespace_cannot_be_entered() {
    // The fake terminal backend cannot enter a mask namespace, so the
    // launch must refuse before any harness starts: fail closed, never
    // a silent unmasked launch.
    let fixture = Fixture::new();
    write_masked_profile(&fixture);
    let error = fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                profile: Some("masked".to_owned()),
                harness_args: vec!["--yolo".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("masked"),
        "launch refused for the mask, got: {error}"
    );
}

#[test]
fn record_override_wins_and_status_names_declared_and_effective() {
    let fixture = Fixture::new();
    write_masked_profile(&fixture);
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                reasoning_effort: Some("high".to_owned()),
                launch: false,
                profile: Some("masked".to_owned()),
                masked_paths: vec!["/off/limits".to_owned()],
                masked_paths_override: Some(vec!["/agent/only".to_owned()]),
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let record = manager.load("lead").unwrap();
    assert_eq!(record.masked_paths, vec!["/agent/only".to_owned()]);
    assert_eq!(
        record.masked_paths_override.as_deref(),
        Some(vec!["/agent/only".to_owned()].as_slice())
    );
    let rows = manager.list().unwrap();
    let row = rows
        .iter()
        .find(|row| row["name"] == "lead")
        .expect("headless row listed");
    assert_eq!(row["declared_masked_paths"], json!(["/off/limits"]));
    assert_eq!(row["effective_masked_paths"], json!(["/agent/only"]));
    assert_eq!(row["masked_paths_source"], "record");
}

#[test]
fn unmasked_record_reports_no_masks() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                reasoning_effort: Some("high".to_owned()),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let rows = manager.list().unwrap();
    let row = rows
        .iter()
        .find(|row| row["name"] == "lead")
        .expect("headless row listed");
    assert_eq!(row["declared_masked_paths"], json!([]));
    assert_eq!(row["effective_masked_paths"], json!([]));
    assert_eq!(row["masked_paths_source"], "none");
}

#[test]
fn no_mcp_profile_renders_strict_mcp_for_claude() {
    // #2163 follow-up: a Claude profile opting into no_mcp_servers
    // launches with agentctl-rendered strict-MCP arguments naming the
    // agentctl-owned empty config under the registry.
    let fixture = Fixture::new();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&fixture.root)
        .status()
        .unwrap()
        .success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test launch profiles", false, false).unwrap();
    agent::atomic_json(
        &directory.join("profiles.json"),
        &json!({
            "schema": "agentctl-profiles/v1",
            "profiles": {"quiet": {
                "harness": "claude",
                "mode": "interactive",
                "reasoning_effort": "high",
                "argv": ["--dangerously-skip-permissions"],
                "no_mcp_servers": true,
            }},
        }),
    )
    .unwrap();
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                profile: Some("quiet".to_owned()),
                harness: "claude".to_owned(),
                no_mcp_servers: true,
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = manager.load("worker").unwrap();
    assert!(
        record
            .arguments
            .iter()
            .any(|arg| arg == "--strict-mcp-config"),
        "{:?}",
        record.arguments
    );
    assert!(
        record
            .arguments
            .windows(2)
            .any(|pair| pair[0] == "--mcp-config" && pair[1].ends_with("mcp-empty.json")),
        "{:?}",
        record.arguments
    );
}

#[test]
fn codex_mcp_server_names_come_from_config_tables() {
    // #2163 follow-up: per-server disables need the declared names;
    // a server's `.env` subtable is not another server.
    let names = crate::profiles::codex_mcp_server_names(
        "[mcp_servers.kernel-mcp]\ncommand = \"x\"\n[mcp_servers.kernel-mcp.env]\nA = \"1\"\n[mcp_servers.meta_core]\ncommand = \"y\"\n",
    );
    assert_eq!(names, vec!["kernel-mcp".to_owned(), "meta_core".to_owned()]);
}
