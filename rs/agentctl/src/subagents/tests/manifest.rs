//! `#1559 restore-recovery-agent-plan`: manifest record facts judged from
//! real records the fake manager wrote, never from invented status JSON.

use super::*;

const CODEX: &str = "/usr/local/bin/codex_cli/codex";

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn adopted(argv: Option<&[&str]>) -> Fixture {
    let fixture = Fixture::new();
    fixture
        .client
        .report_session
        .store(false, Ordering::Relaxed);
    if let Some(argv) = argv {
        fixture
            .client
            .command_lines
            .lock()
            .unwrap()
            .insert(300, strings(argv));
    }
    fixture.adopt();
    fixture
}

#[test]
fn manifest_facts_judge_unredacted_muse_argv() {
    let fixture = Fixture::new();
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "muse".to_owned(),
                harness_args: strings(&["--approval-mode", "never"]),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let status = fixture.manager().status("worker").unwrap();
    // The status row is the redacted shape team_manifest used to judge:
    // the pair-form bypass value is gone, so it cannot prove anything.
    assert_eq!(status["arguments"][1], "<set>", "{status}");
    let facts = fixture.manager().manifest_record_facts("worker").unwrap();
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
    assert!(facts.native_session_present, "{facts:?}");
}

#[test]
fn manifest_facts_read_adopted_launch_spec_not_empty_arguments() {
    let argv = [CODEX, "resume", "argv-conversation", "--yolo"];
    let fixture = adopted(Some(&argv));
    let record = fixture.manager().load("foreign").unwrap();
    assert!(record.arguments.is_empty());
    assert!(record.launch_spec.is_some());
    let facts = fixture.manager().manifest_record_facts("foreign").unwrap();
    assert!(!facts.adopted_without_launch_spec, "{facts:?}");
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
    assert!(facts.native_session_present, "{facts:?}");

    let fixture = adopted(None);
    let facts = fixture.manager().manifest_record_facts("foreign").unwrap();
    assert!(facts.adopted_without_launch_spec, "{facts:?}");
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
    assert_eq!(facts.profile_state, ManifestProfileState::Absent);

    // A named profile stays named even though the missing spec means its
    // replay argv cannot be judged.
    let mut record = fixture.manager().load("foreign").unwrap();
    record.profile = Some("claude-recovery".to_owned());
    fixture.manager().save(&record).unwrap();
    let facts = fixture.manager().manifest_record_facts("foreign").unwrap();
    assert!(facts.adopted_without_launch_spec, "{facts:?}");
    assert_eq!(facts.profile_state, ManifestProfileState::NotApplicable);
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
}

