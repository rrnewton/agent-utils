//! `#1973 agentctl-headless-single-start-path-console-tab`: the ONE viewer path every
//! headless start goes through. It creates a missing console tab, replaces a blank, dead
//! or tail viewer, and leaves a live console untouched, so a wrapper that starts outside
//! Herdr (its systemd unit at boot) heals its own tab when Herdr appears.

use super::*;
use crate::subagents::control::DESIGNATION_FILE;

fn headless_agent(fixture: &Fixture) -> ManagedAgents<'_, Fake> {
    // Viewer replacement closes whole tabs; the Fake keeps a tripwire panic for
    // every other path, so these tests opt in explicitly (#1973 review).
    fixture
        .client
        .allow_close_tab
        .store(true, Ordering::Relaxed);
    let manager = fixture.manager();
    manager
        .start_headless(
            "lead",
            &fixture.root,
            HeadlessStart {
                harness: "claude".to_owned(),
                arguments: vec!["--permission-mode=bypassPermissions".to_owned()],
                launch: false,
                ..HeadlessStart::default()
            },
        )
        .unwrap();
    manager
}

fn record_viewer(manager: &ManagedAgents<'_, Fake>, tab: &str, kind: &str) {
    manager
        .set_viewer_tab_kind("lead", Some(tab.to_owned()), Some(kind))
        .unwrap();
}

fn push_pane(fixture: &Fixture, pane: &str, tab: &str, workspace: &str) {
    fixture.client.panes.lock().unwrap().push(Pane {
        pane_id: pane.to_owned(),
        tab_id: tab.to_owned(),
        workspace_id: workspace.to_owned(),
    });
    // A viewer tab carries its agent's name: that label is the ownership proof.
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert(tab.to_owned(), "lead".to_owned());
}

fn relabel(fixture: &Fixture, tab: &str, label: &str) {
    fixture
        .client
        .labels
        .lock()
        .unwrap()
        .insert(tab.to_owned(), label.to_owned());
}

/// The pane runs a real process (a live console), not its bare shell.
fn pane_busy(fixture: &Fixture) {
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
}

/// The pane is a blank shell: only its shell is in the foreground.
fn pane_blank(fixture: &Fixture) {
    fixture.client.custom_alive.store(false, Ordering::Relaxed);
    fixture
        .client
        .custom_at_idle_shell
        .store(true, Ordering::Relaxed);
}

fn creates(fixture: &Fixture) -> usize {
    fixture.client.environments.lock().unwrap().len()
}

#[test]
fn a_missing_viewer_is_created_as_a_console() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_tab().as_deref(), Some(tab.as_str()));
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    let commands = fixture.client.viewer_commands.lock().unwrap();
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(commands[0].1.contains(" console lead"), "{commands:?}");
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
}

#[test]
fn a_killed_viewer_comes_back() {
    // The recorded tab has no live pane (killed, or Herdr restarted): a fresh console
    // tab replaces the stale record instead of the record vetoing recreation forever.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5");
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_tab().as_deref(), Some(tab.as_str()));
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    assert_eq!(fixture.client.viewer_commands.lock().unwrap().len(), 1);
}

#[test]
fn a_blank_shell_viewer_is_replaced_by_a_console() {
    // Restore-after-reboot shape: Herdr brings the tab back as a blank shell while the
    // record still calls it a console. The blank tab closes and a console takes its place.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_blank(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5");
    assert_eq!(
        *fixture.client.closed_tabs.lock().unwrap(),
        ["w1:t5".to_owned()]
    );
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_tab().as_deref(), Some(tab.as_str()));
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    assert_eq!(fixture.client.viewer_commands.lock().unwrap().len(), 1);
}

#[test]
fn a_tail_viewer_is_replaced_by_a_console() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "tail");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    // The pane provably runs `agentctl tail lead --follow` in its foreground.
    fixture
        .client
        .tail_panes
        .lock()
        .unwrap()
        .insert("w1:p5".to_owned());
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5");
    assert_eq!(
        *fixture.client.closed_tabs.lock().unwrap(),
        ["w1:t5".to_owned()]
    );
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    let commands = fixture.client.viewer_commands.lock().unwrap();
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert!(commands[0].1.contains(" console lead"), "{commands:?}");
    assert!(
        !commands[0].1.contains("tail"),
        "never a tail viewer: {commands:?}"
    );
}

#[test]
fn a_live_console_is_untouched() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_eq!(tab, "w1:t5", "the live console keeps its tab");
    assert_eq!(creates(&fixture), 0, "no duplicate tab was created");
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
    assert!(fixture.client.viewer_commands.lock().unwrap().is_empty());
    // A second pass is equally quiet: no duplicates accumulate.
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_eq!(tab, "w1:t5");
    assert_eq!(creates(&fixture), 0);
}

