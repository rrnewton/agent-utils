use super::*;

const TIMEOUT: Duration = Duration::from_secs(30);

fn kill_owned(fixture: &Fixture, record: &AgentRecord) {
    fixture
        .client
        .dead_panes
        .lock()
        .unwrap()
        .insert(record.pane_id.clone().unwrap());
    fixture.client.started.store(false, Ordering::Relaxed);
    fixture.client.custom_alive.store(false, Ordering::Relaxed);
}

fn recoverable(harness: &str, reports_session: bool) -> (Fixture, AgentRecord) {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .client
        .report_session
        .store(reports_session, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: harness.to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load("worker").unwrap();
    kill_owned(&fixture, &record);
    (fixture, record)
}

#[test]
fn restart_refuses_before_exit_when_the_recorded_cwd_is_gone() {
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    old.cwd = "/nonexistent-restart-cwd".to_owned();
    fixture.manager().save(&old).unwrap();
    let pane = old.pane_id.clone().unwrap();
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    fixture.client.exit_to_shell.store(true, Ordering::Relaxed);
    let error = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap_err();
    assert!(error.to_string().contains("starting directory"), "{error}");
    // Nothing was exited: no /exit was typed into the pane.
    assert!(
        !fixture
            .client
            .runs
            .lock()
            .unwrap()
            .iter()
            .any(|run| run.contains("/exit")),
        "a refused restart must not touch the harness"
    );
}

#[test]
fn restart_resumes_from_an_exited_journal_without_exiting_again() {
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    fixture.manager().save(&old).unwrap();
    // A previous restart wrote its journal and exited the harness, then
    // was interrupted: the rerun must skip the exit and revive.
    let journal = fixture
        .root
        .join("registry/.restarts")
        .join(format!("{}.json", old.token));
    fs::create_dir_all(journal.parent().unwrap()).unwrap();
    fs::write(
        &journal,
        json!({
            "schema": "agentctl-restart/v1", "name": "worker",
            "old_token": old.token, "session": "sess-1",
            "phase": "exited", "started_at": 0.0,
        })
        .to_string(),
    )
    .unwrap();
    let result = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap();
    assert_eq!(result["action"], "restarted", "{result}");
    assert_eq!(result["restart_journal"], "resumed-after-exit");
    assert!(
        !journal.exists(),
        "a completed restart consumes its journal"
    );
}

#[test]
fn a_restart_waits_out_a_harness_that_outlives_its_exit_and_drops_stale_journals() {
    // #1023: Muse prints its resume hint and the pane shows a shell while its runtime lives on;
    // the restart ends that leftover instead of letting revive refuse a live harness.
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    fixture.manager().save(&old).unwrap();
    let pane = old.pane_id.clone().unwrap();
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    fixture.client.exit_to_shell.store(true, Ordering::Relaxed);
    fixture
        .client
        .linger_after_exit
        .store(true, Ordering::Relaxed);
    // A journal an earlier generation's restart left at "exited".
    let restarts = fixture.root.join("registry/.restarts");
    agent::create_private_directory(&restarts, "test restart journals", false, false).unwrap();
    let stale = restarts.join("1794000000000000000-1.json");
    fs::write(
        &stale,
        json!({
            "schema": "agentctl-restart/v1", "name": "worker",
            "old_token": "1794000000000000000-1", "session": "sess-0",
            "phase": "exited", "started_at": 0.0,
        })
        .to_string(),
    )
    .unwrap();
    let result = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap();
    assert_eq!(result["action"], "restarted", "{result}");
    let pinned = old.custom_process_identity.as_ref().unwrap().pid;
    assert!(fixture.client.ended.lock().unwrap().contains(&pinned));
    assert!(!stale.exists(), "a stale restart journal is dropped");
}

#[test]
fn a_restart_whose_relaunch_fails_says_the_harness_exited_and_how_to_finish() {
    // #1023: the harness exits, then the relaunch fails; the error says so and names the
    // command that finishes it, and the restart journal is left at "exited".
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    fixture.manager().save(&old).unwrap();
    let pane = old.pane_id.clone().unwrap();
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    fixture.client.exit_to_shell.store(true, Ordering::Relaxed);
    fixture
        .client
        .fail_ready_once
        .store(true, Ordering::Relaxed);
    let error = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("exited its harness but could not relaunch it"),
        "{error}"
    );
    assert!(
        error.contains("`agentctl revive worker` finishes the restart"),
        "{error}"
    );
    let journal = fixture
        .root
        .join("registry/.restarts")
        .join(format!("{}.json", old.token));
    let phase = serde_json::from_slice::<Value>(&fs::read(&journal).unwrap()).unwrap();
    assert_eq!(phase["phase"], "exited");
}

#[test]
fn restart_drains_prompts_queued_during_the_restart_window() {
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    fixture.manager().save(&old).unwrap();
    // A prompt that reached the old generation while it was down: revive
    // archives it, and restart must replay it into the new session.
    // The restarted Muse sits at an idle composer, ready to be drained into.
    *fixture.client.muse_editor.lock().unwrap() = Some(
        super::herdr_compatibility::MuseEditor::new("Launch overrides", false, None),
    );
    let queue = fixture.root.join("registry/agents/worker/queue/inbox");
    agent::create_private_directory(&queue, "test queue lane", true, false).unwrap();
    agent::atomic_json(
        &queue.join("m-1.json"),
        &json!({"id": "m-1", "text": "[from lane-a via agentctl · reply: agentctl send lane-a]\nplease run the gate"}),
    )
    .unwrap();
    let result = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap();
    assert_eq!(result["action"], "restarted", "{result}");
    assert_eq!(
        result["replayed_prompts"],
        json!([{"old_id": "m-1", "new_id": "restart-m-1", "lane": "inbox"}]),
        "{result}"
    );
    assert!(
        result["drain"]["delivered"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == "restart-m-1")),
        "{result}"
    );
    assert!(
        fixture
            .client
            .literal_pastes
            .lock()
            .unwrap()
            .iter()
            .any(|(_, text)| text.contains("please run the gate")),
        "the replayed prompt was typed into the restarted session"
    );
}

/// Simulate a hand restart in the recorded pane: a new harness process
/// whose argv resumes `session`, while the record still pins the old pid.
fn hand_restarted(fixture: &Fixture, old: &AgentRecord, session: &str) {
    let pane = old.pane_id.clone().unwrap();
    let new_pid = 987_654_u64;
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    fixture
        .client
        .harness_pids
        .lock()
        .unwrap()
        .insert(pane.clone(), new_pid);
    fixture.client.custom_processes.lock().unwrap().insert(
        pane,
        CustomProcessIdentity {
            version: 1,
            boot_id: "22222222-3333-4444-5555-666666666666".to_owned(),
            pid: new_pid,
            starttime_ticks: new_pid,
            executable_device: 5,
            executable_inode: 6,
        },
    );
    fixture.client.command_lines.lock().unwrap().insert(
        new_pid,
        vec![
            "/usr/local/bin/muse".to_owned(),
            "--model".to_owned(),
            "m".to_owned(),
            "resume".to_owned(),
            session.to_owned(),
        ],
    );
}

#[test]
fn auto_repin_repins_a_hand_restarted_harness_and_refuses_mismatches() {
    // Positive: same pane, same terminal, argv resumes the recorded session.
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    old.arguments = vec!["--model".to_owned(), "m".to_owned()];
    fixture.manager().save(&old).unwrap();
    let old_pid = old.custom_process_identity.clone().unwrap().pid;
    hand_restarted(&fixture, &old, "sess-1");
    assert!(fixture.manager().auto_repin_resumed("worker"));
    let repinned = fixture.manager().load("worker").unwrap();
    assert_ne!(
        repinned
            .custom_process_identity
            .as_ref()
            .map(|identity| identity.pid),
        Some(old_pid),
        "the record now pins the new harness"
    );

    // Refusal: the running argv resumes a different session.
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    old.arguments = vec!["--model".to_owned(), "m".to_owned()];
    fixture.manager().save(&old).unwrap();
    let before = fixture.manager().load("worker").unwrap();
    hand_restarted(&fixture, &old, "someone-elses-session");
    assert!(!fixture.manager().auto_repin_resumed("worker"));
    assert_eq!(fixture.manager().load("worker").unwrap(), before);

    // Refusal: the pane's terminal is not the recorded terminal.
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    old.terminal_id = Some("term-not-the-panes".to_owned());
    old.arguments = vec!["--model".to_owned(), "m".to_owned()];
    fixture.manager().save(&old).unwrap();
    let before = fixture.manager().load("worker").unwrap();
    hand_restarted(&fixture, &old, "sess-1");
    assert!(!fixture.manager().auto_repin_resumed("worker"));
    assert_eq!(fixture.manager().load("worker").unwrap(), before);

    // Refusal: a foreign program holds the pane (no resume argv at all).
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    old.arguments = vec!["--model".to_owned(), "m".to_owned()];
    fixture.manager().save(&old).unwrap();
    let before = fixture.manager().load("worker").unwrap();
    hand_restarted(&fixture, &old, "sess-1");
    fixture
        .client
        .command_lines
        .lock()
        .unwrap()
        .insert(987_654, vec!["bash".to_owned()]);
    assert!(!fixture.manager().auto_repin_resumed("worker"));
    assert_eq!(fixture.manager().load("worker").unwrap(), before);
}

fn journal_path(fixture: &Fixture, record: &AgentRecord) -> PathBuf {
    fixture
        .root
        .join("registry/.revives")
        .join(format!("{}.json", record.token))
}

fn stage_path(fixture: &Fixture, record: &AgentRecord) -> PathBuf {
    fixture
        .root
        .join("registry/.revives")
        .join(&record.token)
        .join("new")
}

type FileSnapshot = BTreeMap<PathBuf, (u64, u64, u32, Option<Vec<u8>>)>;

