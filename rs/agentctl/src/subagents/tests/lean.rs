//! `#2322 agentctl-coordinator-lean-headless-profile`: a launch profile with
//! `coordinator_lean` starts Claude with one agentctl-rendered settings file
//! (the Bash ceiling, the tool denies and the `lean-hook`), implies no MCP
//! servers, is refused for other harnesses, and `status` names it. The
//! hook's decisions are unit-tested in `crate::lean::tests`.

use super::*;

fn write_profiles(fixture: &Fixture, profiles: &Value) {
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
        &json!({"schema": "agentctl-profiles/v1", "profiles": profiles}),
    )
    .unwrap();
}

#[test]
fn a_lean_profile_renders_its_settings_and_status_names_it() {
    let fixture = Fixture::new();
    write_profiles(
        &fixture,
        &json!({"lean": {
            "harness": "claude",
            "mode": "interactive",
            "reasoning_effort": "high",
            "argv": ["--dangerously-skip-permissions"],
            "coordinator_lean": {"bash_timeout_seconds": 20, "lanes": ["lead-hands"]},
        }}),
    );
    let root =
        crate::profiles::configuration_root(&fixture.root, &fixture.manager().registry, false)
            .unwrap();
    let (_, profiles) = crate::profiles::load_profiles(&root, false).unwrap();
    let profile = &profiles["lean"];
    assert!(
        profile.no_mcp_servers,
        "a lean profile implies no MCP servers"
    );
    assert_eq!(
        profile
            .coordinator_lean
            .as_ref()
            .unwrap()
            .bash_timeout_seconds,
        20
    );
    let manager = fixture.manager();
    manager
        .start(
            "lead",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                profile: Some("lean".to_owned()),
                harness: "claude".to_owned(),
                harness_args: profile.argv.clone(),
                no_mcp_servers: profile.no_mcp_servers,
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = manager.load("lead").unwrap();
    let settings = crate::lean::settings_path(&manager.registry, "lead");
    assert!(
        record
            .arguments
            .windows(2)
            .any(|pair| pair[0] == "--settings" && pair[1] == settings.display().to_string()),
        "{:?}",
        record.arguments
    );
    assert!(
        record
            .arguments
            .iter()
            .any(|argument| argument == "--strict-mcp-config"),
        "{:?}",
        record.arguments
    );
    assert!(record
        .arguments
        .iter()
        .any(|argument| argument == "--dangerously-skip-permissions"));
    let rendered: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(rendered["env"]["BASH_MAX_TIMEOUT_MS"], "20000");
    assert_eq!(rendered["env"]["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"], "1");
    let command = rendered["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(
        command
            .contains("'lean-hook' '--agent' 'lead' '--bash-timeout' '20' '--lane' 'lead-hands'"),
        "{command}"
    );
    assert!(command.ends_with(" || exit 2"), "{command}");
    let status = manager.status("lead").unwrap();
    assert_eq!(status["coordinator_lean"]["bash_timeout_seconds"], 20);
    assert_eq!(status["coordinator_lean"]["lanes"], json!(["lead-hands"]));
}

#[test]
fn a_lean_profile_for_another_harness_or_a_bad_value_is_refused() {
    for (harness, lean, argv, why) in [
        (
            "codex",
            json!(true),
            "--yolo",
            "applies only to harness claude",
        ),
        (
            "claude",
            json!({"bash_timeout_seconds": 600}),
            "--dangerously-skip-permissions",
            "from 10 to 60",
        ),
        (
            "claude",
            json!("on"),
            "--dangerously-skip-permissions",
            "true, false, or an object",
        ),
    ] {
        let fixture = Fixture::new();
        write_profiles(
            &fixture,
            &json!({"lean": {
                "harness": harness,
                "mode": "interactive",
                "reasoning_effort": "high",
                "argv": [argv],
                "coordinator_lean": lean,
            }}),
        );
        let root =
            crate::profiles::configuration_root(&fixture.root, &fixture.manager().registry, false)
                .unwrap();
        let error = crate::profiles::load_profiles(&root, false).unwrap_err();
        assert!(error.to_string().contains(why), "{harness} {lean}: {error}");
    }
}

#[test]
fn a_profile_without_the_key_launches_as_before() {
    let fixture = Fixture::new();
    write_profiles(
        &fixture,
        &json!({"plain": {
            "harness": "claude",
            "mode": "interactive",
            "reasoning_effort": "high",
            "argv": ["--dangerously-skip-permissions"],
        }}),
    );
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                profile: Some("plain".to_owned()),
                harness: "claude".to_owned(),
                harness_args: vec!["--dangerously-skip-permissions".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = manager.load("worker").unwrap();
    assert!(!record
        .arguments
        .iter()
        .any(|argument| argument == "--settings"));
    assert!(!crate::lean::settings_path(&manager.registry, "worker").exists());
    assert!(manager
        .status("worker")
        .unwrap()
        .get("coordinator_lean")
        .is_none());
}
