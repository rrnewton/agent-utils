use super::*;

fn rotated(fixture: &Fixture) {
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("compacted-session".into())));
}

/// A Codex seat the pane reported no session for, anchored for follow (#2124).
fn unreported_codex(fixture: &Fixture) {
    fixture
        .client
        .report_session
        .store(false, Ordering::Relaxed);
    fixture.adopt();
    let manager = fixture.manager();
    manager.anchor("foreign", false, None).unwrap();
    let record = manager.read_record("foreign").unwrap();
    assert!(record.native_session.is_none(), "{record:?}");
    assert!(record.session_value.is_none(), "{record:?}");
    fixture.client.command_lines.lock().unwrap().insert(
        record.harness_identity.as_ref().unwrap().pid,
        vec!["/opt/bin/codex".into()],
    );
}

struct PretendHeldFiles;
impl Drop for PretendHeldFiles {
    fn drop(&mut self) {
        crate::subagents::revive::HELD_FILES.with(|cell| *cell.borrow_mut() = None);
    }
}
#[must_use]
fn pretend_held_files(files: Vec<PathBuf>) -> PretendHeldFiles {
    crate::subagents::revive::HELD_FILES.with(|cell| *cell.borrow_mut() = Some(files));
    PretendHeldFiles
}

#[test]
fn session_follow_captures_an_initial_codex_session_from_the_pane_report() {
    // #2124: a seat that named no session at start is observed after its
    // first turn from the pinned pane's own report, as a rotation is.
    let fixture = Fixture::new();
    unreported_codex(&fixture);
    let manager = fixture.manager();
    // After the first turn the pane reports its session (the fixture
    // reports none at all while `report_session` is off).
    fixture.client.report_session.store(true, Ordering::Relaxed);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("initial-thread".into())));
    assert!(manager.follow_observed_session("foreign").unwrap());
    let updated = manager.read_record("foreign").unwrap();
    let native = updated.native_session.clone().expect("initial capture");
    assert_eq!(native.value, "initial-thread");
    assert_eq!(native.source, "observed");
    assert_eq!(updated.session_value.as_deref(), Some("initial-thread"));
    assert_eq!(
        updated.launch_spec.as_ref().unwrap()["native_session"]["value"],
        "initial-thread"
    );
    assert!(!manager.follow_observed_session("foreign").unwrap());
}

#[test]
fn session_follow_captures_an_initial_codex_session_from_the_held_rollout() {
    // #2124: the pane reports nothing yet, so the rollout the pinned
    // process itself holds is the observer, as at start. No cwd
    // discovery is involved.
    let fixture = Fixture::new();
    unreported_codex(&fixture);
    let manager = fixture.manager();
    let id = "22222222-2222-4222-8222-222222222222";
    let dir = fixture.root.join("codex-home/sessions/2026/11/06");
    fs::create_dir_all(&dir).unwrap();
    let rollout = dir.join(format!("rollout-2026-11-06T00-00-00-{id}.jsonl"));
    fs::write(
        &rollout,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{id}\"}}}}\n"),
    )
    .unwrap();
    let _held = pretend_held_files(vec![rollout]);
    assert!(manager.follow_observed_session("foreign").unwrap());
    let updated = manager.read_record("foreign").unwrap();
    assert_eq!(
        updated.native_session.as_ref().unwrap().value,
        id,
        "{updated:?}"
    );
    assert_eq!(updated.native_session.as_ref().unwrap().source, "observed");
}

#[test]
fn session_follow_initial_capture_refuses_a_session_claimed_by_an_archived_peer() {
    // #2124: two Codex seats, one cwd: the session belongs to an
    // archived peer, so initial capture refuses and writes nothing.
    let fixture = Fixture::new();
    unreported_codex(&fixture);
    let manager = fixture.manager();
    let mut peer = manager.read_record("foreign").unwrap();
    peer.name = "peer-seat".to_owned();
    peer.token = "peer-token".to_owned();
    peer.lifecycle = "stopped".to_owned();
    peer.native_session = Some(NativeSession::new("codex", "claimed-thread", "observed"));
    peer.session_agent = Some("codex".to_owned());
    peer.session_value = Some("claimed-thread".to_owned());
    let archive = fixture.root.join("registry/archive");
    agent::create_private_directory(&archive, "test agent archive", false, false).unwrap();
    let dir = archive.join("peer-seat-peer-token");
    agent::create_private_directory(&dir, "test archived agent", false, false).unwrap();
    agent::atomic_json(&dir.join("agent.json"), &json!(peer)).unwrap();
    fixture.client.report_session.store(true, Ordering::Relaxed);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("claimed-thread".into())));
    let bytes = fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap();
    let error = manager
        .follow_observed_session("foreign")
        .unwrap_err()
        .to_string();
    assert!(error.contains("already claimed"), "{error}");
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap(),
        bytes
    );
}

