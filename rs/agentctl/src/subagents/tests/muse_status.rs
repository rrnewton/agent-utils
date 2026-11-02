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
// test_verified_muse_yolo_composer_reports_idle_staged_and_paused. Since #734 a paused
// Muse goal reads idle, not paused: it is a planning state, not a hold.
#[test]
fn verified_muse_yolo_composer_reports_idle_and_staged() {
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
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");
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

// #700 chat-receipt-idle-muse: a prompt typed into an idle Muse is proved by the screen (it left
// the editor for a new user turn), so the drain reports it printed and a chat request earns its
// ✅ receipt, as one steered into a working Muse does. Before, an idle Muse's delivery was
// confirmed but never reported printed, so the bridge saved no receipt.
#[test]
fn a_prompt_typed_into_an_idle_muse_is_reported_printed() {
    struct Noting {
        inner: agent::SystemRuntime,
        printed: Mutex<Vec<String>>,
    }
    impl agent::AgentRuntime for Noting {
        fn monotonic(&self) -> Duration {
            self.inner.monotonic()
        }
        fn sleep(&self, duration: Duration) {
            self.inner.sleep(duration);
        }
        fn prompt_printed(&self, message_id: &str) {
            self.printed.lock().unwrap().push(message_id.to_owned());
        }
    }
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    *fixture.client.muse_editor.lock().unwrap() =
        Some(MuseEditor::new("Launch overrides", true, None));
    let runtime = Noting {
        inner: agent::SystemRuntime::default(),
        printed: Mutex::new(Vec::new()),
    };
    let id = format!("chat-{}", "a".repeat(64));
    let result = manager
        .send_identified_with_runtime("worker", "an owner request", held(), &id, &runtime)
        .unwrap();
    assert_eq!(result.delivered, [id.as_str()], "{result:?}");
    assert_eq!(*runtime.printed.lock().unwrap(), [id]);
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

// #734 chat-status-disagreement: a staged editor holds input until it clears, but a
// paused Muse goal does not: Muse accepts and acts on typed input while its goal is
// paused, so an owner prompt typed then is delivered, not queued.
#[test]
fn staged_editor_holds_input_until_idle_and_a_paused_goal_does_not() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    let footer = "kiki · xhigh · /work · Auto-review\n";
    let idle = format!("Muse Code 1.4.0\n{DIVIDER}\n❯\n{DIVIDER}\n{footer}");
    let staged = idle.replace("❯\n", "❯ owner draft\n");
    show(&fixture, "done", &staged);
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "staged");
    let error = manager
        .send("worker", "held while staged", held())
        .unwrap_err();
    assert!(matches!(error, AgentError::Pending(_)), "{error}");
    assert_eq!(error.exit_code(), 75);
    assert!(error.to_string().contains("last status=staged"), "{error}");
    no_input(&fixture);
    assert!(manager
        .drain("worker", held())
        .unwrap()
        .delivered
        .is_empty());
    no_input(&fixture);

    // The editor clears: the prompt is delivered once.
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::default());
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");
    let delivered = manager.drain("worker", held()).unwrap().delivered;
    assert_eq!(delivered.len(), 1, "{delivered:?}");
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
        [format!(
            "{BRACKETED_PASTE_START}held while staged{BRACKETED_PASTE_END}"
        )]
    );
}

// #734 chat-status-disagreement (coordinator-directed): a Muse pane whose footer shows
// `Goal (paused · 1d 17h 40m)` with an empty editor takes an owner request at once. A
// paused goal is a Muse planning state, not `agentctl pause NAME`.
#[test]
fn a_paused_goal_with_an_empty_editor_takes_an_owner_request() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    let mut editor = MuseEditor::new("YOLO", true, None);
    editor.goal_notice = Some("Goal (paused · 1d 17h 40m) 97% █████████▊".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["agent_status"], "idle", "{status}");
    let result = manager.send("worker", "an owner request", held()).unwrap();
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
            "{BRACKETED_PASTE_START}an owner request{BRACKETED_PASTE_END}"
        )]
    );
}

// #734 chat-status-disagreement: the incident pane's viewport was scrolled 1052 rows up, so
// the visible read held no editor or footer and the status fell through to `working` for
// hours while Herdr said `done`. The pane's bottom is read instead: an empty editor there
// is idle and takes the queued prompt; a paused goal drawn with its elapsed duration
// (`Goal (paused · 1d 17h 40m)`) still holds it.
#[test]
fn a_scrolled_viewport_reads_status_and_delivery_from_the_pane_bottom() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(MuseEditor::new("YOLO", true, None));
    *fixture.client.screen.lock().unwrap() =
        Some("◆ Context compacted\n\n❯ [from an owner via a bridge]\n  an old prompt\n".to_owned());
    fixture
        .client
        .scrolled
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["agent_status"], "idle", "{status}");
    assert_eq!(status["herdr_agent_status"], "done", "{status}");
    let result = manager
        .send("worker", "typed at the bottom", held())
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
            "{BRACKETED_PASTE_START}typed at the bottom{BRACKETED_PASTE_END}"
        )]
    );
}

#[test]
fn a_scrolled_viewport_delivers_behind_a_paused_goal_with_a_duration() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.status.lock().unwrap() = Some("done".to_owned());
    let mut editor = MuseEditor::new("YOLO", true, None);
    editor.goal_notice = Some("Goal (paused · 1d 17h 40m) 97% █████████▊".to_owned());
    *fixture.client.muse_editor.lock().unwrap() = Some(editor);
    *fixture.client.screen.lock().unwrap() =
        Some("◆ Context compacted\n\n❯ [from an owner via a bridge]\n  an old prompt\n".to_owned());
    fixture
        .client
        .scrolled
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["agent_status"], "idle", "{status}");
    assert_eq!(status["herdr_agent_status"], "done", "{status}");
    let result = manager
        .send("worker", "typed while paused", held())
        .unwrap();
    assert_eq!(result.delivered.len(), 1, "{result:?}");
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