#[test]
fn the_workspace_resolving_entry_point_creates_the_console() {
    // `ensure_headless_console_tab` is what start and the wrapper call: it finds the
    // workspace itself (here through the designated coordinator's record).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    let mut record = manager.load("lead").unwrap();
    record.workspace_id = Some("w1".to_owned());
    manager.save(&record).unwrap();
    fs::write(
        manager.registry.join(DESIGNATION_FILE),
        json!({"name": "lead"}).to_string(),
    )
    .unwrap();
    let tab = manager.ensure_headless_console_tab("lead").unwrap();
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_tab().as_deref(), Some(tab.as_str()));
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    assert_eq!(fixture.client.viewer_commands.lock().unwrap().len(), 1);
}

#[test]
fn a_recycled_tab_id_naming_another_agent_is_never_closed_or_claimed() {
    // After a reboot the recorded id can name a restored tab of someone else's:
    // its label is not this agent's name, so it is left exactly as found and a
    // fresh console tab is created instead (cold-review B1).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "tail");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    relabel(&fixture, "w1:t5", "coord");
    pane_busy(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5");
    assert!(
        fixture.client.closed_tabs.lock().unwrap().is_empty(),
        "the other agent's tab was closed"
    );
    assert_eq!(
        fixture.client.panes.lock().unwrap().len(),
        2,
        "the other tab's pane survives beside the new console"
    );
}

#[test]
fn a_tail_id_holding_a_live_program_is_never_closed_even_with_our_label() {
    // Hygiene label-sync renames a recorded viewer tab to the agent's name, so a
    // recycled tail id holding a live program can carry our label. Its foreground
    // is neither an idle shell nor `agentctl tail lead`: never closed (#1973).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "tail");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5", "a fresh console is created instead");
    assert!(
        fixture.client.closed_tabs.lock().unwrap().is_empty(),
        "the live program's tab was closed"
    );
    assert_eq!(fixture.client.panes.lock().unwrap().len(), 2);
}

#[test]
fn a_multi_pane_tab_is_never_closed() {
    // A split tab holding another live pane keeps both panes; only a fresh
    // console tab is created (cold-review B2).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "tail");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    push_pane(&fixture, "w1:p6", "w1:t5", "w1");
    pane_busy(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_ne!(tab, "w1:t5");
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
    assert_eq!(fixture.client.panes.lock().unwrap().len(), 3);
}

#[test]
fn a_just_created_console_within_its_grace_is_not_judged_blank() {
    // The start/wrapper race: a second ensure while the first console is still
    // starting (its pane still a shell) must not close and recreate it (#1973/7).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    let mut record = manager.load("lead").unwrap();
    record.set_viewer_created_at(Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64(),
    ));
    manager.save(&record).unwrap();
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_blank(&fixture);
    let tab = manager.ensure_headless_viewer("lead", "w1").unwrap();
    assert_eq!(tab, "w1:t5");
    assert_eq!(creates(&fixture), 0);
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
}

#[test]
fn without_close_permission_a_tail_viewer_is_left_and_a_console_created() {
    // Restore hygiene with close_bare_shells off: no tab is closed, ever (#1973/6).
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "tail");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    let tab = manager
        .ensure_headless_viewer_with_close("lead", "w1", false)
        .unwrap();
    assert_ne!(tab, "w1:t5");
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
}

fn plant_foreground(fixture: &Fixture, pane: &str, argv: Vec<String>) {
    fixture
        .client
        .foreground_argv
        .lock()
        .unwrap()
        .insert(pane.to_owned(), vec![argv]);
}

fn console_argv(fixture: &Fixture, agent: &str) -> Vec<String> {
    let registry = fixture.root.join("registry");
    vec![
        "agentctl".to_owned(),
        "--registry".to_owned(),
        registry.display().to_string(),
        "console".to_owned(),
        agent.to_owned(),
    ]
}

#[test]
fn restore_proves_an_idle_shell_viewer_dead_and_recreates_it_in_its_tab() {
    // #2214, the reboot shape from the issue: the recorded console tab
    // holds only an empty shell. A dry run reports it dead; a real
    // restore starts the console in that same tab, and the record still
    // names it. No tab is created or closed, and no harness is started.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_blank(&fixture);
    let (_, proof) = manager.headless_viewer_proof("lead").unwrap();
    assert!(
        matches!(proof, HeadlessViewerProof::Dead(ref reason) if reason.contains("idle shell")),
        "{proof:?}"
    );
    let row = manager.restore_headless_viewer("lead", true).unwrap();
    assert_eq!(row["state"], "dead", "{row}");
    assert_eq!(row["viewer_tab_id"], "w1:t5");
    assert!(fixture.client.viewer_commands.lock().unwrap().is_empty());
    let row = manager.restore_headless_viewer("lead", false).unwrap();
    assert_eq!(row["state"], "recreated", "{row}");
    assert_eq!(row["viewer_tab_id"], "w1:t5", "the recorded tab is reused");
    let commands = fixture.client.viewer_commands.lock().unwrap();
    assert_eq!(commands.len(), 1, "{commands:?}");
    assert_eq!(commands[0].0, "w1:p5");
    assert!(commands[0].1.contains(" console lead"), "{commands:?}");
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
    assert_eq!(creates(&fixture), 0, "no new tab was created");
    let record = manager.load("lead").unwrap();
    assert_eq!(record.viewer_tab().as_deref(), Some("w1:t5"));
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
}