fn files(path: &Path) -> FileSnapshot {
    fn visit(root: &Path, path: &Path, result: &mut FileSnapshot) {
        // The lock-waiter visibility directory is not registry content:
        // `lock_wait::register` creates `.waiting/<agent>` whenever a
        // lifecycle lock is contended, and only the record inside it is
        // removed afterwards. Whether a given call contended depends on
        // scheduling, so counting the directory makes this snapshot a
        // timing test; its records are asserted through the lock tests.
        if path.file_name().is_some_and(|name| name == ".waiting") && path != root {
            return;
        }
        let metadata = fs::symlink_metadata(path).unwrap();
        result.insert(
            path.strip_prefix(root).unwrap().to_owned(),
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mode(),
                metadata.is_file().then(|| fs::read(path).unwrap()),
            ),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}

#[test]
fn restart_exits_and_revives_in_the_same_pane() {
    let (fixture, mut old) = recoverable("muse", false);
    old.session_agent = Some("muse".to_owned());
    old.session_value = Some("sess-1".to_owned());
    fixture.manager().save(&old).unwrap();
    let pane = old.pane_id.clone().unwrap();
    // Undo the kill: the harness is live and idle in its recorded pane.
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    // The harness exits on /exit, returning the pane to its shell.
    fixture.client.exit_to_shell.store(true, Ordering::Relaxed);
    let result = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap();
    assert_eq!(result["action"], "restarted");
    assert_eq!(result["pane_id"], pane);
    assert_eq!(result["previous_pane_id"], pane);
    let new = fixture.manager().load("worker").unwrap();
    assert_ne!(new.token, old.token);
    assert_eq!(new.pane_id.as_deref(), Some(pane.as_str()));
}

#[test]
fn restart_resumes_the_launch_spec_session_when_the_top_level_field_is_empty() {
    // #948 on the real restart path (#986): a lane whose session lives
    // only in its launch spec (anchored before the top-level write)
    // restarts into that session, not into a refusal.
    let (fixture, mut old) = recoverable("muse", false);
    let mut spec = old.launch_spec.clone().expect("start recorded a spec");
    spec["native_session"] = json!({
        "agent": "muse", "value": "sess-9", "source": "caller"
    });
    old.launch_spec = Some(spec);
    old.native_session = None;
    old.session_value = None;
    old.resume = None;
    fixture.manager().save(&old).unwrap();
    let pane = old.pane_id.clone().unwrap();
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    fixture.client.exit_to_shell.store(true, Ordering::Relaxed);
    let result = fixture
        .manager()
        .restart("worker", false, Duration::from_secs(30), TIMEOUT)
        .unwrap();
    assert_eq!(result["action"], "restarted", "{result}");
    let new = fixture.manager().load("worker").unwrap();
    assert!(
        new.arguments
            .windows(2)
            .any(|pair| pair == ["resume", "sess-9"]),
        "{:?}",
        new.arguments
    );
}

#[test]
fn restart_refuses_an_unknown_session_without_touching_the_pane() {
    let (fixture, mut old) = recoverable("muse", false);
    old.native_session = None;
    old.session_value = None;
    old.resume = None;
    fixture.manager().save(&old).unwrap();
    let before = files(&fixture.root.join("registry"));
    assert!(fixture
        .manager()
        .restart("worker", false, Duration::from_secs(5), TIMEOUT)
        .is_err());
    assert_eq!(files(&fixture.root.join("registry")), before);
}

#[test]
fn revive_resumes_without_reports_using_independent_process_and_terminal_anchors() {
    let (fixture, old) = recoverable("claude", false);
    let native = old.native_session.clone().unwrap();
    let result = fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    assert_eq!(result["previous_token"], old.token);
    assert_eq!(result["resume"], native.value);
    assert_eq!(result["session_value"], Value::Null);
    assert_ne!(result["token"], old.token);
    assert_eq!(result["pane_closed"], true);
    assert_eq!(result["tab_closed"], true);
    let new = fixture.manager().load("worker").unwrap();
    assert!(new.harness_anchor().is_some());
    assert!(new.terminal_id.is_some());
    assert_eq!(new.arguments, ["--resume", &native.value]);
    assert_eq!(new.extra["revived_from"], old.token);
    assert!(
        !new.extra.contains_key("former_tokens"),
        "a registry with no edges or designation gets no new field"
    );
    assert!(new.storage_directory.is_none());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    assert_eq!(
        *fixture.client.closed.lock().unwrap(),
        [old.pane_id.clone().unwrap()]
    );
    assert!(!journal_path(&fixture, &old).exists());
}

#[test]
fn a_standing_agent_stays_standing_through_revive_until_set_otherwise() {
    // #901 watch-standing-agents: the flag is in the record, so the revived generation keeps
    // it; list and status show it, and `--no-standing` removes it.
    let (fixture, old) = recoverable("claude", false);
    let set = fixture.manager().set_standing("worker", true).unwrap();
    assert_eq!(
        set,
        json!({"name": "worker", "standing": true, "previous": false})
    );
    fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap();
    let new = fixture.manager().load("worker").unwrap();
    assert_ne!(new.token, old.token);
    assert!(new.standing());
    assert_eq!(
        fixture.manager().status("worker").unwrap()["standing"],
        true
    );
    let cleared = fixture.manager().set_standing("worker", false).unwrap();
    assert_eq!(cleared["previous"], true);
    assert!(!fixture.manager().load("worker").unwrap().standing());
    assert!(fixture.manager().status("worker").unwrap()["standing"].is_null());
}

#[test]
fn revive_keeps_the_control_edges_and_answers_to_its_former_token() {
    let (fixture, old) = recoverable("claude", false);
    let path = fixture.root.join("registry/agents/worker/agent.json");
    let mut document = agent::read_private_json(&path).unwrap();
    let edge = json!({"agent": "lieutenant", "registry": "/r", "token": "t", "proof": "assigned"});
    document["managed_by"] = edge.clone();
    document["created_by"] = edge.clone();
    agent::atomic_json(&path, &document).unwrap();
    // Only a token something names is kept: here the designation names the old generation.
    fixture.manager().coordinator_set("worker").unwrap();
    fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap();
    let new = fixture.manager().load("worker").unwrap();
    assert_ne!(new.token, old.token);
    assert_eq!(
        fixture.manager().coordinator_show().unwrap()["state"],
        "running",
        "the designation follows the revived agent"
    );
    assert_eq!(new.former_tokens(), std::slice::from_ref(&old.token));
    assert!(new.answers_to(&old.token));
    assert_eq!(json!(new.edge(crate::subagents::control::MANAGED_BY)), edge);
    assert_eq!(json!(new.edge(crate::subagents::control::CREATED_BY)), edge);
}

#[test]
fn revive_preserves_entire_old_queue_and_requested_task_without_replaying_either() {
    let (fixture, mut old) = recoverable("codex", true);
    old.paused = true;
    old.goal = Some("Finish the original task".to_owned());
    old.goal_delivery = Some("delivered".to_owned());
    old.goal_session_id = old.session_value.clone();
    old.goal_message_id = Some("goal-1".to_owned());
    old.goal_messages
        .insert("goal-1".to_owned(), old.goal.clone().unwrap());
    old.extra
        .insert("owner_extension".to_owned(), json!({"retained":true}));
    fixture.manager().save(&old).unwrap();
    let old_directory = fixture.root.join("registry/agents/worker");
    let queue = old_directory.join("queue");
    agent::create_private_directory(&queue, "test old queue", false, false).unwrap();
    for directory in ["pending", "failed", "processed"] {
        let path = queue.join(directory);
        agent::create_private_directory(&path, "test queue lane", false, false).unwrap();
        agent::atomic_json(
            &path.join("message.json"),
            &json!({"text":"Do not replay", "lane":directory}),
        )
        .unwrap();
    }
    for name in [".delivery.lock", ".binding.lock"] {
        agent::open_private_lock(&queue.join(name), "test queue lock").unwrap();
    }
    let before = files(&queue);
    let old_bytes = fs::read(old_directory.join("agent.json")).unwrap();
    let result = fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap();
    let archive = PathBuf::from(result["archive"].as_str().unwrap());
    assert_eq!(files(&archive.join("queue")), before);
    let mut expected: Value = serde_json::from_slice(&old_bytes).unwrap();
    expected["lifecycle"] = json!("stopped");
    assert_eq!(
        agent::read_private_json(&archive.join("agent.json")).unwrap(),
        expected
    );
    let new = fixture.manager().load("worker").unwrap();
    assert!(new.paused);
    assert_eq!(new.goal, old.goal);
    assert_eq!(new.extra["owner_extension"], old.extra["owner_extension"]);
    assert!(
        new.goal_delivery.is_none()
            && new.goal_session_id.is_none()
            && new.goal_message_id.is_none()
    );
    assert!(new.goal_messages.is_empty());
    assert!(fixture.client.runs.lock().unwrap().is_empty());
    assert!(fixture.client.keys_sent.lock().unwrap().is_empty());
    assert!(!fixture
        .root
        .join("registry/agents/worker/queue/pending/message.json")
        .exists());
}

#[test]
fn revive_preserves_producer_decimals_in_stopped_and_resumed_generations() {
    let timestamp = 1_791_594_983.664_875_7;
    let numbers = json!({
        "timestamp": timestamp,
        "nested": [0.123_456_789_012_345_68, {"signed": -timestamp}],
    });
    for point in ["ready", "published"] {
        let (fixture, mut old) = recoverable("codex", true);
        old.created_at = timestamp;
        old.extra
            .insert("preserved_numbers".to_owned(), numbers.clone());
        fixture.manager().save(&old).unwrap();
        let original: Value = serde_json::from_slice(
            &fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(original["created_at"].as_f64(), Some(timestamp));
        assert_eq!(original["preserved_numbers"], numbers);

        super::super::revive::fail_once(point);
        fixture
            .manager()
            .revive("worker", false, Some(&old.token), TIMEOUT)
            .unwrap_err();
        let operation = fixture.root.join("registry/.revives").join(&old.token);
        let stopped = agent::read_private_json(&operation.join("stopped.json")).unwrap();
        let mut expected_stopped = original;
        expected_stopped["lifecycle"] = json!("stopped");
        assert_eq!(stopped, expected_stopped);
        let candidate_path = if point == "published" {
            fixture.root.join("registry/agents/worker/agent.json")
        } else {
            operation.join("new/agent.json")
        };
        let candidate = agent::read_private_json(&candidate_path).unwrap();
        assert_eq!(candidate["preserved_numbers"], numbers);
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, Some(&old.token), TIMEOUT)
                .unwrap()["action"],
            "recover"
        );
        let result = fixture
            .manager()
            .revive("worker", false, Some(&old.token), TIMEOUT)
            .unwrap();
        let archive = PathBuf::from(result["archive"].as_str().unwrap());
        assert_eq!(
            agent::read_private_json(&archive.join("agent.json")).unwrap(),
            expected_stopped
        );
        assert_eq!(
            fixture.manager().load("worker").unwrap().extra["preserved_numbers"],
            numbers
        );
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    }
}

#[test]
fn revive_dry_run_is_an_immutable_safe_plan() {
    let (fixture, old) = recoverable("claude", false);
    let registry = fixture.root.join("registry");
    let before = files(&registry);
    let plan = fixture
        .manager()
        .revive("worker", true, Some(&old.token), TIMEOUT)
        .unwrap();
    assert_eq!(plan["action"], "revive");
    assert_eq!(plan["native_session"], json!(old.native_session));
    assert!(plan.get("arguments").is_none() && plan.get("environment").is_none());
    assert_eq!(files(&registry), before);
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
}

#[test]
fn revive_live_unknown_and_unanchored_processes_never_allocate_a_tab() {
    for (state, action) in [
        ("live", "skip"),
        ("unknown", "blocked"),
        ("unanchored", "blocked"),
    ] {
        let (fixture, mut old) = recoverable("claude", false);
        match state {
            "live" => {
                fixture.client.dead_panes.lock().unwrap().clear();
                fixture.client.started.store(true, Ordering::Relaxed);
            }
            "unknown" => fixture
                .client
                .liveness_unknown
                .store(true, Ordering::Relaxed),
            _ => {
                old.harness_identity = None;
                old.anchor_rule = None;
                fixture.manager().save(&old).unwrap();
            }
        }
        let before = files(&fixture.root.join("registry"));
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .unwrap()["action"],
            action
        );
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        // Actual attempts may create lifecycle locks; all record bytes are retained.
        assert_eq!(
            fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap(),
            before[Path::new("agents/worker/agent.json")]
                .3
                .clone()
                .unwrap()
        );
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}

#[test]
fn revive_all_skips_live_or_nonrunning_records_while_named_actual_refuses_them() {
    for state in ["live", "stopped"] {
        let (fixture, mut old) = recoverable("claude", false);
        if state == "live" {
            fixture.client.dead_panes.lock().unwrap().clear();
            fixture.client.started.store(true, Ordering::Relaxed);
        } else {
            old.lifecycle = "stopped".to_owned();
            fixture.manager().save(&old).unwrap();
        }
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{state}"
        );
        let result = fixture.manager().revive_all(false, TIMEOUT).unwrap();
        assert_eq!(result["revived"], 0);
        assert_eq!(result["blocked"], 0);
        assert_eq!(result["agents"][0]["action"], "skip", "{state}");
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}

#[test]
fn revive_refuses_unsupported_aliases_and_unsafe_environment_replay() {
    for kind in [
        "adopted",
        "headless",
        "environment",
        "missing-conversation",
        "move",
        "sibling",
    ] {
        let (fixture, mut old) = recoverable("codex", true);
        match kind {
            "adopted" => {
                // An adopted record without a launch spec has nothing to replay.
                old.adapter = "herdr-foreign".to_owned();
                old.foreign_shell_identity = Some(Fake::foreign_shell_identity());
                old.launch_spec = None;
            }
            "headless" => {
                old.mode = "headless".to_owned();
                old.backend = "worker".to_owned();
            }
            "environment" => {
                old.environment_names = vec!["PRIVATE_MODE".to_owned()];
            }
            "missing-conversation" => {
                old.native_session = None;
                old.session_agent = None;
                old.session_value = None;
                old.resume = None;
            }
            "move" => {
                fixture
                    .manager()
                    .write_move_intent(&old, "project-workspace")
                    .unwrap();
            }
            "sibling" => fixture
                .client
                .panes
                .lock()
                .unwrap()
                .push(Fake::pane("human")),
            _ => unreachable!(),
        }
        fixture.manager().save(&old).unwrap();
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .unwrap()["action"],
            "blocked",
            "{kind}"
        );
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{kind}"
        );
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            1,
            "{kind}"
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}

