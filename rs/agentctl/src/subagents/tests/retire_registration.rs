use super::*;

fn pins(fixture: &Fixture) -> RetireStaleOptions {
    let bytes = fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap();
    let record = fixture.manager().read_record("worker").unwrap();
    RetireStaleOptions {
        expected_token: record.token,
        expected_record_sha256: format!("{:x}", Sha256::digest(bytes)),
        expected_revive_sha256: None,
        replacement_pane: None,
        replacement_session: None,
        dry_run: false,
    }
}

#[test]
fn retire_stale_archives_exact_record_and_queue_without_touching_runtime() {
    let fixture = Fixture::new();
    fixture.start(None);
    let options = pins(&fixture);
    let bytes = fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap();
    let queue = fixture.root.join("registry/agents/worker/queue");
    agent::enqueue(&queue, "preserve, never replay", Some("retained-prompt")).unwrap();
    fixture.client.panes.lock().unwrap().clear();
    let report = fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    let archive = PathBuf::from(report["archive"].as_str().unwrap());
    assert_eq!(fs::read(archive.join("agent.json")).unwrap(), bytes);
    assert_eq!(
        agent::queued_prompts(&archive.join("queue")).unwrap(),
        ["retained-prompt"]
    );
    assert!(!fixture.root.join("registry/agents/worker").exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    assert_eq!(report["runtime_preserved"], true);
    assert_eq!(
        fixture
            .manager()
            .retire_stale_registration("worker", &options)
            .unwrap()["already_archived"],
        true
    );
}

#[test]
fn retire_stale_refuses_live_rows_wrong_pins_and_unreachable_terminal_server() {
    let fixture = Fixture::new();
    fixture.start(None);
    let mut options = pins(&fixture);
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    fixture.client.panes.lock().unwrap().clear();
    options.expected_token.push('x');
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    options = pins(&fixture);
    options.expected_record_sha256 = "0".repeat(64);
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    options = pins(&fixture);
    fixture.client.fail_panes.store(true, Ordering::Relaxed);
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    assert!(fixture
        .root
        .join("registry/agents/worker/agent.json")
        .exists());
    assert!(!fixture.root.join("registry/archive").exists());
}

#[test]
fn retire_stale_dry_run_preserves_replaced_pane_and_registration() {
    let fixture = Fixture::new();
    fixture.start(None);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("new-native-session".into())));
    let mut options = pins(&fixture);
    options.dry_run = true;
    let report = fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    assert_eq!(report["dry_run"], true);
    assert!(fixture
        .root
        .join("registry/agents/worker/agent.json")
        .exists());
    assert!(!fixture.root.join("registry/archive").exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
}

fn pending_revive() -> (Fixture, AgentRecord, RetireStaleOptions, Vec<u8>) {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture.start(None);
    let old = fixture.manager().load("worker").unwrap();
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(old.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    super::super::revive::fail_once("launching");
    assert!(fixture
        .manager()
        .revive("worker", false, Some(&old.token), Duration::from_secs(1))
        .is_err());
    let options = pins(&fixture);
    let journal = fixture
        .root
        .join("registry/.revives")
        .join(format!("{}.json", old.token));
    let bytes = fs::read(&journal).unwrap();
    (fixture, old, options, bytes)
}

#[test]
fn retire_stale_requires_exact_pending_revive_and_preserves_every_artifact() {
    let (fixture, old, mut options, bytes) = pending_revive();
    let journal = fixture
        .root
        .join("registry/.revives")
        .join(format!("{}.json", old.token));
    fixture.client.panes.lock().unwrap().clear();
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    assert!(fixture.root.join("registry/agents/worker").exists());
    options.expected_revive_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    let report = fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    let archive = PathBuf::from(report["archive"].as_str().unwrap());
    assert_eq!(
        fs::read(archive.join(format!("{}.json", old.token))).unwrap(),
        bytes
    );
    assert!(archive.join("retired-revive/new/agent.json").exists());
    assert!(!journal.exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
}

#[test]
fn retire_stale_pins_live_candidate_and_resumes_after_journal_archival() {
    let (fixture, old, mut options, bytes) = pending_revive();
    let operation = fixture.root.join("registry/.revives").join(&old.token);
    let candidate_path = operation.join("new/agent.json");
    let mut candidate = agent::read_private_json(&candidate_path).unwrap();
    candidate["pane_id"] = json!("replacement");
    agent::atomic_json(&candidate_path, &candidate).unwrap();
    *fixture.client.panes.lock().unwrap() = vec![Fake::pane("replacement")];
    fixture.client.started.store(true, Ordering::Relaxed);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("codex".into()), Some("verified-live-session".into())));
    options.expected_revive_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    options.replacement_pane = Some("replacement".into());
    options.replacement_session = Some("wrong-session".into());
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    assert!(fixture
        .root
        .join("registry/agents/worker/agent.json")
        .exists());
    options.replacement_session = Some("verified-live-session".into());
    let report = fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    let archive = PathBuf::from(report["archive"].as_str().unwrap());
    assert_eq!(fixture.client.panes.lock().unwrap().len(), 1);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    // Model a process interruption after journal publication, before moving its operation.
    fs::rename(archive.join("retired-revive"), &operation).unwrap();
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    assert!(archive.join("retired-revive/new/agent.json").exists());
    assert!(!operation.exists());
}