#[test]
fn restore_leaves_a_proven_live_console_and_a_follow_tail_untouched() {
    // #2214: a pane provably running the agent's console (or its follow
    // tail) for this registry is live; restore touches nothing.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    plant_foreground(&fixture, "w1:p5", console_argv(&fixture, "lead"));
    let (_, proof) = manager.headless_viewer_proof("lead").unwrap();
    assert_eq!(proof, HeadlessViewerProof::Live);
    let row = manager.restore_headless_viewer("lead", false).unwrap();
    assert_eq!(row["state"], "live", "{row}");
    assert_eq!(creates(&fixture), 0);
    assert!(fixture.client.viewer_commands.lock().unwrap().is_empty());
    assert!(fixture.client.closed_tabs.lock().unwrap().is_empty());
    // The same pane running the follow tail is a live viewer too.
    plant_foreground(
        &fixture,
        "w1:p5",
        vec![
            "agentctl".to_owned(),
            "tail".to_owned(),
            "lead".to_owned(),
            "--follow".to_owned(),
        ],
    );
    let (_, proof) = manager.headless_viewer_proof("lead").unwrap();
    assert_eq!(proof, HeadlessViewerProof::Live);
    let row = manager.restore_headless_viewer("lead", false).unwrap();
    assert_eq!(row["state"], "live", "{row}");
    assert_eq!(creates(&fixture), 0);
}

#[test]
fn restore_recreates_a_missing_viewer_tab_and_records_the_new_one() {
    // #2214: the recorded tab is gone entirely; the ensure path creates
    // a console tab and the record names the new tab.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t9", "console");
    let row = manager.restore_headless_viewer("lead", true).unwrap();
    assert_eq!(row["state"], "dead", "{row}");
    let row = manager.restore_headless_viewer("lead", false).unwrap();
    assert_eq!(row["state"], "recreated", "{row}");
    let record = manager.load("lead").unwrap();
    let tab = record.viewer_tab().unwrap();
    assert_ne!(tab, "w1:t9");
    assert_eq!(row["viewer_tab_id"], tab);
    assert_eq!(record.viewer_kind().as_deref(), Some("console"));
    assert_eq!(fixture.client.viewer_commands.lock().unwrap().len(), 1);
}

#[test]
fn a_viewer_showing_another_agent_is_not_live_and_is_never_closed() {
    // #2214: the recorded tab's pane runs another agent's console. It
    // does not count as this agent's viewer; restore records a fresh
    // console tab and leaves the other agent's viewer exactly as found.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    plant_foreground(&fixture, "w1:p5", console_argv(&fixture, "other"));
    let (_, proof) = manager.headless_viewer_proof("lead").unwrap();
    assert!(matches!(proof, HeadlessViewerProof::Dead(_)), "{proof:?}");
    let row = manager.restore_headless_viewer("lead", false).unwrap();
    assert_eq!(row["state"], "recreated", "{row}");
    assert_ne!(row["viewer_tab_id"], "w1:t5");
    assert!(
        fixture.client.closed_tabs.lock().unwrap().is_empty(),
        "the other agent's viewer tab was closed"
    );
    assert_eq!(
        manager.load("lead").unwrap().viewer_tab().as_deref(),
        row["viewer_tab_id"].as_str()
    );
    // A console for the same name in ANOTHER registry is not live here.
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    push_pane(&fixture, "w1:p5", "w1:t5", "w1");
    pane_busy(&fixture);
    plant_foreground(
        &fixture,
        "w1:p5",
        vec![
            "agentctl".to_owned(),
            "--registry".to_owned(),
            "/some/other/registry".to_owned(),
            "console".to_owned(),
            "lead".to_owned(),
        ],
    );
    let (_, proof) = manager.headless_viewer_proof("lead").unwrap();
    assert!(matches!(proof, HeadlessViewerProof::Dead(_)), "{proof:?}");
}

#[test]
fn herdr_down_is_an_error_the_caller_can_retry_not_a_panic() {
    let fixture = Fixture::new();
    let manager = headless_agent(&fixture);
    record_viewer(&manager, "w1:t5", "console");
    fixture.client.fail_panes.store(true, Ordering::Relaxed);
    assert!(manager.ensure_headless_viewer("lead", "w1").is_err());
    // Nothing was created or recorded over the existing viewer.
    assert_eq!(creates(&fixture), 0);
    assert_eq!(
        manager.load("lead").unwrap().viewer_tab().as_deref(),
        Some("w1:t5")
    );
}
