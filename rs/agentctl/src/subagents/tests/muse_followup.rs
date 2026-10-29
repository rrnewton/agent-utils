//! #505 agentctl-muse-followup: a queue drain types an ordinary prompt into a Muse pane whose
//! turn is running, and Muse steers the turn with it.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

fn start_busy_muse(fixture: &Fixture, header: bool) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    let mut editor = MuseEditor::new("Launch overrides", header, None);
    editor.busy = true;
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    manager
}

fn held() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_millis(50),
        ..DrainOptions::default()
    }
}

fn keys(fixture: &Fixture) -> Vec<String> {
    fixture
        .client
        .keys_sent
        .lock()
        .unwrap()
        .iter()
        .map(|(_, key)| key.clone())
        .collect()
}

fn no_input(fixture: &Fixture) {
    assert!(fixture.client.literal_pastes.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
}

#[test]
fn a_working_muse_takes_an_ordinary_prompt_by_steering_its_turn() {
    for header in [false, true] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, header);
        assert_eq!(manager.status("worker").unwrap()["agent_status"], "working");
        let prompt = "hold: stop after the current step\nand report";
        let result = manager
            .send_identified_with_runtime("worker", prompt, held(), "hold", &Clock::default())
            .unwrap();
        assert_eq!(result.delivered, ["hold"], "header={header}");
        assert_eq!(
            *fixture.client.literal_pastes.lock().unwrap(),
            [(
                "owned".to_owned(),
                format!("{BRACKETED_PASTE_START}{prompt}{BRACKETED_PASTE_END}")
            )]
        );
        // One Enter, and no key that could interrupt the turn.
        assert_eq!(keys(&fixture), ["Enter"]);
        let editor = fixture.client.muse_editor.lock().unwrap();
        let screen = editor.as_ref().unwrap().screen();
        assert!(screen.contains(&format!("❯ {prompt}")));
        assert!(screen.contains("esc to interrupt"), "the turn kept running");
        drop(editor);
        let processed = manager.queue("worker").unwrap().join("processed/hold.json");
        assert_eq!(
            agent::read_private_json(&processed).unwrap()["delivery_state"],
            "processed"
        );
    }
}

#[test]
fn a_follow_up_muse_never_accepts_is_taken_back_out_and_stays_pending() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .ignore_enter = true;
    let error = manager
        .send_identified_with_runtime("worker", "never taken", held(), "never", &Clock::default())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("taken back out"), "{error}");
    let sent = keys(&fixture);
    // Enter is pressed again only while the exact prompt is still staged, then the clearing keys
    // take it out; Esc, which interrupts a Muse turn, is never sent.
    let enters = sent.iter().take_while(|key| *key == "Enter").count();
    assert!(enters > 1, "{sent:?}");
    assert_eq!(&sent[enters..], ["ctrl+e", "ctrl+u"]);
    assert_eq!(fixture.client.literal_pastes.lock().unwrap().len(), 1);
    assert!(fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .staged()
        .is_none());
    let retained = agent::read_private_json(&error.undelivered().unwrap().artifact).unwrap();
    assert_eq!(retained["delivery_state"], "pending");
    assert!(retained.get("possibly_submitted").is_none());
    assert!(!manager
        .queue("worker")
        .unwrap()
        .join("failed/never.json")
        .exists());
}

#[test]
fn an_operator_draft_an_approval_form_or_a_slash_command_holds_the_prompt_untyped() {
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, false);
    let set = |change: &dyn Fn(&mut MuseEditor)| {
        change(fixture.client.muse_editor.lock().unwrap().as_mut().unwrap());
    };

    set(&|editor| editor.draft("owner draft"));
    let error = manager
        .send("worker", "behind a draft", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("nothing was typed"), "{error}");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();

    set(&|editor| {
        editor.key("ctrl+u");
        editor.approval = true;
    });
    let error = manager
        .send("worker", "at an approval", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    // The approval form waits for a person, so the pane reads blocked, not working (#678).
    assert!(error.to_string().contains("is blocked"), "{error}");
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "blocked");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();

    // Slash commands open Muse menus at once even mid-turn, so they wait for an idle editor.
    set(&|editor| editor.approval = false);
    let error = manager.send("worker", "/model", held()).unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("last status=working"), "{error}");
    no_input(&fixture);
}

