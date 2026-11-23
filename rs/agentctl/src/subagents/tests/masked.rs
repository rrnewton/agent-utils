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
