use super::*;

fn rotated(fixture: &Fixture) {
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("compacted-session".into())));
}

#[test]
fn session_follow_rebinds_only_same_pinned_runtime_and_recovers_binding_first_interruption() {
    let fixture = Fixture::new();
    fixture.adopt();
    let manager = fixture.manager();
    manager.anchor("foreign", false, None).unwrap();
    let before = manager.read_record("foreign").unwrap();
    fixture.client.command_lines.lock().unwrap().insert(
        before.harness_identity.as_ref().unwrap().pid,
        vec!["/opt/bin/codex".into()],
    );
    let queue = manager.queue("foreign").unwrap();
    agent::enqueue(&queue, "retained input", Some("pending")).unwrap();
    let pending = fs::read(queue.join("inbox/pending.json")).unwrap();
    rotated(&fixture);
    // Simulate interruption after queue binding, before the registration write.
    let mut after = before.clone();
    after.session_value = Some("compacted-session".into());
    agent::rebind_queue(&queue, &before.target().unwrap(), &after.target().unwrap()).unwrap();
    assert!(manager.follow_observed_session("foreign").unwrap());
    let updated = manager.read_record("foreign").unwrap();
    assert_eq!(updated.token, before.token);
    assert_eq!(updated.harness_identity, before.harness_identity);
    assert_eq!(updated.native_session.unwrap().value, "compacted-session");
    let spec = updated
        .launch_spec
        .as_ref()
        .expect("real command line captured");
    assert_eq!(spec["source"], "anchor");
    assert_eq!(spec["native_session"]["value"], "compacted-session");
    assert_eq!(agent::queued_prompts(&queue).unwrap(), ["pending"]);
    assert_eq!(fs::read(queue.join("inbox/pending.json")).unwrap(), pending);
    assert!(!manager.follow_observed_session("foreign").unwrap());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
}

#[test]
fn session_follow_refuses_changed_process_terminal_directory_assertions_and_missing_pins() {
    for mutation in [
        "process",
        "terminal",
        "cwd",
        "asserted",
        "unpinned",
        "busy",
        "duplicate",
    ] {
        let fixture = Fixture::new();
        fixture.adopt();
        let manager = fixture.manager();
        manager.anchor("foreign", false, None).unwrap();
        let mut record = manager.read_record("foreign").unwrap();
        match mutation {
            "process" => {
                fixture
                    .client
                    .harness_pids
                    .lock()
                    .unwrap()
                    .insert("owned".into(), 999);
            }
            "terminal" => record.terminal_id = Some("another-terminal".into()),
            "cwd" => {
                fixture
                    .client
                    .wrong_foreign_cwd
                    .store(true, Ordering::Relaxed);
            }
            "asserted" => record.goal_session_id = Some("operator-pinned".into()),
            "unpinned" => record.harness_identity = None,
            "duplicate" => {
                fixture
                    .client
                    .panes
                    .lock()
                    .unwrap()
                    .push(Fake::pane("sibling"));
            }
            _ => {}
        }
        manager.save(&record).unwrap();
        let bytes = fs::read(fixture.root.join("registry/foreign/agent.json")).unwrap();
        let _busy = (mutation == "busy").then(|| manager.lock("foreign").unwrap());
        rotated(&fixture);
        assert!(
            !matches!(manager.follow_observed_session("foreign"), Ok(true)),
            "{mutation}"
        );
        assert_eq!(
            fs::read(fixture.root.join("registry/foreign/agent.json")).unwrap(),
            bytes,
            "{mutation}"
        );
        assert!(fixture.client.runs.lock().unwrap().is_empty());
    }
}

#[test]
fn session_follow_validates_launch_metadata_before_changing_queue_binding() {
    let fixture = Fixture::new();
    fixture.adopt();
    let manager = fixture.manager();
    manager.anchor("foreign", false, None).unwrap();
    let record = manager.read_record("foreign").unwrap();
    let queue = manager.queue("foreign").unwrap();
    let target = record.target().unwrap();
    agent::rebind_queue(&queue, &target, &target).unwrap();
    let old_binding = fs::read(queue.join("target.json")).unwrap();
    let old_record = fs::read(fixture.root.join("registry/foreign/agent.json")).unwrap();
    fixture.client.command_lines.lock().unwrap().insert(
        record.harness_identity.as_ref().unwrap().pid,
        vec!["/opt/bin/codex\0invalid".into()],
    );
    rotated(&fixture);
    assert!(manager.follow_observed_session("foreign").is_err());
    assert_eq!(fs::read(queue.join("target.json")).unwrap(), old_binding);
    assert_eq!(
        fs::read(fixture.root.join("registry/foreign/agent.json")).unwrap(),
        old_record
    );
    assert!(fixture.client.runs.lock().unwrap().is_empty());
}