#[test]
fn revive_requires_positive_missing_target_and_absent_unambiguous_census() {
    let (fixture, old) = recoverable("claude", false);
    let pane = old.pane_id.as_ref().unwrap();
    fixture
        .client
        .panes
        .lock()
        .unwrap()
        .retain(|item| &item.pane_id != pane);
    assert_eq!(
        fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap()["action"],
        "blocked"
    );
    fixture
        .client
        .herdr_closed
        .lock()
        .unwrap()
        .insert(pane.clone());
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
}

#[test]
fn revive_refuses_a_missing_old_pane_when_its_tab_has_been_reused() {
    let (fixture, old) = recoverable("claude", false);
    let pane = old.pane_id.as_ref().unwrap();
    let tab = old.tab_id.as_ref().unwrap();
    fixture
        .client
        .panes
        .lock()
        .unwrap()
        .retain(|item| &item.pane_id != pane);
    fixture
        .client
        .herdr_closed
        .lock()
        .unwrap()
        .insert(pane.clone());
    let mut unrelated = Fake::pane("human");
    unrelated.tab_id = tab.clone();
    fixture.client.panes.lock().unwrap().push(unrelated);
    let before = files(&fixture.root.join("registry"));
    assert_eq!(
        fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap()["action"],
        "blocked"
    );
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    assert!(!journal_path(&fixture, &old).exists());
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
}

#[test]
fn revive_refuses_changed_pause_policy_before_candidate_ready() {
    let (fixture, mut old) = recoverable("claude", false);
    old.paused = true;
    fixture.manager().save(&old).unwrap();
    super::super::revive::fail_once("launched");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let path = stage_path(&fixture, &old).join("agent.json");
    let mut candidate = agent::read_private_json(&path).unwrap();
    candidate["paused"] = json!(false);
    agent::atomic_json(&path, &candidate).unwrap();
    let before = files(&fixture.root.join("registry"));
    assert_eq!(
        fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap()["action"],
        "blocked"
    );
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
}

#[test]
fn revive_refuses_partial_or_different_provider_legacy_routing_identity() {
    for provider in [None, Some("claude")] {
        let (fixture, mut old) = recoverable("codex", true);
        old.native_session = None;
        old.session_agent = provider.map(str::to_owned);
        fixture.manager().save(&old).unwrap();
        let before = files(&fixture.root.join("registry"));
        let plan = fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap();
        assert_eq!(plan["action"], "blocked");
        assert!(plan["reason"].as_str().unwrap().contains("provider"));
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        assert_eq!(files(&fixture.root.join("registry")), before);
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    }
}

#[test]
fn revive_pending_dry_run_detects_replaced_old_directory_and_unstopped_archive() {
    for modification in ["directory", "archive"] {
        let (fixture, old) = recoverable("claude", false);
        let original = fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap();
        super::super::revive::fail_once(if modification == "directory" {
            "ready"
        } else {
            "archived"
        });
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        if modification == "directory" {
            let active = fixture.root.join("registry/agents/worker");
            fs::rename(&active, fixture.root.join("retained-original")).unwrap();
            DirBuilder::new().mode(0o700).create(&active).unwrap();
            fs::write(active.join("agent.json"), &original).unwrap();
            fs::set_permissions(active.join("agent.json"), fs::Permissions::from_mode(0o600))
                .unwrap();
        } else {
            let path = fixture
                .root
                .join("registry/archive")
                .join(format!("worker-{}", old.token))
                .join("agent.json");
            fs::write(path, &original).unwrap();
        }
        let before = files(&fixture.root.join("registry"));
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .unwrap()["action"],
            "blocked",
            "{modification}"
        );
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{modification}"
        );
        assert_eq!(
            files(&fixture.root.join("registry")),
            before,
            "{modification}"
        );
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{modification}"
        );
        assert!(
            fixture.client.closed.lock().unwrap().is_empty(),
            "{modification}"
        );
    }
}

#[test]
fn revive_journal_survives_device_change_but_still_pins_inode_and_sha() {
    // #305 revive-device-pin: a journal written before a reboot names the
    // pre-reboot anonymous device (btrfs renumbers per mount). Rewriting only
    // the journal devices simulates that remount: the revive must proceed,
    // while a changed inode or record SHA must still refuse.
    for drift in ["device", "inode", "sha"] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once("ready");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        let path = journal_path(&fixture, &old);
        let mut journal = agent::read_private_json(&path).unwrap();
        match drift {
            "device" => {
                for key in ["old_directory_device", "new_directory_device"] {
                    journal[key] = json!(journal[key].as_u64().unwrap() + 100_000);
                }
            }
            "inode" => {
                journal["new_directory_inode"] =
                    json!(journal["new_directory_inode"].as_u64().unwrap() + 1);
            }
            _ => {
                journal["old_record_sha256"] = json!("0".repeat(64));
            }
        }
        agent::atomic_json(&path, &journal).unwrap();
        if drift == "device" {
            assert_eq!(
                fixture
                    .manager()
                    .revive("worker", false, None, TIMEOUT)
                    .unwrap()["revived"],
                true,
                "{drift}"
            );
            assert!(!path.exists(), "{drift}");
        } else {
            assert!(
                fixture
                    .manager()
                    .revive("worker", false, None, TIMEOUT)
                    .is_err(),
                "{drift}"
            );
            assert!(path.exists(), "{drift}");
        }
    }
}

#[test]
fn revive_pending_journal_refuses_symlink_or_hardlink_authority() {
    for link in ["symlink", "hardlink"] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once("ready");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        let path = journal_path(&fixture, &old);
        let saved = path.with_extension("retained");
        fs::rename(&path, &saved).unwrap();
        if link == "symlink" {
            std::os::unix::fs::symlink(&saved, &path).unwrap();
        } else {
            fs::hard_link(&saved, &path).unwrap();
        }
        let before = files(&fixture.root.join("registry"));
        assert!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .is_err(),
            "{link}"
        );
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{link}"
        );
        assert_eq!(files(&fixture.root.join("registry")), before, "{link}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{link}"
        );
        assert!(fixture.client.closed.lock().unwrap().is_empty(), "{link}");
    }
}

#[test]
fn revive_muse_preserves_globals_before_the_resume_subcommand() {
    let (fixture, old) = recoverable("muse", true);
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(
        new.arguments,
        ["resume", &old.native_session.unwrap().value]
    );
    assert!(new.custom_process_identity.is_some());
    assert_eq!(
        harness_arguments(
            "muse",
            Some("model"),
            Some("native"),
            &[
                "--reasoning-effort".to_owned(),
                "high".to_owned(),
                "--permission-mode".to_owned(),
                "owner-policy".to_owned()
            ]
        )
        .unwrap(),
        [
            "--model",
            "model",
            "--reasoning-effort",
            "high",
            "--permission-mode",
            "owner-policy",
            "resume",
            "native"
        ]
    );
}

#[test]
fn revive_reuses_a_proved_idle_shell_pane_and_records_the_reanchor() {
    // Pane reuse (#744): the dead harness left a plain shell in its own pane,
    // reporting no session, so revive relaunches there instead of a new tab.
    let (fixture, old) = recoverable("muse", true);
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    assert_eq!(result["pane_closed"], false);
    assert_eq!(result["tab_closed"], false);
    let resumed = fixture.manager().load("worker").unwrap();
    assert_eq!(resumed.pane_id, old.pane_id);
    assert_eq!(resumed.tab_id, old.tab_id);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    let reanchors = resumed.extra["reanchors"].as_array().unwrap();
    let entry = reanchors.last().unwrap();
    assert_eq!(entry["reason"], "reuse");
    assert_eq!(entry["previous"]["pane_id"], old.pane_id.clone().unwrap());
    assert_eq!(entry["current"]["pane_id"], old.pane_id.unwrap());
    assert_eq!(entry["slot_generation"], serde_json::Value::Null);
}

#[test]
fn revive_with_profile_environment_never_reuses_the_pane_and_delivers_the_values() {
    // #1139: the reused shell inherits its own environment, so an
    // env-carrying launch relaunched there silently loses the profile
    // values. It must take a fresh tab that receives --env instead.
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture.client.report_session.store(true, Ordering::Relaxed);
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test profile config", false, false).unwrap();
    agent::atomic_json(
        &directory.join("profiles.json"),
        &json!({"schema":"agentctl-profiles/v1","profiles":{
            "muse-env": {"harness":"muse","mode":"interactive", "env":{"RECOVERY_MODE":"on"}}
        }}),
    )
    .unwrap();
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "muse".to_owned(),
                profile: Some("muse-env".to_owned()),
                environment: vec!["RECOVERY_MODE=on".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    let old = fixture.manager().load("worker").unwrap();
    assert_eq!(old.environment_names, ["RECOVERY_MODE"]);
    kill_owned(&fixture, &old);
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true, "{result}");
    let resumed = fixture.manager().load("worker").unwrap();
    assert_ne!(
        resumed.pane_id, old.pane_id,
        "an env-carrying revive must not reuse the old pane"
    );
    let environments = fixture.client.environments.lock().unwrap();
    assert_eq!(environments.len(), 2);
    assert_eq!(environments[1], ["RECOVERY_MODE=on"]);
}

#[test]
fn revive_reuse_recovers_after_an_interruption_without_a_new_tab() {
    let (fixture, old) = recoverable("muse", true);
    super::super::revive::fail_once("launched");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true);
    assert_eq!(result["pane_closed"], false);
    let resumed = fixture.manager().load("worker").unwrap();
    assert_eq!(resumed.pane_id, old.pane_id);
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
}

#[test]
fn revive_resumes_muse_with_a_proved_stale_native_report_and_reconciles_each_boundary() {
    for interruption in [None, Some("launched"), Some("ready"), Some("archived")] {
        let (fixture, old) = recoverable("muse", true);
        fixture
            .client
            .stale_reported_panes
            .lock()
            .unwrap()
            .insert(old.pane_id.clone().unwrap());
        let info = fixture
            .client
            .pane_info(old.pane_id.as_ref().unwrap())
            .unwrap();
        assert_eq!(info.agent.as_deref(), Some("muse"));
        assert_eq!(info.session_agent, old.session_agent);
        assert_eq!(info.session_value, old.session_value);
        assert!(old.pane_reported_by_agentctl);
        if let Some(point) = interruption {
            super::super::revive::fail_once(point);
            assert!(fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err());
            let stage: AgentRecord = serde_json::from_value(
                agent::read_private_json(&stage_path(&fixture, &old).join("agent.json")).unwrap(),
            )
            .unwrap();
            let ordinary =
                agent::resolve_target(&fixture.client, &fixture.manager().target(&stage).unwrap())
                    .unwrap_err()
                    .to_string();
            assert!(ordinary.contains("found 2"), "ordinary census: {ordinary}");
            let before = files(&fixture.root.join("registry"));
            assert_eq!(
                fixture
                    .manager()
                    .revive("worker", true, None, TIMEOUT)
                    .unwrap()["action"],
                "recover",
                "{point}"
            );
            assert_eq!(files(&fixture.root.join("registry")), before);
        }
        assert_eq!(
            fixture
                .manager()
                .revive("worker", false, Some(&old.token), TIMEOUT)
                .unwrap()["revived"],
            true,
            "{interruption:?}"
        );
        let resumed = fixture.manager().load("worker").unwrap();
        assert_eq!(resumed.session_value, old.session_value);
        assert!(resumed.custom_process_identity.is_some());
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
        assert_eq!(
            *fixture.client.closed.lock().unwrap(),
            [old.pane_id.clone().unwrap()]
        );
    }
}

#[test]
fn revive_stale_session_exclusion_preserves_refusal_for_a_third_live_duplicate() {
    for workspace in ["workspace", "unrelated-workspace"] {
        let (fixture, old) = recoverable("muse", true);
        fixture
            .client
            .stale_reported_panes
            .lock()
            .unwrap()
            .insert(old.pane_id.clone().unwrap());
        super::super::revive::fail_once("launched");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        let mut third = Fake::pane("third-live");
        third.tab_id = "third-tab".to_owned();
        third.workspace_id = workspace.to_owned();
        fixture.client.panes.lock().unwrap().push(third);
        fixture
            .client
            .duplicate_session
            .store(true, Ordering::Relaxed);
        let before = files(&fixture.root.join("registry"));
        let plan = fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap();
        assert_eq!(plan["action"], "blocked");
        assert!(
            plan["reason"].as_str().unwrap().contains("found 2"),
            "{plan}"
        );
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        assert_eq!(files(&fixture.root.join("registry")), before);
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
        fixture
            .client
            .panes
            .lock()
            .unwrap()
            .retain(|pane| pane.pane_id != "third-live");
        fixture
            .client
            .duplicate_session
            .store(false, Ordering::Relaxed);
        assert_eq!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .unwrap()["revived"],
            true
        );
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    }
}

