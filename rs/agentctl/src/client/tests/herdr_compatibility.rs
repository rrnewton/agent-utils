use super::*;

const IDLE_CODEX: &str =
    "Codex\n» \u{1b}[2mAsk Codex to do anything\u{1b}[0m\n  model high · ~/project\n";

fn process_document(pid: u32, executable: &Path) -> Value {
    serde_json::from_str(&process_info_response(pid, executable)).unwrap()
}

fn without_argv(document: &Value, argv: Option<Value>) -> Value {
    let mut document = document.clone();
    let process = document["result"]["process_info"]["foreground_processes"][0]
        .as_object_mut()
        .unwrap();
    process.remove("argv");
    if let Some(argv) = argv {
        process.insert("argv".to_owned(), argv);
    }
    process.insert("name".to_owned(), Value::String("muse".to_owned()));
    process.insert(
        "cmdline".to_owned(),
        Value::String("muse --model claimed".to_owned()),
    );
    document
}

fn configure_codex(herdr: &FakeExecutable, pid: u32, executable: &Path, screen: &str) -> Value {
    let mut process = process_document(pid, executable)["result"]["process_info"].clone();
    process["shell_pid"] = Value::from(1);
    let frame = serde_json::json!({
        "pane": {
            "pane_id": "pane", "workspace_id": "workspace", "cwd": "/work/project",
            "tab_id": "tab", "terminal_id": "terminal", "agent": "codex",
            "agent_status": "unknown", "agent_session": {"agent": "codex", "value": "thread"}
        },
        "process": process,
        "screen": screen,
        "mutate_after_read": null,
        "process_queries": 0,
        "replace_process_after_query": null,
        "startup": null,
        "named": null,
        "agent_get_calls": 0,
        "mutate_on_final_agent_get": null,
        "fail_prelaunch_snapshot": false,
        "starts": 0,
        "renames": [],
        "screen_after_reads": null,
        "reads": 0
    });
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    fs::write(
        herdr.root.join("herdr"),
        r#"#!/usr/bin/python3
import json, pathlib, sys
path = pathlib.Path(__file__).with_name('frame.json')
state = json.loads(path.read_text())
args = sys.argv[1:]
if args[:2] == ['pane', 'process-info']:
    print(json.dumps({'result': {'process_info': state['process']}}))
    state['process_queries'] += 1
    replacement = state['replace_process_after_query']
    if replacement is not None and state['process_queries'] == replacement['after']:
        state['process'] = replacement['process']
elif args[:2] == ['pane', 'get']:
    if state['fail_prelaunch_snapshot'] and state['starts'] == 0:
        raise SystemExit('prelaunch inspection unavailable')
    print(json.dumps({'result': {'pane': state['pane']}}))
elif args[:2] == ['pane', 'read']:
    state['reads'] += 1
    later = state['screen_after_reads']
    if later is not None and state['reads'] > later['after']:
        state['screen'] = later['screen']
    print(state['screen'], end='')
    change = state['mutate_after_read']
    if change == 'process':
        state['process']['foreground_process_group_id'] += 1
    elif change == 'session':
        state['pane']['agent_session']['value'] = 'replacement-thread'
    elif change in ('terminal_id', 'cwd', 'workspace_id', 'agent'):
        state['pane'][change] = 'replacement'
    state['mutate_after_read'] = None
elif args[:2] == ['agent', 'start']:
    state['starts'] += 1
    state['pane'] = state['launched_pane']
    path.write_text(json.dumps(state))
    print(state['startup']['stdout'], end='')
    print(state['startup']['stderr'], end='', file=sys.stderr)
    raise SystemExit(state['startup']['status'])
elif args[:2] == ['agent', 'get']:
    state['agent_get_calls'] += 1
    if state['named'] is None:
        path.write_text(json.dumps(state))
        print(json.dumps({'error': {'code': 'agent_not_found', 'message': 'agent target ' + args[2] + ' not found'}, 'id': 'cli:agent:get'}), file=sys.stderr)
        raise SystemExit(1)
    if state['agent_get_calls'] == 2 and state['mutate_on_final_agent_get'] is not None:
        state['named'].update(state['mutate_on_final_agent_get'])
    print(json.dumps({'result': {'agent': state['named']}}))
elif args[:2] == ['agent', 'rename']:
    state['renames'].append(args[2:])
    state['named'] = {'name': args[3], 'pane_id': args[2], 'tab_id': state['pane']['tab_id'], 'terminal_id': state['pane']['terminal_id']}
    print(json.dumps({'result': {'agent': state['named']}}))
elif args[:2] == ['agent', 'explain']:
    print('agent: codex\nstate: unknown\nmanifest: bundled 2026.09.23.1\nrule: none\nfallback_reason: codex_state_ambiguous')
else:
    raise SystemExit('unexpected test command')
path.write_text(json.dumps(state))
"#,
    )
    .unwrap();
    frame
}

