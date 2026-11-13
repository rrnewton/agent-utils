//! `#1216 agentctl-effort-option`: per-harness effort validation, headless launch
//! recording, live set-effort, and the record field the latency stamps read.

use super::*;

#[test]
fn effort_levels_are_checked_per_harness() {
    assert!(crate::profiles::validate_effort("claude", "high").is_ok());
    assert!(crate::profiles::validate_effort("claude", "max").is_ok());
    let refused = crate::profiles::validate_effort("claude", "ultra").unwrap_err();
    assert!(refused.to_string().contains("accepted levels"), "{refused}");
    assert!(crate::profiles::validate_effort("muse", "ultra").is_ok());
    assert!(crate::profiles::validate_effort("codex", "xhigh").is_ok());
    assert!(crate::profiles::validate_effort("codex", "max").is_ok());
    assert!(crate::profiles::validate_effort("codex", "ultra").is_ok());
    assert!(crate::profiles::validate_effort("codex", "none").is_err());
    assert!(crate::profiles::validate_effort("codex", "bogus").is_err());
    assert!(crate::profiles::validate_effort("claude", "bogus").is_err());
    assert!(crate::profiles::validate_effort("unknown", "high").is_err());
}

#[test]
fn codex_max_and_ultra_start_with_their_config_argument() {
    for level in ["max", "ultra"] {
        let fixture = Fixture::new();
        let status = fixture
            .manager()
            .start_with_reasoning_effort(
                "worker",
                &fixture.root,
                level,
                StartOptions {
                    workspace_id: Some("workspace".to_owned()),
                    ..StartOptions::default()
                },
            )
            .unwrap();
        assert_eq!(status["reasoning_effort"], level);
        let expected = format!("model_reasoning_effort={level}");
        assert!(
            status["arguments"]
                .as_array()
                .unwrap()
                .windows(2)
                .any(|pair| pair[0] == "--config" && pair[1] == expected),
            "{status}"
        );
        let record = fixture.manager().load("worker").unwrap();
        assert_eq!(record.reasoning_effort.as_deref(), Some(level));
        assert!(record.launch_spec.as_ref().is_some_and(|spec| {
            spec["argv"]
                .as_array()
                .unwrap()
                .windows(2)
                .any(|pair| pair[0] == "--config" && pair[1] == expected)
        }));
    }
    // A level no harness knows is still refused, before any launch.
    let fixture = Fixture::new();
    let error = fixture
        .manager()
        .start_with_reasoning_effort(
            "worker",
            &fixture.root,
            "bogus",
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("unsupported reasoning effort"),
        "{error}"
    );
}

#[test]
fn effort_argument_replaces_an_earlier_setting() {
    let arguments = vec![
        "--model".to_owned(),
        "m".to_owned(),
        "--effort".to_owned(),
        "xhigh".to_owned(),
    ];
    assert_eq!(
        crate::subagents::headless::with_effort_argument(&arguments, "high"),
        vec!["--model", "m", "--effort", "high"]
    );
    let inline = vec!["--effort=max".to_owned()];
    assert_eq!(
        crate::subagents::headless::with_effort_argument(&inline, "low"),
        vec!["--effort", "low"]
    );
}

#[test]
fn headless_start_records_its_effort_and_set_effort_switches_it() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                reasoning_effort: Some("xhigh".to_owned()),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let record = manager.load("lead").unwrap();
    assert_eq!(record.reasoning_effort.as_deref(), Some("xhigh"));
    assert!(record
        .arguments
        .windows(2)
        .any(|pair| pair == ["--effort", "xhigh"]));

    let switched = manager.set_effort("lead", "medium", false).unwrap();
    assert_eq!(switched["effort"], "medium");
    assert_eq!(switched["previous_effort"], "xhigh");
    assert_eq!(
        manager.load("lead").unwrap().reasoning_effort.as_deref(),
        Some("medium")
    );
    // An unsupported level is refused and the record is unchanged.
    assert!(manager.set_effort("lead", "ultra", false).is_err());
    assert_eq!(
        manager.load("lead").unwrap().reasoning_effort.as_deref(),
        Some("medium")
    );
}

