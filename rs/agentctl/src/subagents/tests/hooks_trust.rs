//! #1932 agentctl-muse-hooks-trust-auto-decline: the watcher-side answer
//! reaches only this registry's managed Muse lanes.
use super::*;

const DIALOG: &str = "Review plugin hooks\n\
    39 new or changed hooks need your trust.\n\
    Hooks run commands on your machine when their event matches.\n\
    Approval follows the declaration, including after script updates.\n\
    \n\n\n\
    ⟩ 1. Review hooks\n\
    \x20 2. Trust all and continue\n\
    \x20 3. Continue without trusting\n\
    Up/Down to choose, Enter to confirm. Esc continues without trusting.\n";

fn start_muse(fixture: &Fixture) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "coord",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
    manager
}

fn show_dialog(fixture: &Fixture) {
    *fixture.client.screen.lock().unwrap() = Some(DIALOG.to_owned());
}

#[test]
fn a_managed_muse_lane_at_the_dialog_is_answered_per_policy() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    show_dialog(&fixture);
    let outcome = manager.answer_hooks_trust_if_dialog("coord").unwrap();
    let line = outcome.expect("the exact dialog is answered");
    assert!(
        line.contains("declined hook trust (39 hooks) per built-in default"),
        "{line}"
    );
    assert_eq!(fixture.client.hooks_answers.lock().unwrap().len(), 1);
    assert_eq!(fixture.client.hooks_answers.lock().unwrap()[0].1, "decline");
    // The dialog is gone afterwards.
    let screen = fixture.client.screen.lock().unwrap().clone().unwrap();
    assert!(!crate::client::muse_hooks_trust_dialog(&screen), "{screen}");
}

#[test]
fn a_near_miss_and_another_harness_are_never_answered() {
    let fixture = Fixture::new();
    let manager = start_muse(&fixture);
    *fixture.client.screen.lock().unwrap() =
        Some("Review plugin hooks\nContinue without trusting\n".to_owned());
    assert_eq!(manager.answer_hooks_trust_if_dialog("coord").unwrap(), None);
    assert!(fixture.client.hooks_answers.lock().unwrap().is_empty());

    let codex = Fixture::new();
    let manager = codex.manager();
    manager
        .start(
            "coord",
            &codex.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "codex".to_owned(),
                harness_args: vec!["--yolo".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    show_dialog(&codex);
    assert_eq!(manager.answer_hooks_trust_if_dialog("coord").unwrap(), None);
    assert!(codex.client.hooks_answers.lock().unwrap().is_empty());
}

fn write_trust_config(fixture: &Fixture) {
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    crate::agent::create_private_directory(&directory, "test profile config", false, false)
        .unwrap();
    crate::agent::atomic_json(
        &directory.join("profiles.json"),
        &serde_json::json!({
            "schema": "agentctl-profiles/v1",
            "muse_hooks_trust": "trust",
            "profiles": {},
        }),
    )
    .unwrap();
}

#[test]
fn a_trust_launch_pretrusts_hooks_and_a_decline_launch_never_does() {
    // #1972: under an explicit trust policy the launch pre-trusts hooks
    // through Muse's approve verb before the pane spawns.
    let fixture = Fixture::new();
    write_trust_config(&fixture);
    let _manager = start_muse(&fixture);
    assert_eq!(fixture.client.pretrust_calls.lock().unwrap().len(), 1);

    // The built-in decline default never attempts a pre-trust.
    let decline = Fixture::new();
    let _manager = start_muse(&decline);
    assert!(decline.client.pretrust_calls.lock().unwrap().is_empty());
}

#[test]
fn a_failed_pretrust_never_blocks_the_launch() {
    let fixture = Fixture::new();
    write_trust_config(&fixture);
    fixture
        .client
        .pretrust_fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let _manager = start_muse(&fixture);
    assert_eq!(fixture.client.pretrust_calls.lock().unwrap().len(), 1);
}

#[test]
fn pretrust_candidates_are_only_untrusted_hooks() {
    let document = serde_json::json!({"runtime_capabilities": [
        {"status": "review_needed", "candidate": {
            "kind": "hook", "plugin_id": "ours",
            "stable_id": "plugin:ours:hook:new", "definition_hash": "sha256:new"}},
        {"status": "modified", "candidate": {
            "kind": "hook", "plugin_id": "ours",
            "stable_id": "plugin:ours:hook:changed", "definition_hash": "sha256:changed"}},
        {"status": "trusted_enabled", "candidate": {
            "kind": "hook", "plugin_id": "ours",
            "stable_id": "plugin:ours:hook:fine", "definition_hash": "sha256:fine"}},
        {"status": "trusted_disabled", "candidate": {
            "kind": "hook", "plugin_id": "ours",
            "stable_id": "plugin:ours:hook:rejected", "definition_hash": "sha256:rejected"}},
        {"status": "review_needed", "candidate": {
            "kind": "invocation_preapproval", "plugin_id": "ours",
            "stable_id": "plugin:ours:invocation_preapproval:skill:x",
            "definition_hash": "sha256:skill"}},
    ]});
    let candidates = crate::client::muse_pretrust_candidates(&document, "ours");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.stable_id.as_str())
            .collect::<Vec<_>>(),
        ["plugin:ours:hook:new", "plugin:ours:hook:changed"]
    );
}

#[test]
fn an_adopted_foreign_muse_at_the_dialog_is_never_answered() {
    let fixture = Fixture::new();
    fixture.prepare_foreign();
    fixture
        .client
        .custom_reported
        .store(true, Ordering::Relaxed);
    let mut options = fixture.adopt_options();
    options.harness = "muse".to_owned();
    let manager = fixture.manager();
    manager.adopt("foreign", options).unwrap();
    assert_eq!(manager.load("foreign").unwrap().adapter, "herdr-foreign");
    show_dialog(&fixture);
    assert_eq!(
        manager.answer_hooks_trust_if_dialog("foreign").unwrap(),
        None
    );
    assert!(fixture.client.hooks_answers.lock().unwrap().is_empty());
    // The screen is untouched.
    assert_eq!(
        fixture.client.screen.lock().unwrap().as_deref(),
        Some(DIALOG)
    );
}