fn configure_start(herdr: &FakeExecutable, pid: u32, executable: &Path, screen: &str) -> Value {
    let mut frame = configure_codex(herdr, pid, executable, screen);
    frame["launched_pane"] = frame["pane"].clone();
    frame["pane"]["agent"] = Value::Null;
    frame["pane"]["agent_session"] = Value::Null;
    frame["named"] = serde_json::json!({
        "name": "worker", "pane_id": "pane", "tab_id": "tab", "terminal_id": "terminal"
    });
    frame["startup"] = serde_json::json!({
        "status": 1, "stdout": "",
        "stderr": "{\"id\":\"cli:agent:start\",\"error\":{\"code\":\"timeout\",\"message\":\"timed out waiting for agent startup\"}}"
    });
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    frame
}

fn start_once(herdr: &FakeExecutable, kind: &str, success: bool) {
    let result = herdr
        .client()
        .start_agent("worker", kind, "pane", &[], Duration::from_millis(25));
    assert_eq!(result.is_ok(), success, "{result:?}");
    let frame: Value =
        serde_json::from_slice(&fs::read(herdr.root.join("frame.json")).unwrap()).unwrap();
    assert_eq!(frame["starts"], 1, "startup must never be repeated");
    if !success {
        assert!(result.unwrap_err().to_string().contains("agent start"));
    }
}

#[test]
fn herdr_093_missing_argv_uses_only_coherent_open_image_or_recorded_generation() {
    let herdr = FakeExecutable::new("{}");
    let harness = herdr.root.join("muse");
    install_test_elf("/bin/sleep", &harness);
    let pinned = pin_harness_executable(harness.clone()).unwrap();
    let process = spawn_test_process(&harness);
    let document = process_document(process.0.id(), &harness);
    let identity = live_custom_process(u64::from(process.0.id()))
        .unwrap()
        .identity;
    for argv in [None, Some(Value::Null), Some(serde_json::json!([]))] {
        let frame = without_argv(&document, argv);
        herdr.set_response(&frame.to_string());
        assert_eq!(
            herdr
                .client()
                .custom_harness_observed("pane", &pinned, &|| false)
                .unwrap(),
            Some(identity.clone())
        );
        assert!(herdr
            .client()
            .recorded_custom_harness_observed("pane", &identity, &|| false)
            .unwrap());
        let mut changed = identity.clone();
        changed.starttime_ticks += 1;
        assert!(!herdr
            .client()
            .recorded_custom_harness_observed("pane", &changed, &|| false)
            .unwrap());
    }
    let unrelated = spawn_test_process(Path::new("/bin/sleep"));
    let frame = without_argv(&process_document(unrelated.0.id(), &harness), None);
    herdr.set_response(&frame.to_string());
    assert!(herdr
        .client()
        .custom_harness_observed("pane", &pinned, &|| false)
        .unwrap()
        .is_none());
    assert!(!herdr
        .client()
        .recorded_custom_harness_observed("pane", &identity, &|| false)
        .unwrap());
}

#[test]
fn herdr_093_malformed_argv_is_not_unknown_process_evidence() {
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let document = process_document(process.0.id(), &executable);
    let herdr = FakeExecutable::new("{}");
    let identity = live_custom_process(u64::from(process.0.id()))
        .unwrap()
        .identity;
    for argv in [
        serde_json::json!("claimed command"),
        serde_json::json!(42),
        serde_json::json!([null]),
        serde_json::json!([""]),
        serde_json::json!(["/bin/sleep", 30]),
        serde_json::json!(["/bin/sleep\u{0000}"]),
    ] {
        herdr.set_response(&without_argv(&document, Some(argv)).to_string());
        assert!(herdr
            .client()
            .recorded_custom_harness_observed("pane", &identity, &|| false)
            .is_err());
        assert!(herdr
            .client()
            .verify_harness_identity("pane", &identity)
            .is_err());
    }
}