#[test]
fn retire_stale_pins_live_muse_candidate_by_its_resumed_argv() {
    // #947: Herdr reports no session for a resumed Muse; the replacement
    // pane is found through its foreground `muse ... resume S` argv.
    let (fixture, old, mut options, bytes) = pending_revive();
    let operation = fixture.root.join("registry/.revives").join(&old.token);
    let candidate_path = operation.join("new/agent.json");
    let mut candidate = agent::read_private_json(&candidate_path).unwrap();
    candidate["pane_id"] = json!("replacement");
    candidate["harness"] = json!("muse");
    candidate["launch_spec"] = json!(null);
    candidate["native_session"] = json!(null);
    candidate["session_agent"] = json!(null);
    candidate["session_value"] = json!(null);
    candidate["resume"] = json!(null);
    agent::atomic_json(&candidate_path, &candidate).unwrap();
    *fixture.client.panes.lock().unwrap() = vec![Fake::pane("replacement")];
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture
        .client
        .launches
        .lock()
        .unwrap()
        .insert("replacement".to_owned(), ("muse".to_owned(), Vec::new()));
    *fixture.client.session_override.lock().unwrap() = Some((None, None));
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert("replacement".to_owned(), 777);
    fixture.client.command_lines.lock().unwrap().insert(
        777,
        vec![
            "/usr/local/bin/muse".to_owned(),
            "--model".to_owned(),
            "m".to_owned(),
            "resume".to_owned(),
            "verified-live-session".to_owned(),
        ],
    );
    options.expected_revive_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    options.replacement_pane = Some("replacement".into());
    options.replacement_session = Some("wrong-session".into());
    assert!(fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .is_err());
    options.replacement_session = Some("verified-live-session".into());
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
}

#[test]
fn retire_stale_needs_no_pin_for_a_candidate_pane_holding_only_a_shell() {
    // B8: an orphan candidate pane (bare bash, no harness) cannot satisfy a
    // session pin; retire must proceed without replacement pins.
    let (fixture, old, mut options, bytes) = pending_revive();
    let operation = fixture.root.join("registry/.revives").join(&old.token);
    let candidate_path = operation.join("new/agent.json");
    let mut candidate = agent::read_private_json(&candidate_path).unwrap();
    candidate["pane_id"] = json!("replacement");
    agent::atomic_json(&candidate_path, &candidate).unwrap();
    *fixture.client.panes.lock().unwrap() = vec![Fake::pane("replacement")];
    *fixture.client.session_override.lock().unwrap() = Some((None, None));
    fixture.client.custom_alive.store(false, Ordering::Relaxed);
    fixture
        .client
        .custom_at_idle_shell
        .store(true, Ordering::Relaxed);
    options.expected_revive_sha256 = Some(format!("{:x}", Sha256::digest(&bytes)));
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
}

#[test]
fn retiring_an_old_generation_never_archives_a_new_registration() {
    let fixture = Fixture::new();
    fixture.start(None);
    let options = pins(&fixture);
    fixture.client.panes.lock().unwrap().clear();
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    fixture.start(None);
    let path = fixture.root.join("registry/agents/worker/agent.json");
    let before = fs::read(&path).unwrap();
    fixture
        .manager()
        .retire_stale_registration("worker", &options)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
}
