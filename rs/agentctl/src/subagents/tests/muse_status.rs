use super::herdr_compatibility::MuseEditor;
use super::*;

const DIVIDER: &str = "────────────────────────────────────────";

fn start_muse(fixture: &Fixture) -> ManagedAgents<'_, Fake> {
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
    manager
}

fn show(fixture: &Fixture, status: &str, screen: &str) {
    *fixture.client.status.lock().unwrap() = Some(status.to_owned());
    *fixture.client.screen.lock().unwrap() = Some(screen.to_owned());
}

fn held() -> DrainOptions {
    DrainOptions {
        ready_timeout: Duration::ZERO,
        working_timeout: Duration::from_millis(50),
        ..DrainOptions::default()
    }
}

fn no_input(fixture: &Fixture) {
    assert!(fixture.client.literal_pastes.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
}

// Ported from py/tests/test_herdr_subagents.py
// test_verified_muse_yolo_composer_reports_idle_staged_and_paused.
#[test]
fn verified_muse_yolo_composer_reports_idle_staged_and_paused() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    let footer = "kiki · xhigh · /work · YOLO\n";
    let screen = format!("old transcript\n{DIVIDER}\n❯\n{DIVIDER}\n{footer}");
    show(&fixture, "done", &screen);
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");

    let screen = screen.replace("❯\n", "❯ queued owner prompt\n");
    show(&fixture, "done", &screen);
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");

    let screen = screen
        .replace("❯ queued owner prompt\n", "❯\n")
        .replace(footer, &format!("Goal (paused)\n{footer}"));
    show(&fixture, "done", &screen);
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "paused");
}

// Ported from py/tests/test_herdr_subagents.py
// test_owned_muse_delivers_at_every_footer_mode_and_through_a_collapsed_paste.
//
// #395 muse-launch-overrides-input: a session agentctl launched takes input at whatever approval
// mode its launch chose, through the same process, staged-editor and transcript proofs. A long
// paste may show only as Muse's placeholder with its paste hint on the footer line, and a
// versioned header Muse has scrolled away does not matter once the process is pinned.
#[test]
fn owned_muse_delivers_at_every_footer_mode_and_through_a_collapsed_paste() {
    let long = "first line of a long brief\r\nsecond line, café 🙂, and the rest of it";
    for (mode, header, collapse_over, prompt) in [
        ("YOLO", true, None, "a short literal $(prompt)"),
        ("Launch overrides", true, None, "a short literal $(prompt)"),
        ("", true, None, "a short literal $(prompt)"),
        ("Launch overrides", true, Some(40), long),
        ("Launch overrides", false, Some(40), long),
        ("Auto-review", false, Some(40), long),
    ] {
        let fixture = Fixture::new();
        let manager = start_muse(&fixture);
        *fixture.client.status.lock().unwrap() = Some("done".to_owned());
        *fixture.client.muse_editor.lock().unwrap() =
            Some(MuseEditor::new(mode, header, collapse_over));
        let case = format!("{mode:?} header={header} collapse={collapse_over:?}");
        assert_eq!(
            manager.status("worker").unwrap()["agent_status"],
            "idle",
            "{case}"
        );
        let result = manager.send("worker", prompt, held()).unwrap();
        assert_eq!(result.delivered.len(), 1, "{case}: {result:?}");
        assert_eq!(
            fixture
                .client
                .literal_pastes
                .lock()
                .unwrap()
                .iter()
                .map(|(_, text)| text.clone())
                .collect::<Vec<_>>(),
            [format!(
                "{BRACKETED_PASTE_START}{prompt}{BRACKETED_PASTE_END}"
            )],
            "{case}"
        );
        assert_eq!(fixture.client.keys_sent.lock().unwrap().len(), 1, "{case}");
    }
}

// New: every refusal raised before any input leaves the prompt pending (exit 75), never
// quarantined as possibly submitted (#395 muse-launch-overrides-input).
#[test]
fn a_muse_refusal_before_any_input_keeps_the_prompt_pending() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    *fixture.client.muse_editor.lock().unwrap() =
        Some(MuseEditor::new("Launch overrides", true, None));
    let error = manager
        .send("worker", "colored \u{1b}[31m text", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("nothing was typed"), "{error}");
    no_input(&fixture);
    let retained = agent::read_private_json(&error.undelivered().unwrap().artifact).unwrap();
    assert_eq!(retained["delivery_state"], "pending");
    assert!(retained.get("possibly_submitted").is_none());

    // The same refusal through the adapter itself is not staged, whatever the footer says.
    let record = manager.load("worker").unwrap();
    let pane = record.pane_id.clone().unwrap();
    let client = manager.workspace_client(&record);
    let error = AgentApi::run(&client, &pane, "nul \0 byte").unwrap_err();
    assert_eq!(error.kind(), AdapterErrorKind::NotStaged, "{error}");
    no_input(&fixture);
}

