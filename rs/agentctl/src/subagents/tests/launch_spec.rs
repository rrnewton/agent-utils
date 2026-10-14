use super::*;

const TIMEOUT: Duration = Duration::from_secs(30);
const CODEX: &str = "/usr/local/bin/codex_cli/codex";

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// Adopt the fake's hand-started Codex pane, whose pinned process (pid 300) ran `argv`.
fn adopted(argv: Option<&[&str]>, reports_session: bool) -> (Fixture, Value) {
    let fixture = Fixture::new();
    fixture
        .client
        .report_session
        .store(reports_session, Ordering::Relaxed);
    if let Some(argv) = argv {
        fixture
            .client
            .command_lines
            .lock()
            .unwrap()
            .insert(300, strings(argv));
    }
    let result = fixture.adopt();
    (fixture, result)
}

fn kill(fixture: &Fixture, record: &AgentRecord) {
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(record.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    // Adoption created no tab, so count the adopted one before a replacement is allocated.
    fixture.client.environments.lock().unwrap().push(Vec::new());
}

#[test]
fn start_records_the_exact_launch_before_herdr_reports_back() {
    let fixture = Fixture::new();
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                model: Some("chosen".to_owned()),
                harness_args: strings(&["--dangerously-skip-permissions"]),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load("worker").unwrap();
    let spec = record.launch_spec.clone().unwrap();
    let native = record.native_session.clone().unwrap();
    assert_eq!(spec["source"], "start");
    assert_eq!(spec["executable"], Value::Null);
    assert_eq!(spec["argv"], json!(record.arguments));
    assert_eq!(
        record.arguments,
        strings(&[
            "--session-id",
            &native.value,
            "--model",
            "chosen",
            "--dangerously-skip-permissions"
        ])
    );
    assert_eq!(
        spec["native_session"],
        json!({"agent": "claude", "value": native.value, "source": "asserted"})
    );
}

#[test]
fn adopt_reads_the_pinned_command_line_and_its_conversation() {
    let argv = [CODEX, "resume", "argv-conversation", "--no-alt-screen"];
    let (_fixture, result) = adopted(Some(&argv), false);
    let spec = &result["launch_spec"];
    assert_eq!(spec["source"], "adopt");
    assert_eq!(spec["executable"], CODEX);
    assert_eq!(spec["argv"], json!(argv[1..]));
    assert_eq!(spec["workspace_label"], "subagents");
    assert_eq!(
        spec["native_session"],
        json!({"agent": "codex", "value": "argv-conversation", "source": "argv"})
    );
}

#[test]
fn adopt_without_a_readable_command_line_records_no_spec_and_cannot_revive() {
    let (fixture, result) = adopted(None, false);
    assert!(result.get("launch_spec").is_none());
    let record = fixture.manager().load("foreign").unwrap();
    kill(&fixture, &record);
    let plan = fixture
        .manager()
        .revive("foreign", true, None, TIMEOUT)
        .unwrap();
    assert_eq!(plan["action"], "blocked");
    assert!(plan["reason"]
        .as_str()
        .unwrap()
        .contains("agentctl anchor foreign --replace"));
}

#[test]
fn anchor_records_a_caller_conversation_and_refuses_a_contradiction() {
    let (fixture, _) = adopted(Some(&[CODEX, "resume", "launched-as"]), false);
    let anchored = fixture
        .manager()
        .anchor("foreign", false, Some("after-clear"))
        .unwrap();
    assert_eq!(anchored["launch_spec_source"], "anchor");
    let record = fixture.manager().load("foreign").unwrap();
    assert_eq!(
        record.launch_spec.unwrap()["native_session"],
        json!({"agent": "codex", "value": "after-clear", "source": "caller"})
    );

    let (reported, result) = adopted(Some(&[CODEX, "resume", "x"]), true);
    let session = result["session_value"].as_str().unwrap().to_owned();
    assert_ne!(session, "different");
    let before = reported.manager().load("foreign").unwrap().launch_spec;
    let error = reported
        .manager()
        .anchor("foreign", false, Some("different"))
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("contradicts the session Herdr reports"));
    assert_eq!(
        reported.manager().load("foreign").unwrap().launch_spec,
        before
    );
}