fn profile_fixture() -> (Fixture, PathBuf) {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test profile config", false, false).unwrap();
    let path = directory.join("profiles.json");
    agent::atomic_json(
        &path,
        &json!({"schema":"agentctl-profiles/v1","profiles":{
            "claude-recovery": {"harness":"claude","mode":"interactive","model":"opus","reasoning_effort":"high",
                "argv":["--allowedTools","Read","--dangerously-skip-permissions"], "env":{"RECOVERY_MODE":"profile-private-value"}}
        }}),
    )
    .unwrap();
    fixture
        .manager()
        .start_with_reasoning_effort(
            "worker",
            &fixture.root,
            "high",
            StartOptions {
                harness: "claude".to_owned(),
                profile: Some("claude-recovery".to_owned()),
                model: Some("opus".to_owned()),
                harness_args: strings(&[
                    "--allowedTools",
                    "Read",
                    "--dangerously-skip-permissions",
                ]),
                environment: vec!["RECOVERY_MODE=profile-private-value".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    (fixture, path)
}

#[test]
fn manifest_facts_follow_the_current_profile() {
    let (fixture, path) = profile_fixture();
    let manager = fixture.manager();
    let facts = manager.manifest_record_facts("worker").unwrap();
    assert_eq!(facts.profile_state, ManifestProfileState::Current);
    assert_eq!(facts.missing_bypass_flag, None);

    // The profile gains a flag after launch: revive would refresh onto
    // it, so the manifest names the drift and still finds the bypass.
    let mut config = agent::read_private_json(&path).unwrap();
    config["profiles"]["claude-recovery"]["argv"] = json!([
        "--allowedTools",
        "Read",
        "--dangerously-skip-permissions",
        "--settings",
        "/private/lane-settings.json"
    ]);
    agent::atomic_json(&path, &config).unwrap();
    let facts = manager.manifest_record_facts("worker").unwrap();
    assert_eq!(
        facts.profile_state,
        ManifestProfileState::Drifted(vec!["argv"]),
        "{facts:?}"
    );
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");

    // The profile vanishes: revive refuses by name, and so do the facts.
    config["profiles"] = json!({});
    agent::atomic_json(&path, &config).unwrap();
    let facts = manager.manifest_record_facts("worker").unwrap();
    assert_eq!(facts.profile_state, ManifestProfileState::Missing);
}

#[test]
fn manifest_facts_do_not_blame_a_record_when_its_profile_dropped_its_bypass() {
    // Loading validates every profile in the file, so a load refusal
    // cannot prove which profile lacks the bypass. The facts say only
    // that this record's profile could not be judged.
    let (fixture, path) = profile_fixture();
    let mut config = agent::read_private_json(&path).unwrap();
    config["profiles"]["claude-recovery"]["argv"] = json!(["--allowedTools", "Read"]);
    agent::atomic_json(&path, &config).unwrap();
    let facts = fixture.manager().manifest_record_facts("worker").unwrap();
    assert_eq!(facts.profile_state, ManifestProfileState::Unreadable);
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
}

#[test]
fn manifest_facts_do_not_blame_a_record_for_another_profiles_missing_bypass() {
    let (fixture, path) = profile_fixture();
    let mut config = agent::read_private_json(&path).unwrap();
    config["profiles"]["unrelated"] = json!({
        "harness": "claude",
        "mode": "interactive",
        "argv": ["--allowedTools", "Read"],
    });
    agent::atomic_json(&path, &config).unwrap();
    let facts = fixture.manager().manifest_record_facts("worker").unwrap();
    assert_eq!(facts.profile_state, ManifestProfileState::Unreadable);
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
}

#[test]
fn manifest_facts_read_headless_session_and_bypass() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                reasoning_effort: Some("xhigh".to_owned()),
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    let status = manager.status("lead").unwrap();
    assert!(
        status["session"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "{status}"
    );
    let facts = manager.manifest_record_facts("lead").unwrap();
    assert_eq!(facts.declared_effort.as_deref(), Some("xhigh"));
    assert!(facts.native_session_present, "{facts:?}");
    assert_eq!(facts.missing_bypass_flag, None, "{facts:?}");
}

#[test]
fn manifest_facts_read_cloud_declared_effort_the_status_row_omits() {
    let fixture = Fixture::new();
    fixture.start(None);
    let manager = fixture.manager();
    let mut record = manager.load("worker").unwrap();
    record.adapter = "agentcloud".to_owned();
    record.harness = "agentcloud".to_owned();
    record.reasoning_effort = Some("xhigh".to_owned());
    record.foreign_shell_identity = None;
    record.harness_identity = None;
    record.anchor_rule = None;
    record.session_agent = Some("agentcloud".to_owned());
    record.session_value = None;
    record.native_session = None;
    record.launch_spec = None;
    record.agentcloud = Some(cloud::CloudRecord {
        unknown: Default::default(),
        launch: cloud::CloudLaunch::default(),
        agentterm: "/bin/true".to_owned(),
        endpoint: Some("wss://example.invalid/ws".to_owned()),
        create_exit: Some(0),
        verified: Some(true),
        create_note: None,
        terminal_identity: None,
        halted: false,
        attached_self: false,
    });
    manager.save(&record).unwrap();

    // The real cloud status row has reasoning_effort but synthesizes no
    // declared_effort; judging that row alone flagged every cloud record.
    let status = manager.status("worker").unwrap();
    assert_eq!(status["reasoning_effort"], "xhigh", "{status}");
    assert!(status.get("declared_effort").is_none(), "{status}");
    let facts = manager.manifest_record_facts("worker").unwrap();
    assert_eq!(facts.declared_effort.as_deref(), Some("xhigh"));
    assert_eq!(facts.profile_state, ManifestProfileState::Absent);
    assert_eq!(facts.missing_bypass_flag, None);
    assert!(!facts.native_session_present);
}

#[test]
fn manifest_candidate_index_enumerates_muse_for_a_session_less_record() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "muse".to_owned(),
                harness_args: strings(&["--approval-mode", "never"]),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let mut record = manager.load("worker").unwrap();
    record.native_session = None;
    record.launch_spec = None;
    record.session_agent = None;
    record.session_value = None;
    record.resume = None;
    manager.save(&record).unwrap();

    let store = fixture.root.join("muse-store");
    for (id, prompt) in [
        ("11111111-1111-1111-1111-111111111111", "recover lane A"),
        ("22222222-2222-2222-2222-222222222222", "recover lane B"),
    ] {
        let dir = store.join("sessions/2026/11/06").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("session.jsonl"),
            format!(
                "{}\n",
                json!({
                    "cwd": fixture.root.display().to_string(),
                    "message": {
                        "role": "user",
                        "content": [{"type": "text", "text": prompt}],
                    },
                })
            ),
        )
        .unwrap();
    }
    crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = Some(store));
    let facts = manager.manifest_record_facts("worker").unwrap();
    let index = manager.manifest_session_candidate_index(&["worker".to_owned()]);
    crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = None);

    assert!(!facts.native_session_present, "{facts:?}");
    assert!(facts.session_candidates.is_empty(), "{facts:?}");
    let candidates = &index["worker"];
    let mut ids: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222"
        ]
    );
    assert_eq!(
        candidates
            .iter()
            .find(|candidate| candidate.id == "11111111-1111-1111-1111-111111111111")
            .unwrap()
            .first_prompt
            .as_deref(),
        Some("recover lane A")
    );
    // Recorded-only revive refuses the same record before any write:
    // discovery over these very candidates can never bind one.
    fn tree_snapshot(root: &Path) -> std::collections::BTreeMap<PathBuf, (u32, Vec<u8>)> {
        use std::os::unix::fs::PermissionsExt;
        let mut out = std::collections::BTreeMap::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.is_dir() {
                out.insert(relative, (metadata.permissions().mode(), Vec::new()));
                let mut children: Vec<_> = fs::read_dir(&path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                children.sort();
                stack.extend(children);
            } else if metadata.is_file() {
                out.insert(
                    relative,
                    (metadata.permissions().mode(), fs::read(&path).unwrap()),
                );
            }
        }
        out
    }
    let registry = fixture.root.join("registry");
    let before = tree_snapshot(&registry);
    let refused = manager
        .revive_recorded_only("worker", Duration::from_secs(30))
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("recorded-only revive refuses discovery"),
        "{refused}"
    );
    assert_eq!(
        tree_snapshot(&registry),
        before,
        "refusal changed the registry"
    );
    // Enumeration is report-only: the record still names no session.
    assert!(manager.load("worker").unwrap().native_session.is_none());
}
