//! #2421 agentctl-asks-never-block: the manager refuses a permission
//! prompt only in a pane it may type into and can see at its bottom; any
//! other case is an error ("could not act"), never "nothing to refuse",
//! so the watcher counts it towards giving up and pages as before.
use super::*;

const PERMISSION: &str = "────────────────────────────────────────\n Bash command\n rm -rf /work/project/victim\n Permission rule Bash requires confirmation for this command.\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n Esc to cancel · Tab to amend\n";

const MUSE_APPROVAL: &str = include_str!("../../../testdata/muse-busy/muse-approval.txt");

fn start_claude(fixture: &Fixture) -> ManagedAgents<'_, Fake> {
    let manager = fixture.manager();
    manager
        .start(
            "lane",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    *fixture.client.status.lock().unwrap() = Some("idle".to_owned());
    manager
}

#[test]
fn a_pane_whose_scroll_position_is_unknown_is_not_refused_and_says_so() {
    // The fake client cannot prove the pane is at its bottom (the trait's
    // default answer), so no key may be sent: the refusal fails loudly.
    let fixture = Fixture::new();
    let manager = start_claude(&fixture);
    *fixture.client.screen.lock().unwrap() = Some(PERMISSION.to_owned());
    fixture.client.keys_sent.lock().unwrap().clear();
    let error = manager.deny_permission_prompt("lane", None).unwrap_err();
    assert!(error.to_string().contains("scrolled back"), "{error}");
    assert!(
        fixture.client.keys_sent.lock().unwrap().is_empty(),
        "{:?}",
        fixture.client.keys_sent.lock().unwrap()
    );
    // Its policy is still to refuse: only the attempt failed.
    assert!(manager.permission_policy_for("lane").unwrap().deny);
}

#[test]
fn an_adopted_foreign_pane_is_never_refused_and_its_policy_reports() {
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
    *fixture.client.screen.lock().unwrap() = Some(MUSE_APPROVAL.to_owned());
    fixture.client.keys_sent.lock().unwrap().clear();
    let error = manager.deny_permission_prompt("foreign", None).unwrap_err();
    assert!(
        error.to_string().contains("adopted foreign pane"),
        "{error}"
    );
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    // The watcher reads this as report-only, so the prompt is paged at
    // once instead of waiting on a refusal that cannot come.
    let policy = manager.permission_policy_for("foreign").unwrap();
    assert!(!policy.deny, "{policy:?}");
    assert!(
        policy.source.starts_with("adopted foreign pane"),
        "{}",
        policy.source
    );
}

#[test]
fn a_missing_record_is_nothing_to_refuse() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    assert_eq!(
        manager.deny_permission_prompt("nobody", None).unwrap(),
        None
    );
}
