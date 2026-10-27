use super::*;

// Captured Codex screens, redacted; see testdata/harness-turns/README.md.
const CODEX_LONG: &str = include_str!("../../../testdata/harness-turns/codex-long-turn-queued.txt");
const CODEX_INTERRUPTED: &str =
    include_str!("../../../testdata/harness-turns/codex-interrupted-composer.txt");
const CODEX_AFTER: &str = include_str!("../../../testdata/harness-turns/codex-after-interrupt.txt");
const CLAUDE_WORKING: &str =
    include_str!("../../../testdata/harness-turns/claude-working-queued.txt");

fn start<'f>(fixture: &'f Fixture, harness: &str) -> ManagedAgents<'f, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: harness.to_owned(),
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

fn pane(manager: &ManagedAgents<'_, Fake>) -> String {
    manager.load("worker").unwrap().pane_id.unwrap()
}

#[test]
fn status_shows_a_46_hour_turn_and_the_inputs_queued_behind_it() {
    let fixture = Fixture::new();
    let manager = start(&fixture, "codex");
    show(&fixture, "working", CODEX_LONG);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["agent_status"], "working");
    let turn = &status["turn"];
    assert_eq!(turn["age_seconds"], 168_854.0, "{status}");
    assert_eq!(turn["harness_queued"], 4);
    assert_eq!(turn["harness_queue"], "codex-end-of-turn");
    assert_eq!(turn["undelivered"], 4);
    assert!(turn["summary"]
        .as_str()
        .unwrap()
        .starts_with("inside one turn for 46 h 54 m; 4 inputs queued"));
    let listed = manager.list().unwrap();
    assert_eq!(listed[0]["turn"]["harness_queued"], 4, "list shows it too");

    // An idle agent with nothing waiting has no turn to show.
    show(&fixture, "idle", "• done\n\n» Ask Codex to do anything\n");
    assert!(manager.status("worker").unwrap().get("turn").is_none());
}

#[test]
fn a_pane_herdr_relabels_is_still_the_agent_while_its_pinned_harness_runs_there() {
    let fixture = Fixture::new();
    let manager = start(&fixture, "claude");
    let pane = pane(&manager);
    // Herdr re-detected the Claude Code pane as a finished Muse agent and dropped its name.
    fixture
        .client
        .launches
        .lock()
        .unwrap()
        .entry(pane.clone())
        .and_modify(|launch| launch.0 = "muse".to_owned());
    fixture
        .client
        .agent_get_missing
        .store(true, Ordering::Relaxed);
    show(&fixture, "done", CLAUDE_WORKING);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["probe_error"], Value::Null, "{status}");
    assert_eq!(status["agent"], "claude");
    assert_eq!(
        status["agent_status"], "working",
        "the screen, not the wrong detector"
    );
    assert_eq!(status["turn"]["tokens"], 8_400);

    // Once the screen shows no running turn, the agent reads idle.
    show(
        &fixture,
        "done",
        "● All done.\n────\n❯\u{a0}\n────\n  ⏵⏵ auto mode on\n",
    );
    assert_eq!(manager.status("worker").unwrap()["agent_status"], "idle");

    // Without the pinned harness in the pane, the relabelled pane is not trusted.
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert(pane, 999);
    let status = manager.status("worker").unwrap();
    assert_eq!(status["agent_status"], "unknown", "{status}");
    assert!(status["probe_error"].is_string());
}

#[test]
fn interrupt_and_submit_presses_escape_then_enter_on_the_restored_queue() {
    let fixture = Fixture::new();
    let manager = start(&fixture, "codex");
    show(&fixture, "working", CODEX_AFTER);
    fixture.client.screens.lock().unwrap().extend([
        CODEX_LONG.to_owned(),
        CODEX_INTERRUPTED.to_owned(),
        CODEX_AFTER.to_owned(),
    ]);
    fixture.client.keys_sent.lock().unwrap().clear();
    let report = manager.interrupt_and_submit("worker").unwrap();
    let keys: Vec<String> = fixture
        .client
        .keys_sent
        .lock()
        .unwrap()
        .iter()
        .map(|(_, key)| key.clone())
        .collect();
    assert_eq!(keys, ["Escape", "Enter"]);
    assert_eq!(report["interrupted"], true, "{report}");
    assert_eq!(report["submitted"], true);
    assert_eq!(report["queued_before"], 4);
    assert_eq!(
        report["detail"],
        "interrupted the turn after 46 h 54 m and submitted the 4 queued input(s) it put back in the input box"
    );
}

#[test]
fn nothing_is_pressed_when_no_turn_runs_and_nothing_is_queued() {
    let fixture = Fixture::new();
    let manager = start(&fixture, "codex");
    show(&fixture, "idle", "• done\n\n» Ask Codex to do anything\n");
    fixture.client.keys_sent.lock().unwrap().clear();
    let report = manager.interrupt_and_submit("worker").unwrap();
    assert_eq!(report["interrupted"], false);
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
}

#[test]
fn an_urgent_send_the_harness_takes_at_once_interrupts_nothing() {
    let fixture = Fixture::new();
    let manager = start(&fixture, "codex");
    show(&fixture, "idle", "• done\n\n» Ask Codex to do anything\n");
    fixture.client.keys_sent.lock().unwrap().clear();
    let result = manager
        .send_urgent(
            "worker",
            "switch to the crash work",
            DrainOptions {
                ready_timeout: Duration::ZERO,
                working_timeout: Duration::from_millis(50),
                ..DrainOptions::default()
            },
            Some("urgent-1"),
            Duration::ZERO,
        )
        .unwrap();
    assert_eq!(result["outcome"], "delivered", "{result}");
    assert_eq!(result["urgent"]["interrupted"], false);
    assert!(!fixture
        .client
        .keys_sent
        .lock()
        .unwrap()
        .iter()
        .any(|(_, key)| key == "Escape"));
}