// Ported from py/tests/test_agentctl_adopt.py
// test_suggestion13_foreign_muse_takes_input_in_any_footer_mode, standard mode. Its YOLO and
// Launch overrides modes are herdr_compatibility.rs
// herdr_093_foreign_muse_takes_input_at_every_footer_mode.
#[test]
fn foreign_muse_standard_mode_takes_input() {
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new("", true, None));
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    assert_eq!(manager.status("foreign").unwrap()["agent_status"], "idle");
    let result = manager
        .send("foreign", "typed at standard", held())
        .unwrap();
    assert_eq!(result.delivered.len(), 1, "{result:?}");
    assert_eq!(
        fixture
            .client
            .literal_pastes
            .lock()
            .unwrap()
            .iter()
            .map(|(_, text)| text.clone())
            .collect::<Vec<_>>(),
        [format!(
            "{BRACKETED_PASTE_START}typed at standard{BRACKETED_PASTE_END}"
        )]
    );
    assert_eq!(fixture.client.keys_sent.lock().unwrap().len(), 1);
    assert_eq!(manager.load("foreign").unwrap().adapter, "herdr-foreign");
}

// New: a paused goal outranks the idle Auto-review editor, and staged owner input is never
// typed over; both keep the prompt queued until the editor is idle again.
#[test]
fn paused_goal_and_staged_editor_hold_input_until_idle() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    let footer = "kiki · xhigh · /work · Auto-review\n";
    let idle = format!("Muse Code 1.4.0\n{DIVIDER}\n❯\n{DIVIDER}\n{footer}");
    let paused = idle.replace(footer, &format!("Goal (paused)\n{footer}"));
    let staged = idle.replace("❯\n", "❯ owner draft\n");
    assert!(crate::client::muse_auto_review_idle_composer(&paused));

    for (state, screen, text) in [
        ("paused", &paused, "held while paused"),
        ("staged", &staged, "held while staged"),
    ] {
        show(&fixture, "done", screen);
        assert_eq!(manager.status("worker").unwrap()["agent_status"], state);
        let error = manager.send("worker", text, held()).unwrap_err();
        assert!(matches!(error, AgentError::Pending(_)), "{state}: {error}");
        assert_eq!(error.exit_code(), 75);
        assert!(
            error.to_string().contains(&format!("last status={state}")),
            "{error}"
        );
        no_input(&fixture);
    }
    assert!(manager
        .drain("worker", held())
        .unwrap()
        .delivered
        .is_empty());
    no_input(&fixture);

    // The owner resumes the goal and clears the editor: both prompts are delivered once, in order.
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::default());
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");
    let delivered = manager.drain("worker", held()).unwrap().delivered;
    assert_eq!(delivered.len(), 2, "{delivered:?}");
    let pastes = fixture
        .client
        .literal_pastes
        .lock()
        .unwrap()
        .iter()
        .map(|(_, text)| text.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        pastes,
        [
            format!("{BRACKETED_PASTE_START}held while paused{BRACKETED_PASTE_END}"),
            format!("{BRACKETED_PASTE_START}held while staged{BRACKETED_PASTE_END}"),
        ]
    );
}

// New: the process pin is checked again right after the screen read and before the screen is
// interpreted, as py/agentctl/subagents.py does, so a screen read across a process change is
// never reported as that screen's state, not even as a trust prompt.
#[test]
fn process_pin_is_checked_before_the_screen_is_interpreted() {
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    show(
        &fixture,
        "idle",
        &format!("Muse Code 1.4.0\n{DIVIDER}\n❯\n{DIVIDER}\nkiki · xhigh · /work · Auto-review\n"),
    );
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    show(
        &fixture,
        "idle",
        "Do you trust this workspace?\n❯ Yes\n  No\n",
    );
    let trust = manager.status("foreign").unwrap();
    assert!(
        trust["probe_error"]
            .as_str()
            .unwrap()
            .contains("trust prompt"),
        "{trust}"
    );
    fixture
        .client
        .change_foreign_shell_on_read
        .store(true, Ordering::Relaxed);
    let changed = manager.status("foreign").unwrap();
    assert_eq!(changed["agent_status"], "unknown");
    let error = changed["probe_error"].as_str().unwrap();
    assert!(error.contains("shell process"), "{changed}");
    assert!(!error.contains("trust prompt"), "{changed}");
    no_input(&fixture);
}