#[test]
fn revive_stale_session_exclusion_requires_the_saved_death_and_shell_proofs() {
    for changed in ["alive", "unknown", "shell"] {
        let (fixture, old) = recoverable("muse", true);
        fixture
            .client
            .stale_reported_panes
            .lock()
            .unwrap()
            .insert(old.pane_id.clone().unwrap());
        super::super::revive::fail_once("launched");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        match changed {
            "alive" => {
                fixture
                    .client
                    .dead_panes
                    .lock()
                    .unwrap()
                    .remove(old.pane_id.as_ref().unwrap());
            }
            "unknown" => fixture
                .client
                .liveness_unknown
                .store(true, Ordering::Relaxed),
            _ => {
                fixture
                    .client
                    .foreign_shell_identity
                    .lock()
                    .unwrap()
                    .starttime_ticks += 1
            }
        }
        let before = files(&fixture.root.join("registry"));
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .unwrap()["action"],
            "blocked",
            "{changed}"
        );
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{changed}"
        );
        assert_eq!(files(&fixture.root.join("registry")), before, "{changed}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{changed}"
        );
        assert!(
            fixture.client.closed.lock().unwrap().is_empty(),
            "{changed}"
        );
    }
}

#[test]
fn revive_rejects_a_different_reported_conversation_before_retirement() {
    let (fixture, old) = recoverable("claude", true);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("claude".to_owned()), Some("different".to_owned())));
    let old_bytes = fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap();
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap(),
        old_bytes
    );
    assert!(journal_path(&fixture, &old).exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    // #1138: the rerun abandons the unanchored candidate — closing the
    // pane this revive opened, whose harness reports a different
    // conversation — and launches once more (bounded at one relaunch). The
    // pane still reports a different conversation, so the rerun still
    // refuses before anything is retired.
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 3);
    assert_eq!(
        fs::read(fixture.root.join("registry/agents/worker/agent.json")).unwrap(),
        old_bytes
    );
    assert!(journal_path(&fixture, &old).exists());
    let (_, kept) = abandoned_attempt(&fixture, &old);
    assert!(
        kept["evidence"]
            .as_str()
            .unwrap_or_default()
            .contains("different native conversation"),
        "{kept}"
    );
}

#[test]
fn a_starting_candidate_whose_harness_never_came_up_is_abandoned_and_relaunched() {
    // #1138: a revive killed mid-launch (a boot is exactly that) leaves the
    // candidate `starting`, never stamped `launch_failed`. The #920 own-pane
    // close-and-relaunch must still apply without --abandon-candidate.
    let (fixture, old) = recoverable("codex", true);
    fixture
        .client
        .fail_start_once
        .store(true, Ordering::Relaxed);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let path = stage_path(&fixture, &old).join("agent.json");
    let mut candidate = agent::read_private_json(&path).unwrap();
    assert_eq!(candidate["lifecycle"], "launch_failed");
    candidate["lifecycle"] = json!("starting");
    agent::atomic_json(&path, &candidate).unwrap();
    let (_, staged, _) =
        super::super::revive::read_record_at(&stage_path(&fixture, &old), "worker").unwrap();
    let pane = staged.pane_id.clone().expect("the candidate's pane");
    fixture
        .client
        .custom_at_idle_shell
        .store(false, Ordering::Relaxed);
    let result = fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap();
    assert_eq!(result["revived"], true, "{result}");
    assert!(
        result["abandoned_candidate"]["evidence"]
            .as_str()
            .unwrap_or_default()
            .contains("never came up"),
        "{result}"
    );
    assert!(fixture.client.closed.lock().unwrap().contains(&pane));
}

#[test]
fn an_unanchored_candidate_in_a_foreign_pane_is_abandoned_without_closing_it() {
    // #1138: the candidate pinned no runtime and its pane reports a
    // different conversation, but the pane is not one this revive opened
    // (the recorded tab is not the pane's), so the attempt is abandoned
    // and relaunched while the foreign pane keeps running.
    let (fixture, old) = recoverable("claude", true);
    *fixture.client.session_override.lock().unwrap() =
        Some((Some("claude".to_owned()), Some("different".to_owned())));
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let path = stage_path(&fixture, &old).join("agent.json");
    let mut candidate = agent::read_private_json(&path).unwrap();
    let pane = candidate["pane_id"].as_str().unwrap().to_owned();
    candidate["tab_id"] = json!("foreign-tab");
    agent::atomic_json(&path, &candidate).unwrap();
    // The pane keeps reporting the different conversation, so the bounded
    // relaunch after the abandon fails the same way; what matters is that
    // the first attempt was abandoned on that evidence, one new launch was
    // made, and the foreign pane was never closed.
    assert!(fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .is_err());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 3);
    assert!(
        !fixture.client.closed.lock().unwrap().contains(&pane),
        "the foreign pane must never be closed"
    );
    let (_, kept) = abandoned_attempt(&fixture, &old);
    assert_eq!(kept["explicit"], false);
    assert!(
        kept["evidence"]
            .as_str()
            .unwrap_or_default()
            .contains("different native conversation"),
        "{kept}"
    );
}

#[test]
fn revive_resumes_each_crash_boundary_without_allocating_again() {
    for point in [
        "launched",
        "ready",
        "stopped",
        "archived",
        "published",
        "closed",
    ] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once(point);
        assert!(
            fixture
                .manager()
                .revive("worker", false, Some(&old.token), TIMEOUT)
                .is_err(),
            "{point}"
        );
        assert!(journal_path(&fixture, &old).exists(), "{point}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{point}"
        );
        let result = fixture
            .manager()
            .revive("worker", false, Some(&old.token), TIMEOUT)
            .unwrap_or_else(|error| panic!("{point}: {error}"));
        assert_eq!(result["revived"], true, "{point}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{point}"
        );
        assert!(!journal_path(&fixture, &old).exists(), "{point}");
    }
}

/// A revive interrupted after its candidate launched, and the candidate's staged record.
fn interrupted_after_launch(harness: &str) -> (Fixture, AgentRecord, AgentRecord) {
    let (fixture, old) = recoverable(harness, false);
    super::super::revive::fail_once("launched");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let path = stage_path(&fixture, &old);
    let (_, candidate, _) = super::super::revive::read_record_at(&path, "worker").unwrap();
    assert!(candidate.pane_id.is_some());
    (fixture, old, candidate)
}

/// The one abandoned attempt kept for `old`, and its `abandoned.json`.
fn abandoned_attempt(fixture: &Fixture, old: &AgentRecord) -> (PathBuf, Value) {
    let parent = fixture.root.join("registry/.revives/.abandoned");
    let entries: Vec<PathBuf> = fs::read_dir(&parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    let kept = entries[0].clone();
    assert!(kept
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with(&old.token));
    assert!(kept.join("journal.json").is_file());
    assert!(kept.join("operation/new/agent.json").is_file());
    let evidence = agent::read_private_json(&kept.join("abandoned.json")).unwrap();
    (kept, evidence)
}

#[test]
fn a_dead_candidate_is_abandoned_with_its_evidence_and_the_agent_launches_again() {
    // #899 revive-abandon-dead-candidate: a candidate pane that was closed, and one left holding
    // only an idle shell, are both proof that no harness runs; the next revive marks the attempt
    // failed by itself and launches.
    for closed in [true, false] {
        let (fixture, old, candidate) = interrupted_after_launch("claude");
        let pane = candidate.pane_id.clone().unwrap();
        fixture
            .client
            .dead_panes
            .lock()
            .unwrap()
            .insert(pane.clone());
        if closed {
            fixture
                .client
                .panes
                .lock()
                .unwrap()
                .retain(|row| row.pane_id != pane);
        }
        let plan = fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap();
        assert_eq!(plan["action"], "revive", "{plan}");
        let result = fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .unwrap();
        let wanted = if closed { "is gone" } else { "idle shell" };
        let evidence = result["abandoned_candidate"]["evidence"]
            .as_str()
            .unwrap_or_default();
        assert!(evidence.contains(wanted), "{result}");
        let (_, kept) = abandoned_attempt(&fixture, &old);
        assert_eq!(kept["evidence"], evidence);
        assert_eq!(kept["explicit"], false);
        assert_eq!(kept["new_token"], json!(candidate.token));
        assert!(!journal_path(&fixture, &old).exists());
        let new = fixture.manager().load("worker").unwrap();
        assert_ne!(new.token, old.token);
        assert_ne!(new.token, candidate.token);
        assert_eq!(new.lifecycle, "running");
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 3);
    }
}

#[test]
fn a_candidate_with_a_live_harness_is_never_abandoned() {
    let (fixture, old, candidate) = interrupted_after_launch("claude");
    let before = files(&fixture.root.join("registry"));
    let plan = fixture
        .manager()
        .revive_with("worker", true, None, TIMEOUT, true)
        .unwrap();
    assert_eq!(plan["action"], "blocked", "{plan}");
    let error = fixture
        .manager()
        .revive_with("worker", false, None, TIMEOUT, true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("is not abandoned"), "{error}");
    let pane = candidate.pane_id.unwrap();
    assert!(
        error.contains(&pane) || error.contains("process"),
        "{error}"
    );
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert!(journal_path(&fixture, &old).exists());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

#[test]
fn an_unprovable_candidate_is_abandoned_only_when_the_operator_asks() {
    // Interrupted before the candidate recorded a pane: a harness it may have started cannot be
    // checked, so revive refuses and names --abandon-candidate, which then relaunches. The old
    // queue goes to the archive exactly as in any revive, and its undelivered prompt is reported:
    // named in the result and failed in the new queue, never typed.
    let (fixture, old) = recoverable("claude", false);
    let queue = fixture.root.join("registry/agents/worker/queue/inbox");
    agent::create_private_directory(&queue, "test queue lane", true, false).unwrap();
    agent::atomic_json(
        &queue.join("m-1.json"),
        &json!({"id": "m-1", "text": "[from lane-a via agentctl · reply: agentctl send lane-a]\nplease run the gate"}),
    )
    .unwrap();
    let queued = files(&queue);
    super::super::revive::fail_once("launching");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let error = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string();
    assert!(error.contains("refusing another launch"), "{error}");
    assert!(error.contains("--abandon-candidate"), "{error}");
    let result = fixture
        .manager()
        .revive_with("worker", false, None, TIMEOUT, true)
        .unwrap();
    assert!(result["abandoned_candidate"]["evidence"]
        .as_str()
        .unwrap()
        .contains("recorded no pane"));
    let (_, kept) = abandoned_attempt(&fixture, &old);
    assert_eq!(kept["explicit"], true);
    let archive = PathBuf::from(result["archive"].as_str().unwrap());
    assert_eq!(files(&archive.join("queue/inbox")), queued);
    assert_eq!(
        fixture.manager().load("worker").unwrap().lifecycle,
        "running"
    );
    assert_eq!(
        result["archived_prompts"],
        json!([{"id": "m-1", "sender": "lane-a", "first_line": "please run the gate", "lane": "inbox"}]),
        "{result}"
    );
    assert_eq!(
        fixture.manager().message_state("worker", "m-1").unwrap(),
        Some(crate::agent::QueueMessageState::Failed)
    );
    let failed = crate::failed_digest::failed_entries(&fixture.manager().queue("worker").unwrap());
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].error.contains("archived by revive"), "{failed:?}");
    assert!(
        fixture.client.runs.lock().unwrap().is_empty(),
        "never typed"
    );
    // With nothing interrupted, there is nothing to abandon.
    assert!(fixture
        .manager()
        .revive_with("worker", false, None, TIMEOUT, true)
        .unwrap_err()
        .to_string()
        .contains("no interrupted revive candidate"));
}

