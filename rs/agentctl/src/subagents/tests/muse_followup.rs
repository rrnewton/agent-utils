//! #505 agentctl-muse-followup: a queue drain types an ordinary prompt into a Muse pane whose
//! turn is running, and Muse steers the turn with it. Rust only: the Python edition still waits
//! for an idle editor.
use super::herdr_compatibility::{Clock, MuseEditor};
use super::*;

const DIVIDER: &str = "────────────────────────────────────────";

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
    assert!(error.to_string().contains("last status=working"), "{error}");
    no_input(&fixture);
    fs::remove_file(&error.undelivered().unwrap().artifact).unwrap();

    // Slash commands open Muse menus at once even mid-turn, so they wait for an idle editor.
    set(&|editor| editor.approval = false);
    let error = manager.send("worker", "/model", held()).unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("last status=working"), "{error}");
    no_input(&fixture);
}

#[test]
fn an_adopted_working_muse_takes_follow_ups_only_at_its_auto_review_editor() {
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    let idle =
        format!("Muse Code 1.3.0\n{DIVIDER}\n❯\n{DIVIDER}\nmodel · high · /work · Auto-review\n");
    *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
    *fixture.client.screen.lock().unwrap() = Some(idle.clone());
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    let busy = idle
        .replace("Muse Code 1.3.0\n", "")
        .replace(
            &format!("{DIVIDER}\n❯"),
            &format!("◇ Thinking (3s · esc to interrupt)\n{DIVIDER}\n❯"),
        )
        .replace(" · Auto-review", "");
    *fixture.client.status.lock().unwrap() = Some("working".to_owned());
    *fixture.client.screen.lock().unwrap() = Some(busy);
    assert_eq!(
        manager.status("foreign").unwrap()["agent_status"],
        "working"
    );
    let error = manager
        .send("foreign", "must remain untyped", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert!(error.to_string().contains("Auto-review"), "{error}");
    no_input(&fixture);
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