#[test]
fn revive_replays_an_adopted_spec_as_an_owned_agent_and_leaves_the_old_pane() {
    let argv = [
        CODEX,
        "resume",
        "lead-conversation",
        "--no-alt-screen",
        "--dangerously-bypass-approvals-and-sandbox",
    ];
    let (fixture, _) = adopted(Some(&argv), false);
    let old = fixture.manager().load("foreign").unwrap();
    kill(&fixture, &old);
    let plan = fixture
        .manager()
        .revive("foreign", true, None, TIMEOUT)
        .unwrap();
    assert_eq!(plan["action"], "revive", "{plan}");
    let result = fixture
        .manager()
        .revive("foreign", false, Some(&old.token), TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    assert_eq!(result["pane_closed"], false);
    assert_eq!(result["tab_closed"], false);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    let new = fixture.manager().load("foreign").unwrap();
    let expected = strings(&argv[1..]);
    assert_eq!(new.adapter, "herdr");
    assert_eq!(new.arguments, expected);
    assert_eq!(new.resume.as_deref(), Some("lead-conversation"));
    assert!(new.foreign_shell_identity.is_none());
    let spec = new.launch_spec.unwrap();
    assert_eq!(spec["source"], "revive");
    assert_eq!(spec["argv"], json!(expected));
    assert_eq!(spec["workspace_label"], "subagents");
}

#[test]
fn revive_refuses_a_spec_whose_program_is_not_the_harness() {
    let (fixture, _) = adopted(
        Some(&["/usr/bin/node", "/opt/cli.js", "resume", "x"]),
        false,
    );
    let old = fixture.manager().load("foreign").unwrap();
    kill(&fixture, &old);
    let plan = fixture
        .manager()
        .revive("foreign", true, None, TIMEOUT)
        .unwrap();
    assert!(
        plan["reason"]
            .as_str()
            .unwrap()
            .contains("is not the codex harness"),
        "{plan}"
    );
}

#[test]
fn record_with_a_malformed_spec_is_refused() {
    let fixture = Fixture::new();
    fixture.start(None);
    let path = fixture.root.join("registry/worker/agent.json");
    let mut document = agent::read_private_json(&path).unwrap();
    document["launch_spec"]["argv"] = json!(["", "x"]);
    agent::atomic_json(&path, &document).unwrap();
    let error = fixture.manager().load("worker").unwrap_err();
    assert!(error.to_string().contains("invalid launch spec (argv)"));
}

/// A resumed Codex launch whose readiness wait failed while the harness came up anyway.
fn failed_codex_launch() -> (Fixture, AgentRecord) {
    let fixture = Fixture::new();
    fixture
        .manager()
        .start(
            "liteinst",
            &fixture.root,
            StartOptions {
                harness: "codex".to_owned(),
                resume: Some("goal-conversation".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let mut record = fixture.manager().load("liteinst").unwrap();
    record.lifecycle = "launch_failed".to_owned();
    record.error = Some("agent start: timed out waiting for agent startup".to_owned());
    record.harness_identity = None;
    record.anchor_rule = None;
    record.terminal_id = None;
    record.session_agent = None;
    record.session_value = None;
    fixture.manager().save(&record).unwrap();
    (fixture, record)
}

#[test]
fn promote_pins_a_live_harness_behind_a_failed_launch_without_touching_its_tab() {
    let (fixture, record) = failed_codex_launch();
    let result = fixture
        .manager()
        .promote("liteinst", Some(&record.token))
        .unwrap();
    assert_eq!(result["promoted"], true);
    let promoted = fixture.manager().load("liteinst").unwrap();
    assert_eq!(promoted.lifecycle, "running");
    assert!(promoted.error.is_none());
    assert!(promoted.harness_anchor().is_some());
    assert!(promoted.terminal_id.is_some());
    assert_eq!(promoted.token, record.token);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    let error = fixture.manager().promote("liteinst", None).unwrap_err();
    assert!(error
        .to_string()
        .contains("applies to a launch_failed record"));
}

#[test]
fn promote_refuses_unless_the_launched_harness_is_provably_there() {
    for failure in ["absent", "token", "unpinned"] {
        let (fixture, record) = failed_codex_launch();
        match failure {
            "absent" => {
                fixture
                    .client
                    .dead_panes
                    .lock()
                    .unwrap()
                    .insert(record.pane_id.clone().unwrap());
                fixture.client.started.store(false, Ordering::Relaxed);
            }
            "unpinned" => fixture
                .client
                .missing_harness_identity
                .store(true, Ordering::Relaxed),
            _ => {}
        }
        let token = if failure == "token" {
            Some("stale")
        } else {
            None
        };
        assert!(
            fixture.manager().promote("liteinst", token).is_err(),
            "{failure}"
        );
        assert_eq!(
            fixture.manager().load("liteinst").unwrap().lifecycle,
            "launch_failed",
            "{failure}"
        );
    }
}

/// A Codex harness started behind a root slot relay, its process pinned at launch.
fn relay_worker() -> (Fixture, AgentRecord) {
    let fixture = Fixture::new();
    let executable = fake_wrkslots(&fixture.root, &fixture.root, "root");
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                slot: Some(SlotLaunch {
                    slot: "s1".to_owned(),
                    executable: Some(executable),
                    ..SlotLaunch::default()
                }),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load("worker").unwrap();
    // The fake reports a pinned custom process alive while its pane holds it.
    fixture.client.custom_processes.lock().unwrap().insert(
        record.pane_id.clone().unwrap(),
        record.custom_process_identity.clone().unwrap(),
    );
    (fixture, record)
}

#[test]
fn a_relay_launch_records_its_spec_and_anchor_backfills_one_from_the_pinned_process() {
    let (fixture, mut record) = relay_worker();
    let spec = record.launch_spec.clone().unwrap();
    assert_eq!(spec["source"], "start");
    assert_eq!(spec["executable"], "/opt/bin/codex");
    assert_eq!(spec["argv"], json!(record.arguments));
    assert_eq!(spec["slot"], "s1");
    assert_eq!(spec["slot_isolation"], "root");
    // A record written before launch specs existed: anchor reads the pinned process.
    record.launch_spec = None;
    fixture.manager().save(&record).unwrap();
    let identity = record.custom_process_identity.clone().unwrap();
    let mut argv = vec!["/opt/bin/codex".to_owned()];
    argv.extend(record.arguments.iter().cloned());
    fixture
        .client
        .command_lines
        .lock()
        .unwrap()
        .insert(identity.pid, argv);
    let result = fixture.manager().anchor("worker", false, None).unwrap();
    assert_eq!(result["launch_spec_source"], "anchor");
    assert_eq!(result["replaced"], false);
    let backfilled = fixture.manager().load("worker").unwrap();
    assert_eq!(backfilled.custom_process_identity, Some(identity.clone()));
    assert_eq!(
        backfilled.launch_spec.unwrap()["argv"],
        json!(record.arguments)
    );
    fixture.client.command_lines.lock().unwrap().clear();
    let error = fixture.manager().anchor("worker", false, None).unwrap_err();
    assert!(error.to_string().contains("cannot read the command line"));
}

#[test]
fn promote_marks_a_failed_relay_launch_running_when_its_pinned_harness_runs() {
    let (fixture, mut record) = relay_worker();
    record.lifecycle = "launch_failed".to_owned();
    record.error = Some("timed out".to_owned());
    record.pane_reported_by_agentctl = false;
    fixture.manager().save(&record).unwrap();
    let result = fixture
        .manager()
        .promote("worker", Some(&record.token))
        .unwrap();
    assert_eq!(result["promoted"], true);
    let promoted = fixture.manager().load("worker").unwrap();
    assert_eq!(promoted.lifecycle, "running");
    assert!(promoted.error.is_none());
    assert!(promoted.pane_reported_by_agentctl);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    let mut failed = promoted;
    failed.lifecycle = "launch_failed".to_owned();
    fixture.manager().save(&failed).unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(failed.pane_id.clone().unwrap());
    assert!(fixture.manager().promote("worker", None).is_err());
    assert_eq!(
        fixture.manager().load("worker").unwrap().lifecycle,
        "launch_failed"
    );
}