#[test]
fn herdr_093_missing_argv_does_not_prove_an_idle_shell() {
    let executable = fs::canonicalize("/bin/bash").unwrap();
    let mut command = Command::new(&executable);
    command
        .args(["--noprofile", "--norc"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0);
    let shell = ChildGuard(command.spawn().unwrap());
    let mut document = process_document(shell.0.id(), &executable);
    document["result"]["process_info"]["shell_pid"] = Value::from(shell.0.id());
    let herdr = FakeExecutable::new(&without_argv(&document, None).to_string());
    assert!(herdr.client().pane_shell_identity("pane").is_ok());
    assert!(!herdr.client().pane_is_idle_shell("pane").unwrap());
}

#[test]
fn herdr_093_unknown_codex_ready_requires_a_stable_process_terminal_and_screen() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    configure_codex(&herdr, process.0.id(), &executable, IDLE_CODEX);
    let client = herdr.client();
    let info = client.pane_info("pane").unwrap();
    assert!(client
        .codex_idle_ready_with_cancellation(&info, &|| false)
        .unwrap());
    assert_eq!(client.pane_info("pane").unwrap().status, "unknown");
    for change in [
        "process",
        "session",
        "terminal_id",
        "cwd",
        "workspace_id",
        "agent",
    ] {
        let mut frame = configure_codex(&herdr, process.0.id(), &executable, IDLE_CODEX);
        frame["mutate_after_read"] = Value::from(change);
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        let info = client.pane_info("pane").unwrap();
        assert!(
            !client
                .codex_idle_ready_with_cancellation(&info, &|| false)
                .unwrap(),
            "{change}"
        );
    }
    let mut frame = configure_codex(&herdr, process.0.id(), &executable, IDLE_CODEX);
    frame["process"]["shell_pid"] = Value::from(process.0.id());
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    let info = client.pane_info("pane").unwrap();
    assert!(!client
        .codex_idle_ready_with_cancellation(&info, &|| false)
        .unwrap());
}

#[test]
fn herdr_093_unknown_codex_ready_refuses_drafts_dialogs_unpinned_and_malformed_identities() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let client = herdr.client();
    for screen in [
        "» operator draft\n  ? for shortcuts\n",
        "Working · esc to interrupt\n»\n  ? for shortcuts\n",
        "Approval required\n»\n  ? for shortcuts\n",
        "Trust this folder?\n›\n  Trust and continue\n",
        "shell$\n",
    ] {
        configure_codex(&herdr, process.0.id(), &executable, screen);
        let info = client.pane_info("pane").unwrap();
        assert!(
            !client
                .codex_idle_ready_with_cancellation(&info, &|| false)
                .unwrap(),
            "{screen:?}"
        );
    }
    for field in [
        "agent",
        "agent_status",
        "terminal_id",
        "provider",
        "value",
        "partial",
    ] {
        let mut frame = configure_codex(&herdr, process.0.id(), &executable, IDLE_CODEX);
        match field {
            "agent" => frame["pane"][field] = Value::from("claude"),
            "agent_status" => frame["pane"][field] = Value::from("blocked"),
            "terminal_id" => frame["pane"][field] = Value::Null,
            "provider" => frame["pane"]["agent_session"]["agent"] = Value::from("claude"),
            "value" => frame["pane"]["agent_session"]["value"] = Value::from(""),
            _ => frame["pane"]["agent_session"]["value"] = Value::Null,
        }
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        let info = client.pane_info("pane").unwrap();
        assert!(
            !client
                .codex_idle_ready_with_cancellation(&info, &|| false)
                .unwrap(),
            "{field}"
        );
    }
}

#[test]
fn herdr_093_start_accepts_only_the_exact_codex_timeout_with_independent_ready_proof() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    configure_start(&herdr, process.0.id(), &executable, IDLE_CODEX);
    start_once(&herdr, "codex", true);
    assert_eq!(herdr.client().pane_info("pane").unwrap().status, "unknown");

    let mut frame = configure_start(&herdr, process.0.id(), &executable, IDLE_CODEX);
    frame["launched_pane"]["agent_session"] = Value::Null;
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    start_once(&herdr, "codex", true);

    configure_start(&herdr, process.0.id(), &executable, IDLE_CODEX);
    start_once(&herdr, "claude", false);
}