#[test]
fn session_follow_initial_capture_writes_nothing_when_nothing_is_reported() {
    // A pane that reports no session and a process holding no rollout:
    // the record stays missing, byte for byte.
    let fixture = Fixture::new();
    unreported_codex(&fixture);
    let manager = fixture.manager();
    let _held = pretend_held_files(Vec::new());
    let bytes = fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap();
    assert!(!manager.follow_observed_session("foreign").unwrap());
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap(),
        bytes
    );
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
        let bytes = fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap();
        let _busy = (mutation == "busy").then(|| manager.lock("foreign").unwrap());
        rotated(&fixture);
        assert!(
            !matches!(manager.follow_observed_session("foreign"), Ok(true)),
            "{mutation}"
        );
        assert_eq!(
            fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap(),
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
    let old_record = fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap();
    fixture.client.command_lines.lock().unwrap().insert(
        record.harness_identity.as_ref().unwrap().pid,
        vec!["/opt/bin/codex\0invalid".into()],
    );
    rotated(&fixture);
    assert!(manager.follow_observed_session("foreign").is_err());
    assert_eq!(fs::read(queue.join("target.json")).unwrap(), old_binding);
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap(),
        old_record
    );
    assert!(fixture.client.runs.lock().unwrap().is_empty());
}

/// #783 chat-reply-capture-miss: the chat bridge binds a Claude Code conversation that a live
/// process names into a record that names none, never waiting for a busy record and never
/// replacing a conversation a record names.
#[test]
fn a_discovered_conversation_is_bound_only_into_a_free_record_that_names_none() {
    let fixture = Fixture::new();
    fixture
        .client
        .report_session
        .store(false, Ordering::Relaxed);
    fixture.adopt();
    let manager = fixture.manager();
    let mut record = manager.read_record("foreign").unwrap();
    record.harness = "claude".into();
    record.session_agent = None;
    record.session_value = None;
    record.native_session = None;
    manager.save(&record).unwrap();
    let bytes = fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap();
    {
        let _busy = manager.lock("foreign").unwrap();
        let error = manager
            .bind_discovered_session("foreign", "s-live")
            .unwrap_err();
        assert!(error.to_string().contains("lifecycle busy"), "{error}");
    }
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/foreign/agent.json")).unwrap(),
        bytes
    );
    assert!(manager
        .bind_discovered_session("foreign", "bad\0id")
        .is_err());
    assert!(manager
        .bind_discovered_session("foreign", "s-live")
        .unwrap());
    let native = manager
        .read_record("foreign")
        .unwrap()
        .native_session
        .unwrap();
    assert_eq!(native.value, "s-live");
    assert_eq!(
        native.source, "asserted",
        "recorded as bind-session records it"
    );
    assert!(!manager
        .bind_discovered_session("foreign", "s-other")
        .unwrap());
    assert_eq!(
        manager
            .read_record("foreign")
            .unwrap()
            .native_session
            .unwrap()
            .value,
        "s-live"
    );
    assert!(fixture.client.runs.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
}

/// A lock dropped while a copy of its descriptor lives on, as in a child that another thread
/// forked and that has not reached exec yet, is free at once: the guards unlock rather than
/// only close. Before they did, the bind right after a dropped lock found it "lifecycle busy"
/// whenever a test beside it was spawning a process.
#[test]
fn a_dropped_lock_is_free_while_a_forked_copy_of_its_descriptor_lives() {
    let fixture = Fixture::new();
    fixture.adopt();
    let manager = fixture.manager();
    let lock = manager.lock("foreign").unwrap();
    let forked: Vec<File> = lock
        ._files
        .0
        .iter()
        .map(|file| file.try_clone().unwrap())
        .collect();
    drop(lock);
    let locks = manager
        .try_record_locks("foreign", "session bind")
        .expect("the lifecycle lock is free once its guard is dropped");
    let forked_too: Vec<File> = locks
        .0
        .iter()
        .map(|file| file.try_clone().unwrap())
        .collect();
    drop(locks);
    drop(
        manager
            .try_record_locks("foreign", "session bind")
            .expect("the record locks are free once their guard is dropped"),
    );
    drop((forked, forked_too));
}