#[test]
fn run_headless_declared_refuses_an_agent_with_no_declared_effort() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let terminate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let error = manager
        .run_headless_declared("lead", None, None, None, None, terminate)
        .unwrap_err();
    assert!(
        error.to_string().contains("no reasoning effort declared"),
        "{error}"
    );
    // Declaring one on the call satisfies the rule without touching the record first.
    let terminate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let error = manager
        .run_headless_declared("lead", None, Some("ultra"), None, None, terminate)
        .unwrap_err();
    assert!(error.to_string().contains("accepted levels"), "{error}");
}

#[test]
fn headless_start_refuses_an_unsupported_effort() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    let error = manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                reasoning_effort: Some("ultra".to_owned()),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("accepted levels"), "{error}");
}

#[test]
fn set_model_records_an_override_that_wins_and_refuses_bad_names() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                model: Some("claude-first-1".to_owned()),
                reasoning_effort: Some("high".to_owned()),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let switched = manager.set_model("lead", "claude-second-2").unwrap();
    assert_eq!(switched["model"], "claude-second-2");
    assert_eq!(switched["previous_model"], "claude-first-1");
    let record = manager.load("lead").unwrap();
    assert_eq!(record.model_override.as_deref(), Some("claude-second-2"));
    assert_eq!(record.declared_model().as_deref(), Some("claude-second-2"));
    let status = manager.status("lead").unwrap();
    assert_eq!(status["declared_model"], "claude-second-2");
    assert_eq!(status["model_source"], "override");
    assert_eq!(status["declared_effort"], "high");
    // Bad names are refused and the record is unchanged.
    for bad in ["", "two words", "-leading-dash"] {
        assert!(manager.set_model("lead", bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        manager.load("lead").unwrap().model_override.as_deref(),
        Some("claude-second-2")
    );
    // An older record without the field still loads.
    let mut value = serde_json::to_value(&record).unwrap();
    value.as_object_mut().unwrap().remove("model_override");
    let older: AgentRecord = serde_json::from_value(value).unwrap();
    assert_eq!(older.model_override, None);
    assert_eq!(older.declared_model().as_deref(), Some("claude-first-1"));
}

#[test]
fn overrides_survive_a_profile_refresh_and_status_agrees() {
    // A profile-backed interactive agent, killed, then switched by
    // set-effort/set-model: a refresh must not revert either override.
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test profile config", false, false).unwrap();
    agent::atomic_json(
        &directory.join("profiles.json"),
        &json!({"schema":"agentctl-profiles/v1","profiles":{
            "claude-recovery": {"harness":"claude","mode":"interactive","model":"opus","reasoning_effort":"high",
                "argv":["--allowedTools","Read"], "env":{}}
        }}),
    )
    .unwrap();
    let manager = fixture.manager();
    manager
        .start_with_reasoning_effort(
            "worker",
            &fixture.root,
            "high",
            StartOptions {
                harness: "claude".to_owned(),
                profile: Some("claude-recovery".to_owned()),
                model: Some("opus".to_owned()),
                harness_args: vec!["--allowedTools".to_owned(), "Read".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    manager.set_effort("worker", "low", false).unwrap();
    manager.set_model("worker", "claude-override-3").unwrap();
    // Kill the harness, then refresh: the profile must not revert either
    // override, and status shows declared = effective for both.
    let record = manager.load("worker").unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(record.pane_id.clone().unwrap());
    fixture
        .client
        .started
        .store(false, std::sync::atomic::Ordering::Relaxed);
    fixture
        .client
        .custom_alive
        .store(false, std::sync::atomic::Ordering::Relaxed);
    manager.refresh_profile("worker").unwrap();
    let record = manager.load("worker").unwrap();
    assert_eq!(record.reasoning_effort.as_deref(), Some("low"));
    assert_eq!(record.model_override.as_deref(), Some("claude-override-3"));
    assert_eq!(
        record.declared_model().as_deref(),
        Some("claude-override-3")
    );
    let status = manager.status("worker").unwrap();
    assert_eq!(status["declared_effort"], "low");
    assert_eq!(status["effective_effort"], "low");
    assert_eq!(status["declared_model"], "claude-override-3");
    assert_eq!(status["model_override"], "claude-override-3");
}