#[test]
fn revive_launch_once_latch_refuses_unanchored_interruption_and_gates_ordinary_controls() {
    let (fixture, old) = recoverable("claude", false);
    super::super::revive::fail_once("launching");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    assert!(fixture
        .manager()
        .get("worker")
        .unwrap_err()
        .to_string()
        .contains("revive"));
    assert!(fixture.manager().stop("worker").is_err());
    assert!(fixture
        .manager()
        .start("worker", &fixture.root, StartOptions::default())
        .is_err());
    let before = files(&fixture.root.join("registry"));
    let dry_run = fixture
        .manager()
        .revive("worker", true, Some(&old.token), TIMEOUT)
        .unwrap();
    assert_eq!(dry_run["action"], "blocked");
    assert!(dry_run["reason"]
        .as_str()
        .unwrap()
        .contains("no automatic relaunch"));
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string()
        .contains("refusing another launch"));
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
}

#[test]
fn revive_preserves_unlaunched_partial_setup_before_retrying_once() {
    for point in [
        "operation-created",
        "stage-created",
        "stopped-prepared",
        "candidate-prepared",
    ] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once(point);
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{point}"
        );
        assert!(!journal_path(&fixture, &old).exists(), "{point}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            1,
            "{point}"
        );
        let operation = stage_path(&fixture, &old).parent().unwrap().to_owned();
        let artifacts = files(&operation);
        let before = files(&fixture.root.join("registry"));
        assert_eq!(
            fixture
                .manager()
                .revive("worker", true, None, TIMEOUT)
                .unwrap()["action"],
            "revive",
            "{point}"
        );
        assert_eq!(files(&fixture.root.join("registry")), before, "{point}");
        assert_eq!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .unwrap()["revived"],
            true,
            "{point}"
        );
        let root = fixture.root.join("registry/.revives");
        let orphans: Vec<_> = fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".orphan-")
            })
            .collect();
        assert_eq!(orphans.len(), 1, "{point}");
        assert_eq!(files(&orphans[0]), artifacts, "{point}");
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            2,
            "{point}"
        );
    }
}

#[test]
fn revive_refuses_orphaned_setup_with_runtime_claims_or_unrecognized_artifacts() {
    for modification in ["pane", "session", "unknown", "symlink", "hardlink"] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once("candidate-prepared");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        let stage = stage_path(&fixture, &old);
        let record_path = stage.join("agent.json");
        match modification {
            "pane" | "session" => {
                let mut candidate = agent::read_private_json(&record_path).unwrap();
                if modification == "pane" {
                    candidate["pane_id"] = json!("unproven-pane");
                } else {
                    candidate["session_agent"] = json!("claude");
                    candidate["session_value"] = old
                        .native_session
                        .as_ref()
                        .map_or(Value::Null, |native| json!(native.value));
                }
                agent::atomic_json(&record_path, &candidate).unwrap();
            }
            "unknown" => fs::write(stage.join("unrecognized"), "retained").unwrap(),
            "symlink" => {
                std::os::unix::fs::symlink(&record_path, stage.join(".agent.json-recovery-1-1-0"))
                    .unwrap()
            }
            _ => fs::hard_link(&record_path, stage.join(".agent.json-recovery-1-1-0")).unwrap(),
        }
        let before = files(&fixture.root.join("registry"));
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{modification}"
        );
        assert_eq!(
            files(&fixture.root.join("registry")),
            before,
            "{modification}"
        );
        assert_eq!(
            fixture.client.environments.lock().unwrap().len(),
            1,
            "{modification}"
        );
    }
}

#[test]
fn revive_preserves_both_producers_private_setup_temporary_files() {
    let (fixture, old) = recoverable("claude", false);
    super::super::revive::fail_once("candidate-prepared");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let stage = stage_path(&fixture, &old);
    for name in [
        ".agent.json-recovery-17-123456789-0",
        ".agent.json-recovery-19-aabbccddeeff00112233445566778899",
    ] {
        let path = stage.join(name);
        fs::write(&path, b"retained temporary").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
    let operation = stage.parent().unwrap();
    let artifacts = files(operation);
    assert_eq!(
        fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .unwrap()["revived"],
        true
    );
    let orphan = fs::read_dir(fixture.root.join("registry/.revives"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".orphan-")
        })
        .unwrap();
    assert_eq!(files(&orphan), artifacts);
}