// Owner-directed: an adopted Muse pane takes a follow-up at whatever footer mode it shows, so an
// adopted coordinator running in YOLO still receives messages while it works.
#[test]
fn an_adopted_working_muse_takes_follow_ups_at_any_footer_mode() {
    for mode in ["YOLO", "", "Launch overrides", "Auto-review"] {
        let fixture = Fixture::new();
        fixture.prepare_foreign();
        fixture
            .client
            .custom_reported
            .store(true, Ordering::Relaxed);
        *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
        *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new(mode, true, None));
        let mut options = fixture.adopt_options();
        options.harness = "muse".to_owned();
        let manager = fixture.manager();
        manager.adopt("foreign", options).unwrap();
        *fixture.client.status.lock().unwrap() = Some("working".to_owned());
        fixture
            .client
            .muse_editor
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .busy = true;
        assert_eq!(
            manager.status("foreign").unwrap()["agent_status"],
            "working",
            "{mode:?}"
        );
        let result = manager
            .send_identified_with_runtime(
                "foreign",
                "steer the adopted turn",
                held(),
                "steer",
                &Clock::default(),
            )
            .unwrap();
        assert_eq!(result.delivered, ["steer"], "{mode:?}");
        assert_eq!(keys(&fixture), ["Enter"], "{mode:?}");
        let editor = fixture.client.muse_editor.lock().unwrap();
        let screen = editor.as_ref().unwrap().screen();
        assert!(screen.contains("❯ steer the adopted turn"), "{mode:?}");
        assert!(
            screen.contains("esc to interrupt"),
            "{mode:?}: the turn kept running"
        );
    }
}

#[test]
fn an_adopted_muse_whose_pane_reports_no_agent_still_takes_working_follow_ups() {
    // #697 send-muse-while-working: Herdr has no Muse manifest (#691), so an
    // adopted Muse pane can report no agent kind at all, and once the harness
    // re-execs the pinned-process correction no longer names it either. The
    // record's harness must still drive readiness, or a busy adopted Muse
    // waits for idle like the other team's coordinator did.
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new("YOLO", true, None));
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    let record_path = fixture.root.join("registry/agents/foreign/agent.json");
    let mut record = agent::read_private_json(&record_path).unwrap();
    record
        .as_object_mut()
        .unwrap()
        .insert("anchor_rule".to_owned(), serde_json::Value::Null);
    agent::atomic_json(&record_path, &record).unwrap();
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    fixture
        .client
        .muse_editor
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .busy = true;
    // Readiness now reaches the typing stage instead of waiting out the ready
    // timeout for idle; typing itself still refuses without a process anchor,
    // pending with nothing typed. (The Fake cannot express a re-exec'd harness
    // whose shell identity verifies while the foreground pin does not.)
    let error = manager
        .send_identified_with_runtime(
            "foreign",
            "steer without a reported kind",
            held(),
            "steer",
            &Clock::default(),
        )
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(
        error
            .to_string()
            .contains("foreground harness process anchor"),
        "{error}"
    );
    assert!(
        !error.to_string().contains("did not become idle"),
        "{error}"
    );
    no_input(&fixture);
}

#[test]
fn a_revived_adopted_muse_on_the_native_adapter_still_verifies_follow_ups() {
    // #697 send-muse-while-working: revive re-owns an adopted pane under adapter `herdr`;
    // a Muse record there must keep the verified-composer path or busy sends wait.
    let fixture = Fixture::new();
    let manager = start_busy_muse(&fixture, true);
    let mut record = manager.load("worker").unwrap();
    record.adapter = "herdr".to_owned();
    let client = manager.workspace_client(&record);
    assert!(agent::AgentApi::verifies_muse_followups(&client));
}

#[test]
fn a_paste_never_seen_as_the_exact_editor_is_taken_back_out_and_quarantined() {
    // Idle and running turns alike: no Enter is sent, and the paste is cleared out of the editor
    // rather than left there unsubmitted. The prompt is quarantined as possibly submitted, so it
    // is neither typed again on every drain nor holds the queue.
    for busy in [false, true] {
        let fixture = Fixture::new();
        let manager = start_busy_muse(&fixture, true);
        {
            let mut editor = fixture.client.muse_editor.lock().unwrap();
            let editor = editor.as_mut().unwrap();
            editor.busy = busy;
            editor.garble = true;
        }
        if !busy {
            *fixture.client.status.lock().unwrap() = Some("done".to_owned());
        }
        let error = manager
            .send_identified_with_runtime(
                "worker",
                "never exact",
                held(),
                "garbled",
                &Clock::default(),
            )
            .unwrap_err();
        assert_eq!(error.exit_code(), 76, "busy={busy}: {error}");
        assert!(error.to_string().contains("taken back out"), "{error}");
        let sent = keys(&fixture);
        assert!(
            !sent.iter().any(|key| key == "Enter" || key == "esc"),
            "{sent:?}"
        );
        assert!(sent.iter().any(|key| key == "ctrl+u"), "{sent:?}");
        assert!(fixture
            .client
            .muse_editor
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .staged()
            .is_none());
        let queue = manager.queue("worker").unwrap();
        assert!(queue.join("failed/garbled.json").is_file());
        assert!(!queue.join("inbox/garbled.json").exists());
    }
}