/// The Codex v0.164.0 composer, captured from a pane Herdr 0.9.3 reported `unknown`.
const CODEX_0164_IDLE: &str = include_str!("../../../testdata/codex/v0164-idle-composer.ansi");

fn frame_of(herdr: &FakeExecutable) -> Value {
    serde_json::from_slice(&fs::read(herdr.root.join("frame.json")).unwrap()).unwrap()
}

/// Herdr 0.9.3 drops the name of an agent whose startup wait timed out and never applies it,
/// though Codex keeps running in the pane: agentctl proves the pane ready, then names it.
#[test]
fn herdr_093_timeout_that_drops_the_name_is_named_once_the_pane_is_proved_ready() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let mut frame = configure_start(&herdr, process.0.id(), &executable, CODEX_0164_IDLE);
    frame["named"] = Value::Null;
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    start_once(&herdr, "codex", true);
    let frame = frame_of(&herdr);
    assert_eq!(frame["renames"], serde_json::json!([["pane", "worker"]]));
    assert_eq!(frame["named"]["pane_id"], "pane");

    // Nothing is named until the pane is proved ready.
    for screen in [
        "Codex\nInstalling plugins for preset\n",
        "Codex\nWorking (7s)\n»\n  ? for shortcuts\n",
        "Codex\n» operator draft\n  ? for shortcuts\n",
    ] {
        let mut frame = configure_start(&herdr, process.0.id(), &executable, screen);
        frame["named"] = Value::Null;
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        start_once(&herdr, "codex", false);
        assert_eq!(
            frame_of(&herdr)["renames"],
            serde_json::json!([]),
            "{screen:?}"
        );
    }
    // Nor when the pane changed terminal since the launch.
    let mut frame = configure_start(&herdr, process.0.id(), &executable, CODEX_0164_IDLE);
    frame["named"] = Value::Null;
    frame["launched_pane"]["terminal_id"] = Value::from("another");
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    start_once(&herdr, "codex", false);
    assert_eq!(frame_of(&herdr)["renames"], serde_json::json!([]));
}

/// Codex can still be drawing its composer when Herdr gives up; agentctl keeps looking for up
/// to the startup timeout again (at most 15 s).
#[test]
fn herdr_093_timeout_waits_a_grace_period_for_the_composer() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let mut frame = configure_start(
        &herdr,
        process.0.id(),
        &executable,
        "Codex\nInstalling plugins for preset\n",
    );
    frame["named"] = Value::Null;
    frame["screen_after_reads"] = serde_json::json!({"after": 1, "screen": CODEX_0164_IDLE});
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    let result = herdr
        .client()
        .start_agent("worker", "codex", "pane", &[], Duration::from_secs(5));
    assert!(result.is_ok(), "{result:?}");
    let frame = frame_of(&herdr);
    assert_eq!(frame["starts"], 1);
    assert_eq!(frame["renames"], serde_json::json!([["pane", "worker"]]));
}

/// A start that does time out says what Herdr and the screen showed.
#[test]
fn startup_timeout_reports_the_observed_agent_status_name_and_screen() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let screen = "Codex\n\nerror: the argument '--no-alt-screen' cannot be used multiple times\n\n";
    let mut frame = configure_start(&herdr, process.0.id(), &executable, screen);
    frame["named"] = Value::Null;
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    let error = herdr
        .client()
        .start_agent("worker", "codex", "pane", &[], Duration::from_millis(25))
        .unwrap_err()
        .to_string();
    for part in [
        "timed out waiting for agent startup",
        "observed: Herdr reports agent codex with status unknown",
        "(rule none, fallback codex_state_ambiguous)",
        "the name \"worker\" was not applied",
        "screen: Codex | error: the argument '--no-alt-screen' cannot be used multiple times",
    ] {
        assert!(error.contains(part), "{part:?} missing from {error}");
    }
    // Any harness kind gets the observation, not only Codex.
    configure_start(&herdr, process.0.id(), &executable, screen);
    let error = herdr
        .client()
        .start_agent("worker", "claude", "pane", &[], Duration::from_millis(25))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("observed: Herdr reports agent codex"),
        "{error}"
    );
    assert!(error.contains("named \"worker\""), "{error}");
}