#[test]
fn revive_staged_pane_and_session_stay_reserved_across_archive_publication_gap() {
    let (fixture, old) = recoverable("codex", true);
    super::super::revive::fail_once("archived");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    assert!(!fixture.root.join("registry/agents/worker").exists());
    let candidate =
        agent::read_private_json(&stage_path(&fixture, &old).join("agent.json")).unwrap();
    let mut options = fixture.adopt_options();
    options.pane_id = candidate["pane_id"].as_str().unwrap().to_owned();
    let error = fixture
        .manager()
        .adopt("alias", options)
        .unwrap_err()
        .to_string();
    assert!(error.contains("already registered"), "{error}");
    assert!(!fixture.root.join("registry/agents/alias").exists());
    assert!(fixture
        .manager()
        .start(
            "duplicate",
            &fixture.root,
            StartOptions {
                resume: old
                    .native_session
                    .as_ref()
                    .map(|native| native.value.clone()),
                ..StartOptions::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already registered"));
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    let before = files(&fixture.root.join("registry"));
    assert_eq!(
        fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap()["action"],
        "recover"
    );
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert_eq!(
        fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .unwrap()["revived"],
        true
    );
}

#[test]
fn revive_recovers_failed_or_starting_candidate_only_with_its_saved_live_anchor() {
    for lifecycle in ["starting", "launch_failed"] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once("launched");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        let path = stage_path(&fixture, &old).join("agent.json");
        let mut candidate = agent::read_private_json(&path).unwrap();
        candidate["lifecycle"] = json!(lifecycle);
        candidate["error"] = json!("injected failure after pinning");
        agent::atomic_json(&path, &candidate).unwrap();
        assert_eq!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .unwrap()["revived"],
            true
        );
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
    }
}

#[test]
fn revive_lost_close_acknowledgment_reconciles_only_explicit_absence() {
    let (fixture, old) = recoverable("claude", false);
    fixture
        .client
        .fail_after_close
        .store(true, Ordering::Relaxed);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string()
        .contains("close is uncertain"));
    assert_eq!(
        agent::read_private_json(&journal_path(&fixture, &old)).unwrap()["phase"],
        "published"
    );
    assert_eq!(
        fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .unwrap()["revived"],
        true
    );
    assert_eq!(fixture.client.closed.lock().unwrap().len(), 1);
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

#[test]
fn revive_never_adopts_a_replacement_idle_shell_as_cleanup_authority() {
    let (fixture, old) = recoverable("claude", false);
    fixture.client.fail_close.store(true, Ordering::Relaxed);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    fixture.client.fail_close.store(false, Ordering::Relaxed);
    fixture
        .client
        .foreign_shell_identity
        .lock()
        .unwrap()
        .starttime_ticks += 1;
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string()
        .contains("shell generation changed"));
    assert!(journal_path(&fixture, &old).exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

#[test]
fn revive_published_retry_does_not_require_the_new_harness_to_still_run() {
    let (fixture, old) = recoverable("claude", false);
    fixture.client.fail_close.store(true, Ordering::Relaxed);
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let new: AgentRecord = serde_json::from_value(
        agent::read_private_json(&fixture.root.join("registry/agents/worker/agent.json")).unwrap(),
    )
    .unwrap();
    kill_owned(&fixture, &new);
    fixture.client.fail_close.store(false, Ordering::Relaxed);
    assert_eq!(
        fixture
            .manager()
            .revive("worker", false, Some(&old.token), TIMEOUT)
            .unwrap()["revived"],
        true
    );
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

#[test]
fn revive_published_retry_retains_the_journal_if_a_stage_reappears() {
    let (fixture, old) = recoverable("claude", false);
    super::super::revive::fail_once("published");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let stage = stage_path(&fixture, &old);
    DirBuilder::new().mode(0o700).create(&stage).unwrap();
    fs::copy(
        fixture.root.join("registry/agents/worker/agent.json"),
        stage.join("agent.json"),
    )
    .unwrap();
    let before = files(&fixture.root.join("registry"));
    assert_eq!(
        fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap()["action"],
        "blocked"
    );
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string()
        .contains("conflicting staged generation"));
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert!(journal_path(&fixture, &old).exists());
    assert!(fixture.client.closed.lock().unwrap().is_empty());
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

#[test]
fn revive_detects_changed_old_bytes_and_staged_directory_generation() {
    for mutation in ["old-bytes", "new-directory", "new-policy", "stopped-bytes"] {
        let (fixture, old) = recoverable("claude", false);
        super::super::revive::fail_once("ready");
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        match mutation {
            "old-bytes" => {
                let path = fixture.root.join("registry/agents/worker/agent.json");
                let mut value = agent::read_private_json(&path).unwrap();
                value["opaque"] = json!("changed");
                agent::atomic_json(&path, &value).unwrap();
            }
            "new-directory" => {
                let path = stage_path(&fixture, &old);
                let bytes = fs::read(path.join("agent.json")).unwrap();
                fs::rename(&path, path.with_file_name("replaced-new")).unwrap();
                agent::create_private_directory(&path, "replacement test candidate", false, false)
                    .unwrap();
                fs::write(path.join("agent.json"), bytes).unwrap();
                fs::set_permissions(path.join("agent.json"), fs::Permissions::from_mode(0o600))
                    .unwrap();
            }
            "new-policy" => {
                let path = stage_path(&fixture, &old).join("agent.json");
                let mut value = agent::read_private_json(&path).unwrap();
                value["arguments"] = json!(["--resume", "different"]);
                agent::atomic_json(&path, &value).unwrap();
            }
            _ => {
                let path = stage_path(&fixture, &old)
                    .parent()
                    .unwrap()
                    .join("stopped.json");
                let mut value = agent::read_private_json(&path).unwrap();
                value["opaque"] = json!("changed");
                agent::atomic_json(&path, &value).unwrap();
            }
        }
        assert!(
            fixture
                .manager()
                .revive("worker", false, None, TIMEOUT)
                .is_err(),
            "{mutation}"
        );
        assert!(journal_path(&fixture, &old).exists());
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
        assert!(fixture.client.closed.lock().unwrap().is_empty());
    }
}

#[test]
fn pending_failed_candidate_reserves_its_pane_and_native_conversation() {
    let (fixture, old) = recoverable("claude", false);
    super::super::revive::fail_once("launched");
    assert!(fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .is_err());
    let path = stage_path(&fixture, &old).join("agent.json");
    let mut candidate = agent::read_private_json(&path).unwrap();
    candidate["lifecycle"] = json!("launch_failed");
    agent::atomic_json(&path, &candidate).unwrap();
    let pane = candidate["pane_id"].as_str().unwrap();
    assert_eq!(
        fixture
            .manager()
            .claim_owner(pane, candidate["terminal_id"].as_str(), Some("peer"))
            .unwrap(),
        Some("worker".to_owned())
    );
    let native = old.native_session.unwrap();
    assert_eq!(
        fixture
            .manager()
            .identity_owner(&native.agent, &native.value, Some("peer"))
            .unwrap()
            .unwrap()
            .name,
        "worker"
    );
}

#[test]
fn revive_all_snapshots_tokens_and_continues_independent_blocked_records() {
    let (fixture, old) = recoverable("claude", false);
    let manager = fixture.manager();
    let mut peer = old.clone();
    peer.name = "peer".to_owned();
    peer.token = "different-generation".to_owned();
    peer.native_session = None;
    peer.session_agent = None;
    peer.session_value = None;
    peer.harness_identity = None;
    peer.anchor_rule = None;
    peer.pane_id = Some("unknown-pane".to_owned());
    peer.tab_id = Some("unknown-tab".to_owned());
    peer.terminal_id = Some("unknown-terminal".to_owned());
    let directory = fixture.root.join("registry/agents/peer");
    agent::create_private_directory(&directory, "test blocked peer", false, false).unwrap();
    manager.save(&peer).unwrap();
    let plan = manager.revive_all(true, TIMEOUT).unwrap();
    assert_eq!(plan["blocked"], 1);
    assert_eq!(plan["revived"], 0);
    let result = manager.revive_all(false, TIMEOUT).unwrap();
    assert_eq!(result["blocked"], 1);
    assert_eq!(result["revived"], 1);
    assert_eq!(result["agents"].as_array().unwrap().len(), 2);
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

fn profile_fixture() -> (Fixture, AgentRecord, PathBuf) {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    let output = std::process::Command::new("/usr/bin/git")
        .args(["init", "--quiet"])
        .current_dir(&fixture.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(fixture.root.join(".gitignore"), ".agentctl/\n").unwrap();
    let directory = fixture.root.join(".agentctl");
    agent::create_private_directory(&directory, "test profile config", false, false).unwrap();
    let path = directory.join("profiles.json");
    agent::atomic_json(&path, &json!({"schema":"agentctl-profiles/v1","profiles":{
        "claude-recovery": {"harness":"claude","mode":"interactive","model":"opus","reasoning_effort":"high",
            "argv":["--allowedTools","Read"], "env":{"RECOVERY_MODE":"profile-private-value"}}
    }})).unwrap();
    fixture
        .manager()
        .start_with_reasoning_effort(
            "worker",
            &fixture.root,
            "high",
            StartOptions {
                harness: "claude".to_owned(),
                profile: Some("claude-recovery".to_owned()),
                model: Some("opus".to_owned()),
                harness_args: vec!["--allowedTools".to_owned(), "Read".to_owned()],
                environment: vec!["RECOVERY_MODE=profile-private-value".to_owned()],
                ..StartOptions::default()
            },
        )
        .unwrap();
    let old = fixture.manager().load("worker").unwrap();
    kill_owned(&fixture, &old);
    (fixture, old, path)
}

#[test]
fn revive_loads_profile_environment_privately_and_keeps_its_policy() {
    let (fixture, old, _) = profile_fixture();
    let before = files(&fixture.root.join("registry"));
    let plan = fixture
        .manager()
        .revive("worker", true, None, TIMEOUT)
        .unwrap();
    assert_eq!(plan["action"], "revive");
    assert!(!plan.to_string().contains("profile-private-value"));
    assert_eq!(files(&fixture.root.join("registry")), before);
    let result = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap();
    assert_eq!(result["profile"], "claude-recovery");
    assert!(!result.to_string().contains("profile-private-value"));
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(&new.arguments[2..], &old.arguments[2..]);
    assert_eq!(
        fixture.client.environments.lock().unwrap()[1],
        ["RECOVERY_MODE=profile-private-value"]
    );
}

#[test]
fn revive_replays_a_spec_through_profile_drift_but_not_environment_name_drift() {
    for field in ["model", "effort", "argv", "environment"] {
        let (fixture, _, path) = profile_fixture();
        let mut config = agent::read_private_json(&path).unwrap();
        let profile = &mut config["profiles"]["claude-recovery"];
        match field {
            "model" => profile["model"] = json!("different"),
            "effort" => profile["reasoning_effort"] = json!("medium"),
            "argv" => profile["argv"] = json!(["--allowedTools", "Read,Write"]),
            _ => profile["env"] = json!({"DIFFERENT_MODE":"profile-private-value"}),
        }
        agent::atomic_json(&path, &config).unwrap();
        let plan = fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap();
        // The spec replays what ran; the profile only supplies the named environment values.
        let expected = if field == "environment" {
            "blocked"
        } else {
            "revive"
        };
        assert_eq!(plan["action"], expected, "{field}");
        assert!(!plan.to_string().contains("profile-private-value"));
    }
}

#[test]
fn revive_without_a_spec_refuses_profile_model_effort_argv_and_environment_name_drift() {
    for field in ["model", "effort", "argv", "environment"] {
        let (fixture, mut old, path) = profile_fixture();
        old.launch_spec = None;
        fixture.manager().save(&old).unwrap();
        let mut config = agent::read_private_json(&path).unwrap();
        let profile = &mut config["profiles"]["claude-recovery"];
        match field {
            "model" => profile["model"] = json!("different"),
            "effort" => profile["reasoning_effort"] = json!("medium"),
            "argv" => profile["argv"] = json!(["--allowedTools", "Read,Write"]),
            _ => profile["env"] = json!({"DIFFERENT_MODE":"profile-private-value"}),
        }
        agent::atomic_json(&path, &config).unwrap();
        let plan = fixture
            .manager()
            .revive("worker", true, None, TIMEOUT)
            .unwrap();
        assert_eq!(plan["action"], "blocked", "{field}");
        assert!(!plan.to_string().contains("profile-private-value"));
        assert!(fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .is_err());
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    }
}

// #1023 agentctl-restart-profile-refresh: an operator can move a stopped or restarted agent onto
// its profile's current definition without a new start.

/// Give the fixture's profile a new environment variable and new argv, as an operator editing
/// profiles.json does.
fn edit_profile(path: &Path) {
    let mut config = agent::read_private_json(path).unwrap();
    let profile = &mut config["profiles"]["claude-recovery"];
    profile["argv"] = json!(["--allowedTools", "Read,Write"]);
    profile["env"] = json!({"RECOVERY_MODE": "profile-private-value", "ULTRA_MODE": "on"});
    agent::atomic_json(path, &config).unwrap();
}

#[test]
fn revive_with_a_refreshed_profile_launches_its_new_environment_on_the_same_conversation() {
    let (fixture, old, path) = profile_fixture();
    let manager = fixture.manager();
    assert!(manager.status("worker").unwrap()["profile_drift"].is_null());
    edit_profile(&path);
    let drift = &manager.status("worker").unwrap()["profile_drift"];
    assert_eq!(
        drift["fields"],
        json!(["environment_names", "argv"]),
        "{drift}"
    );
    assert_eq!(drift["fix"], "agentctl restart worker --refresh-profile");
    let refreshed = manager.refresh_profile("worker").unwrap();
    assert_eq!(refreshed["changed"], json!(["environment_names", "argv"]));
    assert!(!refreshed.to_string().contains("profile-private-value"));
    manager.revive("worker", false, None, TIMEOUT).unwrap();
    assert_eq!(
        fixture.client.environments.lock().unwrap()[1],
        ["RECOVERY_MODE=profile-private-value", "ULTRA_MODE=on"]
    );
    let new = manager.load("worker").unwrap();
    assert_eq!(
        new.resume,
        old.native_session
            .as_ref()
            .map(|native| native.value.clone())
    );
    assert!(new
        .arguments
        .windows(2)
        .any(|pair| pair == ["--allowedTools", "Read,Write"]));
    assert_eq!(new.environment_names, ["RECOVERY_MODE", "ULTRA_MODE"]);
    assert!(manager.status("worker").unwrap()["profile_drift"].is_null());
}

#[test]
fn restart_with_a_refreshed_profile_relaunches_from_it() {
    let (fixture, _old, path) = profile_fixture();
    edit_profile(&path);
    let result = fixture
        .manager()
        .restart_stopping(
            "worker",
            false,
            Duration::from_secs(30),
            TIMEOUT,
            false,
            true,
        )
        .unwrap();
    assert_eq!(result["action"], "restarted", "{result}");
    assert_eq!(
        result["profile_refresh"],
        json!(["environment_names", "argv"])
    );
    assert_eq!(
        fixture.client.environments.lock().unwrap()[1],
        ["RECOVERY_MODE=profile-private-value", "ULTRA_MODE=on"]
    );
}

#[test]
fn a_profile_refresh_refuses_a_running_harness_and_a_profile_that_changed_harness() {
    let (fixture, old, path) = profile_fixture();
    let manager = fixture.manager();
    let pane = old.pane_id.clone().unwrap();
    fixture.client.dead_panes.lock().unwrap().remove(&pane);
    fixture.client.started.store(true, Ordering::Relaxed);
    fixture.client.custom_alive.store(true, Ordering::Relaxed);
    let error = manager.refresh_profile("worker").unwrap_err().to_string();
    assert!(error.contains("--refresh-profile"), "{error}");
    kill_owned(&fixture, &old);
    let mut config = agent::read_private_json(&path).unwrap();
    config["profiles"]["claude-recovery"]["harness"] = json!("codex");
    agent::atomic_json(&path, &config).unwrap();
    let error = manager.refresh_profile("worker").unwrap_err().to_string();
    assert!(error.contains("another harness or mode"), "{error}");
    assert_eq!(manager.load("worker").unwrap(), old);
}

#[test]
fn revive_checks_recorded_slot_path_and_isolation_in_read_only_preflight() {
    for field in ["path", "isolation"] {
        let (fixture, mut old) = recoverable("claude", false);
        old.slot = Some("slot_name".to_owned());
        old.slot_project = Some(fixture.root.display().to_string());
        old.slot_isolation = Some("userns".to_owned());
        let spec = old.launch_spec.as_mut().unwrap();
        spec["slot"] = json!(old.slot);
        spec["slot_project"] = json!(old.slot_project);
        spec["slot_isolation"] = json!(old.slot_isolation);
        fixture.manager().save(&old).unwrap();
        let bin = fixture.root.join("wrkslots-test");
        let output_path = if field == "path" {
            fixture.root.join("changed")
        } else {
            fixture.root.clone()
        };
        let output = json!({"command":"exec owner-policy", "slot_path":output_path, "isolation":if field == "path" { "userns" } else { "cgroup" }}).to_string();
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
            output.replace('\'', "'\\''")
        );
        fs::write(&bin, script).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        let before = files(&fixture.root.join("registry"));
        let mut manager = fixture.manager();
        manager.revive_wrkslots_executable = Some(bin);
        let plan = manager.revive("worker", true, None, TIMEOUT).unwrap();
        assert_eq!(plan["action"], "blocked");
        assert!(
            plan["reason"]
                .as_str()
                .unwrap()
                .contains("recorded slot directory or isolation changed"),
            "{plan}"
        );
        assert_eq!(files(&fixture.root.join("registry")), before);
        assert_eq!(fixture.client.environments.lock().unwrap().len(), 1);
    }
}

#[test]
fn revive_heals_a_slot_project_recorded_as_the_slot_itself() {
    // #1140: the record names its own slot directory as the project.
    // Forcing that root resolves double-nested, so the plan retries
    // unforced, the slot manager discovers the holding root, and only the
    // exact recorded cwd and isolation are accepted. (Dry run: the fake
    // Herdr reports every pane at the fixture root, so a real launch into
    // the nested slot directory cannot be modelled here.)
    let (fixture, mut old) = recoverable("claude", false);
    let project = fixture.root.join("project");
    let slot_dir = project.join("worktrees/slots/existing_slot");
    fs::create_dir_all(&slot_dir).unwrap();
    fs::write(project.join(".wrkslots.yml"), "project\n").unwrap();
    fs::write(slot_dir.join(".wrkslots.yml"), "slot copy\n").unwrap();
    let slot_dir = fs::canonicalize(&slot_dir).unwrap();
    old.cwd = slot_dir.display().to_string();
    old.slot = Some("existing_slot".to_owned());
    old.slot_project = Some(slot_dir.display().to_string());
    old.slot_isolation = Some("userns".to_owned());
    let spec = old.launch_spec.as_mut().unwrap();
    spec["cwd"] = json!(old.cwd);
    spec["slot"] = json!(old.slot);
    spec["slot_project"] = json!(old.slot_project);
    spec["slot_isolation"] = json!(old.slot_isolation);
    fixture.manager().save(&old).unwrap();
    // The recorded pane is gone (a reboot took it), so the recovery proof
    // is the missing-pane one and never compares the fake pane's cwd.
    let pane = old.pane_id.clone().unwrap();
    fixture
        .client
        .panes
        .lock()
        .unwrap()
        .retain(|row| row.pane_id != pane);
    fixture.client.herdr_closed.lock().unwrap().insert(pane);
    // Like the real slot manager: a forced root resolves nested under it;
    // unforced discovery from the slot resolves the slot itself.
    let nested = slot_dir.join("worktrees/slots/existing_slot");
    let argv_log = fixture.root.join("wrkslots-argv");
    let bin = fixture.root.join("wrkslots-test");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncase \"$*\" in *--project-root*) \
         printf '%s\\n' '{{\"command\":\"exec nested\",\"slot_path\":\"{}\",\"isolation\":\"userns\"}}' ;;\n\
         *) printf '%s\\n' '{{\"command\":\"exec healed\",\"slot_path\":\"{}\",\"isolation\":\"userns\"}}' ;;\nesac\n",
        argv_log.display(),
        nested.display(),
        slot_dir.display(),
    );
    fs::write(&bin, script).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let mut manager = fixture.manager();
    manager.revive_wrkslots_executable = Some(bin);
    let plan = manager.revive("worker", true, None, TIMEOUT).unwrap();
    assert_eq!(plan["action"], "revive", "{plan}");
    let argv = fs::read_to_string(&argv_log).unwrap();
    assert!(
        !argv.contains("--project-root"),
        "the heal must not force the recorded root: {argv}"
    );
}

#[test]
fn revive_reuses_the_recorded_slot_without_allocation_or_rebinding() {
    let (fixture, mut old) = recoverable("claude", false);
    old.slot = Some("existing_slot".to_owned());
    old.slot_project = Some(fixture.root.display().to_string());
    old.slot_isolation = Some("userns".to_owned());
    let spec = old.launch_spec.as_mut().unwrap();
    spec["slot"] = json!(old.slot);
    spec["slot_project"] = json!(old.slot_project);
    spec["slot_isolation"] = json!(old.slot_isolation);
    fixture.manager().save(&old).unwrap();
    let output = json!({"command":"exec preserved-owner-policy", "slot_path":fixture.root, "isolation":"userns"}).to_string();
    let bin = fixture.root.join("wrkslots-test");
    fs::write(
        &bin,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
            output.replace('\'', "'\\''")
        ),
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
    let mut manager = fixture.manager();
    manager.revive_wrkslots_executable = Some(bin);
    let before = files(&fixture.root.join("registry"));
    assert_eq!(
        manager.revive("worker", true, None, TIMEOUT).unwrap()["action"],
        "revive"
    );
    assert_eq!(files(&fixture.root.join("registry")), before);
    assert!(fixture.client.slot_commands.lock().unwrap().is_empty());
    assert_eq!(
        manager.revive("worker", false, None, TIMEOUT).unwrap()["revived"],
        true
    );
    let new = manager.load("worker").unwrap();
    assert_eq!(new.slot, old.slot);
    assert_eq!(new.slot_project, old.slot_project);
    assert_eq!(new.slot_isolation, old.slot_isolation);
    assert_eq!(
        *fixture.client.slot_commands.lock().unwrap(),
        [(
            new.pane_id.unwrap(),
            "exec preserved-owner-policy".to_owned()
        )]
    );
    assert_eq!(fixture.client.environments.lock().unwrap().len(), 2);
}

// --- Native ids captured at start and adopt (#916 muse-native-session-id) -----------------------

/// Pretend the agent's processes hold these files open until the guard drops.
struct PretendHeld;

impl Drop for PretendHeld {
    fn drop(&mut self) {
        crate::subagents::revive::HELD_FILES.with(|cell| *cell.borrow_mut() = None);
    }
}

#[must_use]
fn pretend_held(files: Vec<PathBuf>) -> PretendHeld {
    crate::subagents::revive::HELD_FILES.with(|cell| *cell.borrow_mut() = Some(files));
    PretendHeld
}

/// Pretend the Muse data root lives at `root` until the guard drops (#948).
struct PretendMuseStore;

impl Drop for PretendMuseStore {
    fn drop(&mut self) {
        crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = None);
    }
}

#[must_use]
fn pretend_muse_store(fixture: &Fixture) -> (PathBuf, PretendMuseStore) {
    let root = fixture.root.join("data/muse");
    crate::subagents::revive::MUSE_STORE_ROOT.with(|cell| *cell.borrow_mut() = Some(root.clone()));
    (root, PretendMuseStore)
}

/// A session journal under the store naming `cwd`, as a Muse runtime writes it.
fn fake_muse_journal(root: &Path, id: &str, cwd: &Path) {
    let dir = root.join("sessions/2026/11/06").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("session.jsonl"),
        format!("{{\"cwd\":\"{}\"}}\n", cwd.display()),
    )
    .unwrap();
}

/// What a fake Muse writes when it starts: `<data>/muse/sessions/2026/11/06/<id>/` with the
/// `.session.lock` its runtime holds (`pid=`, `host=`). Returns the lock.
fn fake_muse_session(fixture: &Fixture, id: &str) -> PathBuf {
    let dir = fixture.root.join("data/muse/sessions/2026/11/06").join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("session.jsonl"), "").unwrap();
    let lock = dir.join(".session.lock");
    fs::write(&lock, "pid=4242\nhost=localhost\n").unwrap();
    lock
}

