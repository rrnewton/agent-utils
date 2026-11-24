//! `#1429 launch-required-env`: registry and profile required launch environment.

use super::*;
use std::collections::BTreeMap;

fn write_config(fixture: &Fixture, document: serde_json::Value) {
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test profile config", false, false).unwrap();
    agent::atomic_json(&directory.join("profiles.json"), &document).unwrap();
}

#[test]
fn required_env_check_names_missing_variables_and_sources() {
    let mut registry = BTreeMap::new();
    registry.insert("codex".to_owned(), vec!["EXAMPLE_GATEWAY".to_owned()]);
    let profile = crate::profiles::LaunchProfile {
        name: "gw".to_owned(),
        harness: "codex".to_owned(),
        mode: "interactive".to_owned(),
        model: None,
        reasoning_effort: None,
        argv: Vec::new(),
        environment: vec!["EXAMPLE_GATEWAY=https://example.invalid".to_owned()],
        cloud: None,
        record_edges: None,
        pinned: None,
        write_sandbox: false,
        slot_isolation: None,
        required_env: vec!["EXAMPLE_EXTRA".to_owned()],
        masked_paths: Vec::new(),
        unset_env: Vec::new(),
        no_mcp_servers: false,
    };
    let empty: BTreeMap<String, String> = BTreeMap::new();
    let error =
        crate::profiles::check_required_env("codex", &empty, &registry, Some(&profile), &[])
            .unwrap_err();
    assert!(error.to_string().contains("EXAMPLE_GATEWAY"), "{error}");
    assert!(
        error.to_string().contains("registry required_env"),
        "{error}"
    );
    assert!(error.to_string().contains("EXAMPLE_EXTRA"), "{error}");
    assert!(
        error.to_string().contains("profile \"gw\" required_env"),
        "{error}"
    );
    // Present but empty does not count; present and nonempty passes.
    let mut effective = BTreeMap::new();
    effective.insert("EXAMPLE_GATEWAY".to_owned(), String::new());
    effective.insert("EXAMPLE_EXTRA".to_owned(), "x".to_owned());
    assert!(crate::profiles::check_required_env(
        "codex",
        &effective,
        &registry,
        Some(&profile),
        &[]
    )
    .is_err());
    effective.insert(
        "EXAMPLE_GATEWAY".to_owned(),
        "https://example.invalid".to_owned(),
    );
    crate::profiles::check_required_env("codex", &effective, &registry, Some(&profile), &[])
        .unwrap();
    // No requirement for another harness; literal entries override the ambient value.
    crate::profiles::check_required_env("claude", &empty, &registry, None, &[]).unwrap();
    let overridden = crate::profiles::effective_environment(&["EXAMPLE_GATEWAY=".to_owned()]);
    assert_eq!(
        overridden.get("EXAMPLE_GATEWAY").map(String::as_str),
        Some("")
    );
}

#[test]
fn required_env_rejects_bad_shapes() {
    for bad in [
        serde_json::json!({"codex": ["BAD-NAME"]}),
        serde_json::json!({"codex": ["EXAMPLE_GATEWAY", "EXAMPLE_GATEWAY"]}),
        serde_json::json!({"bogus": ["EXAMPLE_GATEWAY"]}),
    ] {
        assert!(
            crate::profiles::registry_required_env(Some(&bad)).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn profile_less_start_is_refused_and_profile_start_launches() {
    let fixture = Fixture::new();
    write_config(
        &fixture,
        json!({"schema": "agentctl-profiles/v1",
               "required_env": {"codex": ["EXAMPLE_GATEWAY"]},
               "profiles": {"gw": {"harness": "codex", "mode": "interactive", "argv": ["--yolo"],
                                   "env": {"EXAMPLE_GATEWAY": "https://example.invalid"}}}}),
    );
    std::env::remove_var("EXAMPLE_GATEWAY");
    let manager = fixture.manager();
    let error = manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness_args: vec!["--yolo".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("EXAMPLE_GATEWAY"), "{error}");
    assert!(error.to_string().contains("--profile \"gw\""), "{error}");
    // The profile supplies the variable, so its launch proceeds.
    let status = manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                profile: Some("gw".to_owned()),
                environment: vec!["EXAMPLE_GATEWAY=https://example.invalid".to_owned()],
                harness_args: vec!["--yolo".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    assert_eq!(status["name"], "worker");
}