#[test]
fn startup_screen_evidence_keeps_the_last_rows_and_cuts_long_ones() {
    let screen = (1..=9)
        .map(|row| format!("row {row}\n\n"))
        .collect::<String>();
    assert_eq!(
        startup_screen_evidence(&screen),
        "row 4 | row 5 | row 6 | row 7 | row 8 | row 9"
    );
    let long = format!("{}é", "x".repeat(200));
    assert_eq!(
        startup_screen_evidence(&long),
        format!("{}…", "x".repeat(160))
    );
    assert_eq!(startup_screen_evidence("\n  \n"), "");
}

/// A resumed Codex with an active goal starts working at once and is never idle at Herdr's
/// startup wait: the exact timeout is forgiven only for that resumed conversation.
#[test]
fn exact_codex_timeout_accepts_a_resumed_harness_already_working_on_its_goal() {
    const WORKING_CODEX: &str = "Codex\n• Pursuing goal (3d 10h)\n  esc to interrupt\n";
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    let resume = |session: Option<&str>, arguments: &[&str]| {
        let mut frame = configure_start(&herdr, process.0.id(), &executable, WORKING_CODEX);
        frame["launched_pane"]["agent_status"] = Value::from("working");
        frame["launched_pane"]["agent_session"] = match session {
            Some(value) => serde_json::json!({"agent": "codex", "value": value}),
            None => Value::Null,
        };
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        let arguments = arguments
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        herdr.client().start_agent(
            "worker",
            "codex",
            "pane",
            &arguments,
            Duration::from_millis(25),
        )
    };
    assert!(resume(Some("goal"), &["resume", "goal", "--no-alt-screen"]).is_ok());
    assert!(resume(None, &["resume", "goal"]).is_ok());
    assert!(resume(Some("other"), &["resume", "goal"]).is_err());
    assert!(resume(None, &["--no-alt-screen"]).is_err());
    assert!(resume(None, &["resume", "--last"]).is_err());
}

#[test]
fn herdr_093_start_never_forgives_near_timeout_envelopes_or_other_failures() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    for (status, stdout, stderr) in [
        (
            2,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "{\"result\":{}}",
            r#"{"id":"cli:agent:start","error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (1, "", "timed out waiting for agent startup"),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"agent_not_ready","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start:timeout","error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"timeout","message":"timeout while trusting folder"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","extra":true,"error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"timeout","message":"timed out waiting for agent startup","extra":true}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"other","id":"cli:agent:start","error":{"code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"other","code":"timeout","message":"timed out waiting for agent startup"}}"#,
        ),
        (
            1,
            "",
            r#"{"id":"cli:agent:start","error":{"code":"timeout","message":"other","message":"timed out waiting for agent startup"}}"#,
        ),
        (1, "", "{broken"),
    ] {
        let mut frame = configure_start(&herdr, process.0.id(), &executable, IDLE_CODEX);
        frame["startup"] =
            serde_json::json!({"status": status, "stdout": stdout, "stderr": stderr});
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        start_once(&herdr, "codex", false);
        let frame: Value =
            serde_json::from_slice(&fs::read(herdr.root.join("frame.json")).unwrap()).unwrap();
        assert_eq!(frame["process_queries"], 0, "{stderr}");
    }
}

#[test]
fn herdr_093_start_refuses_busy_dialog_draft_and_changed_presentation_or_name() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    for screen in [
        "Codex\nWorking (7s)\n»\n  ? for shortcuts\n",
        "Codex\nExploring (1m 2s • Ctrl+c to interrupt)\n»\n  ? for shortcuts\n",
        "Codex\nApproval required\n»\n  ? for shortcuts\n",
        "Codex\n» operator draft\n  ? for shortcuts\n",
        "Trust this folder?\n›\n  Trust and continue\n",
    ] {
        configure_start(&herdr, process.0.id(), &executable, screen);
        start_once(&herdr, "codex", false);
    }
    for field in [
        "agent",
        "agent_status",
        "terminal_id",
        "workspace_id",
        "cwd",
        "tab_id",
        "name",
        "named_pane",
        "final_name",
        "initial_agent",
        "missing_initial",
    ] {
        let mut frame = configure_start(&herdr, process.0.id(), &executable, IDLE_CODEX);
        match field {
            "name" => frame["named"]["name"] = Value::from("another"),
            "named_pane" => frame["named"]["pane_id"] = Value::from("another"),
            "final_name" => {
                frame["mutate_on_final_agent_get"] = serde_json::json!({"pane_id": "another"})
            }
            "initial_agent" => frame["pane"]["agent"] = Value::from("codex"),
            "missing_initial" => frame["fail_prelaunch_snapshot"] = Value::from(true),
            "agent_status" => frame["launched_pane"][field] = Value::from("blocked"),
            _ => frame["launched_pane"][field] = Value::from("another"),
        }
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        start_once(&herdr, "codex", false);
    }
}