/// A fixture whose Herdr reports no native session, as it reports none for Muse or Codex.
fn unreported() -> Fixture {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .client
        .report_session
        .store(false, Ordering::Relaxed);
    fixture
}

fn start_worker(fixture: &Fixture, harness: &str) -> Value {
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: harness.to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap()
}

/// Stop `worker` (which archives it), then revive it from the archive.
fn stop_and_revive_archived(fixture: &Fixture) -> crate::subagents::Result<Value> {
    let record = fixture.manager().load("worker").unwrap();
    let stopped = fixture.manager().stop("worker").unwrap();
    assert!(stopped["archive"].is_string(), "{stopped}");
    kill_owned(fixture, &record);
    fixture
        .manager()
        .revive_archived("worker", false, None, TIMEOUT)
}

#[test]
fn a_muse_lane_records_the_session_its_runtime_holds_and_revives_from_the_archive_without_a_bind() {
    let fixture = unreported();
    let id = "01a1ab0f-0a3c-73c1-8056-ee5296bc1574";
    let lock = fake_muse_session(&fixture, id);
    // A sub-agent's session the runtime also holds is not the lane's own.
    let sub = fixture
        .root
        .join("data/muse/sessions/2026/11/06")
        .join(id)
        .join("subagent/child/.session.lock");
    fs::create_dir_all(sub.parent().unwrap()).unwrap();
    fs::write(&sub, "pid=4242\n").unwrap();
    let held = pretend_held(vec![lock, sub, fixture.root.join("data/muse/cron.db")]);
    let status = start_worker(&fixture, "muse");
    assert!(status.get("native_session_warning").is_none(), "{status}");
    let record = fixture.manager().load("worker").unwrap();
    let native = record.native_session.clone().expect("captured at start");
    assert_eq!((native.agent.as_str(), native.value.as_str()), ("muse", id));
    assert_eq!(native.source, "observed");
    assert_eq!(
        record.session_value, None,
        "Herdr reported none; nothing claims it did"
    );
    drop(held);

    let revived = stop_and_revive_archived(&fixture).unwrap();
    assert_eq!(revived["revived"], true, "{revived}");
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(new.arguments, ["resume", id]);
}