#[test]
fn herdr_093_start_pins_the_original_process_across_each_readiness_observation() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let original = spawn_test_process(&executable);
    let replacement = spawn_test_process(&executable);
    let mut frame = configure_start(&herdr, original.0.id(), &executable, IDLE_CODEX);
    let mut replacement_info =
        process_document(replacement.0.id(), &executable)["result"]["process_info"].clone();
    replacement_info["shell_pid"] = Value::from(1);
    frame["replace_process_after_query"] =
        serde_json::json!({"after": 1, "process": replacement_info});
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    start_once(&herdr, "codex", false);
    for change in ["process", "session", "terminal_id", "workspace_id", "cwd"] {
        let mut frame = configure_start(&herdr, original.0.id(), &executable, IDLE_CODEX);
        frame["mutate_after_read"] = Value::from(change);
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        start_once(&herdr, "codex", false);
    }
}

#[test]
fn herdr_093_native_start_success_keeps_its_existing_barrier_when_fallback_is_unavailable() {
    let herdr = FakeExecutable::new("{}");
    let executable = fs::canonicalize("/bin/sleep").unwrap();
    let process = spawn_test_process(&executable);
    for kind in ["codex", "claude"] {
        let mut frame = configure_start(&herdr, process.0.id(), &executable, "not a composer");
        frame["fail_prelaunch_snapshot"] = Value::from(true);
        frame["startup"] =
            serde_json::json!({"status": 0, "stdout": "{\"result\":{}}", "stderr": ""});
        fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
        start_once(&herdr, kind, true);
        let frame: Value =
            serde_json::from_slice(&fs::read(herdr.root.join("frame.json")).unwrap()).unwrap();
        assert_eq!(frame["process_queries"], 0);
        assert_eq!(frame["agent_get_calls"], 0);
    }
}

#[test]
fn herdr_093_muse_uses_only_the_active_mode_and_complete_literal_editor() {
    let frame = |editor: &str, footer: &str| {
        format!("Muse Code 1.3.0\nprior answer mentioned Auto-review\n────────────\n{editor}\n────────────\n{footer}\n")
    };
    let auto_review = "watermelon-preview · xhigh · /work/project · Auto-review";
    let yolo = "watermelon-preview · xhigh · /work/project · YOLO";
    assert!(muse_idle_composer(&frame("❯", auto_review)));
    assert!(muse_auto_review_idle_composer(&frame("❯", auto_review)));
    assert!(muse_idle_composer(&frame("❯", yolo)));
    assert!(!muse_auto_review_idle_composer(&frame("❯", yolo)));
    let cropped = frame("❯", yolo).replace("Muse Code 1.3.0\n", "");
    assert!(!muse_idle_composer(&cropped));
    assert!(muse_verified_process_idle_composer(&cropped));
    for footer in [
        "unrecognized Auto-review",
        "watermelon-preview · xhigh · /work/project · unknown",
    ] {
        assert!(!muse_auto_review_idle_composer(&frame("❯", footer)));
    }
    assert!(!muse_auto_review_idle_composer(&frame(
        "❯",
        &format!("{auto_review}\n{yolo}")
    )));
    assert!(!muse_auto_review_idle_composer(&frame(
        "❯ operator draft",
        auto_review
    )));
    let prompt = "literal $(not-a-command) ; 'quoted'\nsecond line";
    assert!(muse_prompt_is_exact_composer(
        &frame(&format!("❯ {prompt}"), auto_review),
        prompt
    ));
    assert!(!muse_prompt_is_exact_composer(
        &frame(&format!("❯ other draft {prompt}"), auto_review),
        prompt
    ));
    assert!(!muse_prompt_is_exact_composer(
        &frame(&format!("❯ {prompt} appended draft"), auto_review),
        prompt
    ));
}

#[test]
fn native_codex_group_leader_remains_pinned_with_foreground_helpers() {
    let herdr = FakeExecutable::new("{}");
    for name in ["codex", "launcher"] {
        let executable = herdr.root.join(name);
        install_test_elf("/bin/sleep", &executable);
        let process = spawn_test_process(&executable);
        let mut document = process_document(process.0.id(), &executable);
        document["result"]["process_info"]["shell_pid"] = Value::from(1);
        let processes = document["result"]["process_info"]["foreground_processes"]
            .as_array_mut()
            .unwrap();
        let mut helper = processes[0].clone();
        helper["pid"] = Value::from(2);
        processes.insert(0, helper);
        herdr.set_response(&document.to_string());
        let identity = live_custom_process(u64::from(process.0.id()))
            .unwrap()
            .identity;
        assert_eq!(
            herdr.client().harness_identity("pane", "codex").unwrap(),
            (name == "codex").then_some(identity.clone())
        );
        assert_eq!(
            herdr
                .client()
                .verify_harness_identity("pane", &identity)
                .unwrap(),
            name == "codex"
        );
        let mut changed = identity;
        changed.starttime_ticks += 1;
        assert!(!herdr
            .client()
            .verify_harness_identity("pane", &changed)
            .unwrap());
    }
}

/// A Herdr stand-in for `agent start` refused while a new pane's login shell starts.
///
/// `agent start` answers `agent_pane_busy` for the first `busy_starts` calls. `pane
/// process-info` walks `processes` one report per call and then repeats the last one; a `null`
/// entry fails that report, and every report takes `report_delay` seconds. Every call is logged.
fn configure_busy_start(herdr: &FakeExecutable, busy_starts: u32, processes: Value, delay: f64) {
    let frame = serde_json::json!({
        "busy_starts": busy_starts,
        "processes": processes,
        "report_delay": delay,
        "terminal": "terminal-1",
        "replace_terminal_after_report": null,
        "starts": 0,
        "reports": 0,
        "calls": [],
        "panes": {
            "pane": {"tab_id": "tab-new", "label": "worker"},
            "old": {"tab_id": "tab-old", "label": "worker"}
        }
    });
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    fs::write(
        herdr.root.join("herdr"),
        r#"#!/usr/bin/python3
import json, pathlib, sys, time
path = pathlib.Path(__file__).with_name('frame.json')
state = json.loads(path.read_text())
args = sys.argv[1:]
state['calls'].append(args)
def finish(status=0):
    path.write_text(json.dumps(state))
    raise SystemExit(status)
if args[:2] == ['agent', 'start']:
    state['starts'] += 1
    if state['starts'] <= state['busy_starts']:
        pane = args[args.index('--pane') + 1]
        print(json.dumps({'id': 'cli:agent:start', 'error': {'code': 'agent_pane_busy',
              'message': 'agent target pane %s is not an available shell' % pane}}),
              file=sys.stderr)
        finish(1)
    print(json.dumps({'id': 'cli:agent:start', 'result': {'agent': {'name': args[2]}}}))
elif args[:2] == ['pane', 'get']:
    pane = state['panes'][args[2]]
    print(json.dumps({'result': {'pane': {
        'pane_id': args[2], 'workspace_id': 'workspace', 'cwd': '/work/project',
        'tab_id': pane['tab_id'], 'terminal_id': state['terminal'], 'agent': None,
        'agent_status': 'unknown', 'agent_session': None}}}))
elif args[:2] == ['pane', 'process-info']:
    time.sleep(state['report_delay'])
    index = min(state['reports'], len(state['processes']) - 1)
    state['reports'] += 1
    if state['replace_terminal_after_report'] == state['reports']:
        state['terminal'] = 'terminal-2'
    report = state['processes'][index]
    if report is None:
        print('process-info is not ready', file=sys.stderr)
        finish(1)
    print(json.dumps({'result': {'process_info': dict(report, pane_id=args[3])}}))
else:
    print('unexpected test command %r' % args, file=sys.stderr)
    finish(2)
finish()
"#,
    )
    .unwrap();
}

fn busy_frame(herdr: &FakeExecutable) -> Value {
    serde_json::from_slice(&fs::read(herdr.root.join("frame.json")).unwrap()).unwrap()
}

/// The pane's own shell with `others` (pid, argv0) in its foreground process group `group`.
fn shell_report(group: u32, others: &[(u32, &str)]) -> Value {
    let mut processes = vec![serde_json::json!({"pid": 4242, "name": "bash", "argv": ["-bash"]})];
    processes.extend(
        others
            .iter()
            .map(|(pid, argv0)| serde_json::json!({"pid": pid, "name": argv0, "argv": [argv0]})),
    );
    serde_json::json!({
        "shell_pid": 4242,
        "foreground_process_group_id": group,
        "foreground_processes": processes
    })
}