#[test]
fn a_second_lane_holding_the_same_session_is_recorded_as_unknown_naming_the_first() {
    // #928: two lanes whose runtimes hold the same session file must
    // not both record it; the second is left unknown, naming the live
    // record that already binds it.
    let fixture = unreported();
    let id = "33333333-3333-3333-3333-333333333333";
    let lock = fake_muse_session(&fixture, id);
    let _held = pretend_held(vec![lock]);
    let first = start_worker(&fixture, "muse");
    assert!(first.get("native_session_warning").is_none(), "{first}");
    let second = fixture
        .manager()
        .start(
            "other",
            &fixture.root,
            StartOptions {
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let warning = second["native_session_warning"]
        .as_str()
        .unwrap_or_default();
    assert!(warning.contains("\"worker\""), "{second}");
    assert!(warning.contains("bind-session other"), "{warning}");
    let record = fixture.manager().load("other").unwrap();
    assert!(record.native_session.is_none(), "duplicate was recorded");
    let unknown = &record.extra["native_session_unknown"];
    assert!(
        unknown["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("\"worker\""),
        "{unknown}"
    );
}

#[test]
fn a_muse_lane_holding_two_sessions_is_recorded_as_unknown_and_says_so() {
    let fixture = unreported();
    let first = fake_muse_session(&fixture, "11111111-1111-1111-1111-111111111111");
    let second = fake_muse_session(&fixture, "22222222-2222-2222-2222-222222222222");
    let _held = pretend_held(vec![first, second]);
    let status = start_worker(&fixture, "muse");
    let warning = status["native_session_warning"]
        .as_str()
        .unwrap_or_default();
    assert!(warning.contains("2 muse sessions"), "{status}");
    assert!(warning.contains("bind-session worker"), "{warning}");
    let record = fixture.manager().load("worker").unwrap();
    assert!(record.native_session.is_none(), "one of two was guessed");
    let note = &record.extra[crate::subagents::revive::NATIVE_SESSION_UNKNOWN];
    assert_eq!(
        note["candidates"],
        json!([
            "11111111-1111-1111-1111-111111111111",
            "22222222-2222-2222-2222-222222222222"
        ])
    );

    let refused = stop_and_revive_archived(&fixture).unwrap_err().to_string();
    assert!(refused.contains("no native conversation ID"), "{refused}");
    assert!(refused.contains("does not guess"), "{refused}");
}

#[test]
fn a_muse_lane_records_the_session_the_store_names_by_pid_at_start() {
    // #948 condition 3: the fd walk holds nothing (the runtime is not a
    // descendant the walk can see), but the store names the session by
    // the pinned pid, so start records it and restore never discovers.
    let fixture = unreported();
    let (store, _store_guard) = pretend_muse_store(&fixture);
    let id = "88888888-8888-8888-8888-888888888888";
    let runtime = store.join("runtime/muse/sessions");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(
        runtime.join(format!("{id}.json")),
        json!({"session_id": id, "process_generation_hint": "pid=4242"}).to_string(),
    )
    .unwrap();
    let _held = pretend_held(Vec::new());
    let status = start_worker(&fixture, "muse");
    assert!(status.get("native_session_missing").is_none(), "{status}");
    let record = fixture.manager().load("worker").unwrap();
    let native = record.native_session.clone().expect("captured at start");
    assert_eq!(native.value, id);
    assert_eq!(native.source, "observed");
}

#[test]
fn doctor_names_a_live_muse_lane_whose_store_match_is_ambiguous() {
    // #948 condition 4: the pre-reboot check. Several store sessions name
    // the lane's cwd, so revive would refuse; doctor says so while the
    // lane can still be bound by hand.
    let fixture = unreported();
    let (store, _store_guard) = pretend_muse_store(&fixture);
    fake_muse_journal(
        &store,
        "99999999-9999-9999-9999-999999999999",
        &fixture.root,
    );
    fake_muse_journal(
        &store,
        "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
        &fixture.root,
    );
    let _held = pretend_held(Vec::new());
    start_worker(&fixture, "muse");
    let report = fixture.manager().doctor(false).unwrap();
    let row = report["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "worker")
        .unwrap_or_else(|| panic!("no doctor row: {report}"));
    let findings: Vec<&str> = row["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding.as_str().unwrap())
        .collect();
    assert!(findings.contains(&"native-session-missing"), "{findings:?}");
    assert!(
        findings.contains(&"native-session-ambiguous"),
        "{findings:?}"
    );
}

#[test]
fn a_muse_lane_with_no_native_id_is_flagged_live_and_revives_through_the_single_store_match() {
    // #948: capture missed this lane entirely, so the record names no
    // session. Status flags it while it lives, and after archival the one
    // store journal naming its cwd is recorded and resumed; none holds.
    let fixture = unreported();
    let (store, _store_guard) = pretend_muse_store(&fixture);
    let id = "44444444-4444-4444-4444-444444444444";
    fake_muse_journal(&store, id, &fixture.root);
    let _held = pretend_held(Vec::new());
    let status = start_worker(&fixture, "muse");
    assert_eq!(status["native_session_missing"], json!(true), "{status}");
    let record = fixture.manager().load("worker").unwrap();
    assert!(record.native_session.is_none(), "capture still missed");

    let revived = stop_and_revive_archived(&fixture).unwrap();
    assert_eq!(revived["revived"], true, "{revived}");
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(new.arguments, ["resume", id]);
    assert_eq!(
        new.native_session
            .as_ref()
            .map(|native| native.value.as_str()),
        Some(id),
        "the store match was recorded"
    );
}

#[test]
fn revive_refuses_zero_and_several_store_matches_with_candidates_listed() {
    // #948: a shared cwd is how a wrong session gets bound, so several
    // store journals naming the record's cwd refuse and name them all.
    let fixture = unreported();
    let (store, _store_guard) = pretend_muse_store(&fixture);
    fake_muse_journal(
        &store,
        "55555555-5555-5555-5555-555555555555",
        &fixture.root,
    );
    fake_muse_journal(
        &store,
        "66666666-6666-6666-6666-666666666666",
        &fixture.root,
    );
    let _held = pretend_held(Vec::new());
    start_worker(&fixture, "muse");
    let refused = stop_and_revive_archived(&fixture).unwrap_err().to_string();
    assert!(refused.contains("no native conversation ID"), "{refused}");
    assert!(
        refused.contains("55555555-5555-5555-5555-555555555555"),
        "{refused}"
    );
    assert!(
        refused.contains("66666666-6666-6666-6666-666666666666"),
        "{refused}"
    );
    assert!(refused.contains("does not guess"), "{refused}");

    // Zero matches keep the plain refusal.
    let fixture = unreported();
    let (store, _store_guard) = pretend_muse_store(&fixture);
    fake_muse_journal(
        &store,
        "77777777-7777-7777-7777-777777777777",
        Path::new("/somewhere/else"),
    );
    let _held = pretend_held(Vec::new());
    start_worker(&fixture, "muse");
    let refused = stop_and_revive_archived(&fixture).unwrap_err().to_string();
    assert!(refused.contains("no native conversation ID"), "{refused}");
    assert!(
        !refused.contains("77777777-7777-7777-7777-777777777777"),
        "{refused}"
    );
}

#[test]
fn a_codex_lane_records_its_main_thread_rollout_and_revives_from_the_archive() {
    let fixture = unreported();
    let sessions = fixture.root.join("codex/sessions/2026/11/06");
    fs::create_dir_all(&sessions).unwrap();
    let thread = "01a19d09-4d93-70c0-b001-2f419851ddf2";
    let main = sessions.join(format!("rollout-2026-11-06T00-00-00-{thread}.jsonl"));
    fs::write(
        &main,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"session_id\":\"{thread}\",\"source\":\"cli\"}}}}\n"
        ),
    )
    .unwrap();
    // A sub-agent thread's rollout, held by the same process, is not the lane's conversation.
    let child =
        sessions.join("rollout-2026-11-06T00-00-09-01a1ab92-3fc4-7b13-b906-ac1289ce1f14.jsonl");
    fs::write(
        &child,
        format!(
            "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"01a1ab92-3fc4-7b13-b906-ac1289ce1f14\",\"session_id\":\"{thread}\",\"parent_thread_id\":\"{thread}\",\"source\":{{\"subagent\":{{}}}}}}}}\n"
        ),
    )
    .unwrap();
    let held = pretend_held(vec![main, child]);
    start_worker(&fixture, "codex");
    let native = fixture
        .manager()
        .load("worker")
        .unwrap()
        .native_session
        .expect("captured at start");
    assert_eq!(
        (native.agent.as_str(), native.value.as_str()),
        ("codex", thread)
    );
    drop(held);

    let revived = stop_and_revive_archived(&fixture).unwrap();
    assert_eq!(revived["revived"], true, "{revived}");
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(new.arguments[..2], ["resume", thread]);
}

#[test]
fn a_claude_lane_carries_the_id_it_was_launched_with_and_revives_from_the_archive() {
    let fixture = unreported();
    start_worker(&fixture, "claude");
    let record = fixture.manager().load("worker").unwrap();
    let native = record.native_session.clone().expect("assigned at launch");
    assert_eq!(native.source, "asserted");
    assert_eq!(record.arguments, ["--session-id", &native.value]);

    let revived = stop_and_revive_archived(&fixture).unwrap();
    assert_eq!(revived["revived"], true, "{revived}");
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(new.arguments, ["--resume", &native.value]);
}

#[test]
fn adopting_a_muse_or_codex_lane_records_the_session_it_holds() {
    for harness in ["muse", "codex"] {
        let fixture = unreported();
        let held = match harness {
            "muse" => fake_muse_session(&fixture, "33333333-3333-3333-3333-333333333333"),
            _ => {
                let dir = fixture.root.join("codex/sessions/2026/11/06");
                fs::create_dir_all(&dir).unwrap();
                let path = dir
                    .join("rollout-2026-11-06T00-00-00-33333333-3333-3333-3333-333333333333.jsonl");
                fs::write(
                    &path,
                    "{\"type\":\"session_meta\",\"payload\":{\"id\":\"33333333-3333-3333-3333-333333333333\",\"source\":\"cli\"}}\n",
                )
                .unwrap();
                path
            }
        };
        let _held = pretend_held(vec![held]);
        fixture.prepare_foreign();
        // The pane runs the harness being adopted.
        fixture
            .client
            .launches
            .lock()
            .unwrap()
            .insert("owned".to_owned(), (harness.to_owned(), Vec::new()));
        let mut options = fixture.adopt_options();
        options.harness = harness.to_owned();
        let adopted = fixture.manager().adopt("foreign", options).unwrap();
        assert!(adopted.get("native_session_warning").is_none(), "{adopted}");
        let native = fixture
            .manager()
            .load("foreign")
            .unwrap()
            .native_session
            .expect("captured at adopt");
        assert_eq!(
            (native.agent.as_str(), native.value.as_str()),
            (harness, "33333333-3333-3333-3333-333333333333")
        );
    }
}

#[test]
fn a_codex_lane_whose_rollout_appears_with_its_brief_records_it_after_delivery() {
    // Codex writes its rollout only once it has a first prompt: nothing is held at launch.
    let fixture = unreported();
    let dir = fixture.root.join("codex/sessions/2026/11/06");
    fs::create_dir_all(&dir).unwrap();
    let thread = "44444444-4444-4444-4444-444444444444";
    let rollout = dir.join(format!("rollout-2026-11-06T00-00-00-{thread}.jsonl"));
    fs::write(
        &rollout,
        format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"source\":\"cli\"}}}}\n"),
    )
    .unwrap();
    let _held = pretend_held(Vec::new());
    *fixture.client.on_run.lock().unwrap() = Some(Box::new(move |_| {
        crate::subagents::revive::HELD_FILES
            .with(|cell| *cell.borrow_mut() = Some(vec![rollout.clone()]));
    }));
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "codex".to_owned(),
                brief: Some("first task".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let native = fixture
        .manager()
        .load("worker")
        .unwrap()
        .native_session
        .expect("captured once the brief was delivered");
    assert_eq!(native.value, thread);
}

// --- Repeated stop and revive (#920 revive-repeat-incomplete) -----------------------------------

/// Start `worker` under `harness` (Herdr reporting its native session), then stop it and
/// revive it from the archive `cycles` times; every cycle must complete.
fn stop_revive_cycles(harness: &str, cycles: usize) -> Fixture {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: harness.to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let first = fixture.manager().load("worker").unwrap();
    let conversation = first.native_session.clone().unwrap().value;
    for cycle in 1..=cycles {
        let record = fixture.manager().load("worker").unwrap();
        let stopped = fixture.manager().stop("worker").unwrap();
        assert!(
            stopped["archive"].is_string(),
            "{harness} cycle {cycle}: {stopped}"
        );
        kill_owned(&fixture, &record);
        let revived = fixture
            .manager()
            .revive_archived("worker", false, None, TIMEOUT)
            .unwrap_or_else(|error| panic!("{harness} cycle {cycle}: {error}"));
        assert_eq!(
            revived["revived"], true,
            "{harness} cycle {cycle}: {revived}"
        );
        let now = fixture.manager().load("worker").unwrap();
        assert_eq!(now.lifecycle, "running", "{harness} cycle {cycle}");
        assert_eq!(
            now.native_session
                .as_ref()
                .map(|native| native.value.as_str()),
            Some(conversation.as_str()),
            "{harness} cycle {cycle} resumed another conversation"
        );
        assert!(
            !fixture
                .root
                .join("registry/.revives")
                .join(format!("{}.json", record.token))
                .exists(),
            "{harness} cycle {cycle} left its revive journal"
        );
    }
    fixture
}

#[test]
fn five_stop_and_revive_cycles_complete_for_every_harness_that_revives() {
    for harness in ["claude", "codex", "muse"] {
        let fixture = stop_revive_cycles(harness, 5);
        let ended = fixture.client.ended.lock().unwrap().len();
        // Only Muse is asked to exit before its pane closes: it records whether it ended cleanly.
        assert_eq!(ended, if harness == "muse" { 5 } else { 0 }, "{harness}");
    }
}

#[test]
fn stop_asks_a_muse_lane_to_exit_before_closing_its_pane() {
    let fixture = Fixture::new();
    fixture
        .client
        .fresh_presentations
        .store(true, Ordering::Relaxed);
    fixture
        .manager()
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "muse".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = fixture.manager().load("worker").unwrap();
    let pinned = record.custom_process_identity.clone().unwrap();
    let stopped = fixture.manager().stop("worker").unwrap();
    assert_eq!(stopped["harness_exited"], true, "{stopped}");
    assert_eq!(*fixture.client.ended.lock().unwrap(), [pinned.pid]);
    assert_eq!(
        *fixture.client.closed.lock().unwrap(),
        [record.pane_id.unwrap()]
    );
}

#[test]
fn an_incomplete_revive_recovers_by_running_the_printed_command_as_printed() {
    for harness in ["claude", "muse"] {
        let (fixture, old) = recoverable(harness, true);
        fixture
            .client
            .fail_ready_once
            .store(true, Ordering::Relaxed);
        let error = fixture
            .manager()
            .revive("worker", false, None, TIMEOUT)
            .unwrap_err()
            .to_string();
        assert!(error.contains("incomplete"), "{harness}: {error}");
        // The printed recovery: a command line in backquotes, run exactly as printed.
        let printed = error
            .split('`')
            .nth(1)
            .unwrap_or_else(|| panic!("{harness}: no command printed in {error}"));
        let words: Vec<&str> = printed.split_whitespace().collect();
        assert_eq!(words[..2], ["agentctl", "revive"], "{harness}: {printed}");
        assert_eq!(words[2], "worker", "{harness}: {printed}");
        let token = words
            .iter()
            .position(|word| *word == "--expected-token")
            .map(|index| words[index + 1]);
        let revived = fixture
            .manager()
            .revive("worker", false, token, TIMEOUT)
            .unwrap_or_else(|error| panic!("{harness}: `{printed}` refused: {error}"));
        assert_eq!(revived["revived"], true, "{harness}: {revived}");
        assert_eq!(revived["previous_token"], old.token, "{harness}");
    }
}

#[test]
fn a_failed_launch_whose_harness_never_came_up_in_its_own_pane_is_abandoned_and_relaunched() {
    // #920: a Codex revive under load opened its candidate pane, but Codex never came up and the
    // launch failed. The pane holds something agentctl cannot call an idle shell, so #899 alone
    // would leave the candidate unproven and refuse; the pane is this revive's own, so the
    // printed rerun closes it, keeps the attempt as evidence, and launches afresh.
    let (fixture, old) = recoverable("codex", true);
    fixture
        .client
        .fail_start_once
        .store(true, Ordering::Relaxed);
    let error = fixture
        .manager()
        .revive("worker", false, None, TIMEOUT)
        .unwrap_err()
        .to_string();
    assert!(error.contains("incomplete"), "{error}");
    let (_, candidate, _) =
        super::super::revive::read_record_at(&stage_path(&fixture, &old), "worker").unwrap();
    assert_eq!(candidate.lifecycle, "launch_failed");
    let pane = candidate
        .pane_id
        .clone()
        .expect("the candidate's pane was opened");
    assert_ne!(Some(&pane), old.pane_id.as_ref());
    fixture
        .client
        .custom_at_idle_shell
        .store(false, Ordering::Relaxed);
    let plan = fixture
        .manager()
        .revive("worker", true, None, TIMEOUT)
        .unwrap();
    assert_eq!(
        plan["action"], "revive",
        "the dry run says what the rerun does: {plan}"
    );
    let result = fixture
        .manager()
        .revive("worker", false, Some(&old.token), TIMEOUT)
        .unwrap_or_else(|error| panic!("the printed rerun refused: {error}"));
    assert_eq!(result["revived"], true, "{result}");
    let evidence = result["abandoned_candidate"]["evidence"]
        .as_str()
        .unwrap_or_default();
    assert!(evidence.contains("never came up"), "{result}");
    assert!(fixture.client.closed.lock().unwrap().contains(&pane));
    let (_, kept) = abandoned_attempt(&fixture, &old);
    assert_eq!(kept["explicit"], false);
    let new = fixture.manager().load("worker").unwrap();
    assert_eq!(new.lifecycle, "running");
    assert_eq!(
        new.native_session.as_ref().map(|native| &native.value),
        old.native_session.as_ref().map(|native| &native.value),
        "the relaunch resumed another conversation"
    );
    assert_ne!(new.pane_id.as_deref(), Some(pane.as_str()));
}