/// Every call that names a pane names `pane`, the one the launch was offered.
fn assert_only_pane_targeted(frame: &Value) {
    for call in frame["calls"].as_array().unwrap() {
        let call: Vec<&str> = call
            .as_array()
            .unwrap()
            .iter()
            .map(|word| word.as_str().unwrap())
            .collect();
        let target = match call[..2] {
            ["pane", "get"] => call[2],
            ["pane", "process-info"] | ["agent", "start"] => {
                call[call.iter().position(|word| *word == "--pane").unwrap() + 1]
            }
            _ => panic!("unexpected call {call:?}"),
        };
        assert_eq!(target, "pane", "{call:?} must target only the created pane");
    }
}

#[test]
fn start_waits_out_a_noisy_login_shell_and_launches_in_the_same_pane() {
    // The login shell prints errors from its startup files and runs their command
    // substitutions in its own process group, so Herdr refuses until it is alone there.
    let herdr = FakeExecutable::new("{}");
    let startup = serde_json::json!([
        shell_report(4242, &[(4300, "grep")]),
        shell_report(4242, &[(4301, "uuidgen")]),
        shell_report(4400, &[(4400, "tput")]),
        shell_report(4242, &[]),
    ]);
    configure_busy_start(&herdr, 1, startup, 0.0);
    let result =
        herdr
            .client()
            .start_agent("worker", "claude", "pane", &[], Duration::from_secs(10));
    assert!(result.is_ok(), "{result:?}");
    let frame = busy_frame(&herdr);
    assert_eq!(
        frame["starts"], 2,
        "one refused launch, then one in the same pane"
    );
    assert!(frame["reports"].as_u64().unwrap() >= 5, "{frame}");
    assert_only_pane_targeted(&frame);
}

#[test]
fn start_survives_a_slow_and_failing_foreground_process_report() {
    let herdr = FakeExecutable::new("{}");
    let reports = serde_json::json!([Value::Null, shell_report(4242, &[])]);
    configure_busy_start(&herdr, 2, reports, 0.3);
    let result =
        herdr
            .client()
            .start_agent("worker", "claude", "pane", &[], Duration::from_secs(20));
    assert!(result.is_ok(), "{result:?}");
    let frame = busy_frame(&herdr);
    assert_eq!(frame["starts"], 3);
    assert_only_pane_targeted(&frame);
}

#[test]
fn start_never_moves_to_a_same_label_pane_and_reports_what_kept_its_own_busy() {
    // Another tab carries the same label; the launch still waits only on its own pane and
    // gives up at the deadline with what it saw there.
    let herdr = FakeExecutable::new("{}");
    let busy = serde_json::json!([shell_report(5000, &[(5000, "vim")])]);
    configure_busy_start(&herdr, u32::MAX, busy, 0.0);
    let error = herdr
        .client()
        .start_agent("worker", "claude", "pane", &[], Duration::from_secs(1))
        .unwrap_err()
        .to_string();
    assert!(error.contains("agent_pane_busy"), "{error}");
    assert!(
        error.contains("pane pane did not become an available shell within 1s"),
        "{error}"
    );
    assert!(
        error.contains("group 5000 holds [4242 -bash, 5000 vim]"),
        "{error}"
    );
    let frame = busy_frame(&herdr);
    assert_eq!(
        frame["starts"], 1,
        "a pane that never frees is never offered again"
    );
    assert_only_pane_targeted(&frame);
}

#[test]
fn start_stops_when_its_pane_holds_another_terminal() {
    let herdr = FakeExecutable::new("{}");
    configure_busy_start(
        &herdr,
        u32::MAX,
        serde_json::json!([shell_report(4242, &[])]),
        0.0,
    );
    let mut frame = busy_frame(&herdr);
    frame["replace_terminal_after_report"] = Value::from(1);
    fs::write(herdr.root.join("frame.json"), frame.to_string()).unwrap();
    // The shell looks available once, then the pane holds a different terminal.
    let error = herdr
        .client()
        .start_agent("worker", "claude", "pane", &[], Duration::from_secs(10))
        .unwrap_err()
        .to_string();
    assert!(error.contains("not the Some(\"terminal-1\")"), "{error}");
    let frame = busy_frame(&herdr);
    assert_eq!(frame["starts"], 1, "{frame}");
    assert_only_pane_targeted(&frame);
}
