use super::*;
use crate::subagents::control::{harness_repo_origins, host_index_path};
use agent_queue::identity::{Ancestor, Ancestry};

const FAKE_BOOT: &str = "22222222-3333-4444-5555-666666666666";

fn registry(fixture: &Fixture) -> PathBuf {
    fixture.root.join("registry")
}

fn record(fixture: &Fixture, agent_name: &str) -> Value {
    agent::read_private_json(&registry(fixture).join(agent_name).join("agent.json")).unwrap()
}

/// A registered agent `agent_name` whose pinned harness is process `pid`, copied from a worker
/// started in a scratch fixture, so a test can make it an ancestor of the caller.
fn register(fixture: &Fixture, agent_name: &str, pid: u64) -> String {
    let scratch = Fixture::new();
    scratch.start(None);
    let mut document = record(&scratch, "worker");
    let token = format!("{agent_name}-generation");
    document["name"] = json!(agent_name);
    document["token"] = json!(token);
    document["pane_id"] = json!(format!("{agent_name}-pane"));
    document["tab_id"] = json!(format!("{agent_name}-tab"));
    document["terminal_id"] = Value::Null;
    document["native_session"] = Value::Null;
    document["session_value"] = Value::Null;
    document["harness_identity"] = json!(Fake::harness(pid));
    for key in [
        crate::subagents::control::CREATED_BY,
        crate::subagents::control::MANAGED_BY,
    ] {
        document.as_object_mut().unwrap().remove(key);
    }
    let directory = registry(fixture).join(agent_name);
    agent::create_private_directory(&registry(fixture), "agent registry", true, true).unwrap();
    DirBuilder::new().mode(0o700).create(&directory).unwrap();
    agent::atomic_json(&directory.join("agent.json"), &document).unwrap();
    token
}

/// A caller whose ancestors are this test process and then the harness processes `pids`.
fn called_from(pids: &[u64]) -> Option<Option<Ancestry>> {
    let mut links = vec![Ancestor {
        pid: 999_999,
        starttime_ticks: 999_999,
    }];
    links.extend(pids.iter().map(|pid| Ancestor {
        pid: *pid,
        starttime_ticks: *pid,
    }));
    Some(Some(Ancestry {
        boot_id: FAKE_BOOT.to_owned(),
        links,
    }))
}

fn canonical_registry(fixture: &Fixture) -> String {
    fs::canonicalize(registry(fixture))
        .unwrap()
        .display()
        .to_string()
}

#[test]
fn start_records_the_calling_agent_as_creator_and_manager_unless_told_not_to() {
    let fixture = Fixture::new();
    let token = register(&fixture, "lieutenant", 77);
    let mut manager = fixture.manager();
    manager.caller_ancestry = called_from(&[77]);
    let started = manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let expected = json!({
        "agent": "lieutenant",
        "registry": canonical_registry(&fixture),
        "token": token,
        "proof": "harness-ancestry",
    });
    let document = record(&fixture, "worker");
    assert_eq!(document["created_by"], expected);
    assert_eq!(document["managed_by"], expected);
    assert_eq!(
        started["managed_by"], expected,
        "status shows the edge: {started}"
    );

    // No proved caller, or --no-edges, records nothing.
    let quiet = Fixture::new();
    register(&quiet, "lieutenant", 77);
    let mut manager = quiet.manager();
    manager.caller_ancestry = called_from(&[77]);
    manager
        .start(
            "worker",
            &quiet.root,
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                record_edges: false,
                ..StartOptions::default()
            },
        )
        .unwrap();
    let document = record(&quiet, "worker");
    assert!(document.get("created_by").is_none() && document.get("managed_by").is_none());
    let human = Fixture::new();
    register(&human, "lieutenant", 77);
    human.start(None);
    assert!(record(&human, "worker").get("managed_by").is_none());
}

#[test]
fn adopt_records_only_the_adopting_agent_as_manager() {
    let fixture = Fixture::new();
    let token = register(&fixture, "coordinator", 88);
    fixture
        .client
        .panes
        .lock()
        .unwrap()
        .push(Fake::pane("owned"));
    fixture.client.started.store(true, Ordering::Relaxed);
    let mut manager = fixture.manager();
    manager.caller_ancestry = called_from(&[88]);
    manager.adopt("foreign", fixture.adopt_options()).unwrap();
    let document = record(&fixture, "foreign");
    assert_eq!(document["managed_by"]["agent"], "coordinator");
    assert_eq!(document["managed_by"]["token"], json!(token));
    assert!(document.get("created_by").is_none());
}

#[test]
fn the_designation_names_one_generation_and_reports_what_became_of_it() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    assert_eq!(manager.coordinator_show().unwrap()["designated"], false);
    assert!(manager.coordinator_set("nobody").is_err());
    let token = register(&fixture, "coordinator", 88);
    let shown = manager.coordinator_set("coordinator").unwrap();
    assert_eq!(shown["designated"], true);
    assert_eq!(shown["name"], "coordinator");
    assert_eq!(shown["token"], json!(token));
    assert_eq!(shown["state"], "running");
    // The fixture's harness belongs to another boot, so no process ID is offered for it.
    assert_eq!(shown["harness_pid"], Value::Null);
    let file = agent::read_private_json(&registry(&fixture).join("coordinator.json")).unwrap();
    assert_eq!(file["schema"], "agentctl-coordinator/v1");
    assert_eq!(
        manager.designated_coordinator().unwrap().as_deref(),
        Some("coordinator")
    );

    // The same name with another token is another agent.
    let mut document = record(&fixture, "coordinator");
    document["token"] = json!("someone-else");
    agent::atomic_json(
        &registry(&fixture).join("coordinator/agent.json"),
        &document,
    )
    .unwrap();
    assert_eq!(manager.coordinator_show().unwrap()["state"], "replaced");
    assert_eq!(manager.designated_coordinator().unwrap(), None);

    // Stopped: its record is archived under NAME-TOKEN.
    fs::rename(
        registry(&fixture).join("coordinator"),
        registry(&fixture).join("stale-copy"),
    )
    .unwrap();
    assert_eq!(manager.coordinator_show().unwrap()["state"], "missing");
    let archive = registry(&fixture).join("archive");
    agent::create_private_directory(&archive, "archive", false, false).unwrap();
    DirBuilder::new()
        .mode(0o700)
        .create(archive.join(format!("coordinator-{token}")))
        .unwrap();
    assert_eq!(
        manager.coordinator_show().unwrap()["state"],
        "coordinator-stopped"
    );

    let cleared = manager.coordinator_clear().unwrap();
    assert_eq!(cleared["cleared"], true);
    assert_eq!(cleared["previous"]["name"], "coordinator");
    assert_eq!(manager.coordinator_clear().unwrap()["cleared"], false);
    assert!(!registry(&fixture).join("coordinator.json").exists());
}

#[test]
fn the_designation_follows_a_revived_generation_through_its_former_tokens() {
    let fixture = Fixture::new();
    let token = register(&fixture, "coordinator", 88);
    let manager = fixture.manager();
    manager.coordinator_set("coordinator").unwrap();
    let mut document = record(&fixture, "coordinator");
    document["token"] = json!("coordinator-revived");
    document["former_tokens"] = json!([token]);
    agent::atomic_json(
        &registry(&fixture).join("coordinator/agent.json"),
        &document,
    )
    .unwrap();
    let shown = manager.coordinator_show().unwrap();
    assert_eq!(shown["state"], "running");
    assert_eq!(shown["token"], "coordinator-revived");
    assert_eq!(shown["designated_token"], json!(token));
}

#[test]
fn manage_moves_control_and_refuses_itself_cycles_and_an_unproved_caller() {
    let fixture = Fixture::new();
    let coordinator = register(&fixture, "coordinator", 88);
    let lieutenant = register(&fixture, "lieutenant", 77);
    register(&fixture, "worker", 66);
    let mut manager = fixture.manager();

    let error = manager.manage("worker", ManageAction::Caller).unwrap_err();
    assert!(error.to_string().contains("--by NAME"), "{error}");
    manager.caller_ancestry = called_from(&[77, 88]);
    let changed = manager.manage("worker", ManageAction::Caller).unwrap();
    assert_eq!(
        changed["managed_by"]["agent"], "lieutenant",
        "the nearest ancestor wins"
    );
    assert_eq!(changed["managed_by"]["token"], json!(lieutenant));
    assert_eq!(changed["previous"], Value::Null);

    let changed = manager
        .manage("lieutenant", ManageAction::By("coordinator".to_owned()))
        .unwrap();
    assert_eq!(changed["managed_by"]["proof"], "assigned");
    assert_eq!(changed["managed_by"]["token"], json!(coordinator));

    let error = manager
        .manage("coordinator", ManageAction::By("worker".to_owned()))
        .unwrap_err();
    assert!(error.to_string().contains("cycle"), "{error}");
    let error = manager
        .manage("worker", ManageAction::By("worker".to_owned()))
        .unwrap_err();
    assert!(error.to_string().contains("itself"), "{error}");
    assert!(manager
        .manage("worker", ManageAction::By("nobody".to_owned()))
        .is_err());

    let cleared = manager.manage("worker", ManageAction::Clear).unwrap();
    assert_eq!(cleared["managed_by"], Value::Null);
    assert_eq!(cleared["previous"]["agent"], "lieutenant");
    assert!(record(&fixture, "worker").get("managed_by").is_none());
}

#[test]
fn the_tree_hangs_every_agent_under_its_manager_and_the_rest_under_the_coordinator() {
    let fixture = Fixture::new();
    register(&fixture, "coordinator", 88);
    register(&fixture, "lieutenant", 77);
    register(&fixture, "worker", 66);
    register(&fixture, "loner", 55);
    let manager = fixture.manager();
    manager.coordinator_set("coordinator").unwrap();
    manager
        .manage("lieutenant", ManageAction::By("coordinator".to_owned()))
        .unwrap();
    manager
        .manage("worker", ManageAction::By("lieutenant".to_owned()))
        .unwrap();
    let tree = manager.tree().unwrap();
    assert_eq!(tree["coordinator"], "coordinator");
    assert_eq!(tree["agents"], 4);
    let roots = tree["roots"].as_array().unwrap();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0]["name"], "coordinator");
    assert_eq!(roots[0]["control"], "designated-coordinator");
    let children: Vec<&str> = roots[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| child["name"].as_str().unwrap())
        .collect();
    assert_eq!(children, ["lieutenant", "loner"]);
    let lieutenant = &roots[0]["children"][0];
    assert_eq!(lieutenant["children"][0]["name"], "worker");
    assert_eq!(lieutenant["children"][0]["harness_pid"], Value::Null);
    assert_eq!(roots[0]["children"][1]["control"], "unmanaged");
    let text = render_tree(&tree);
    assert_eq!(
        text,
        "coordinator (designated coordinator; running)\n\
         ├── lieutenant (running)\n\
         │   └── worker (running)\n\
         └── loner (running; no manager recorded)\n"
    );

    // A manager that is gone leaves its worker under the coordinator, said as such.
    fs::rename(
        registry(&fixture).join("lieutenant"),
        registry(&fixture).join("gone"),
    )
    .unwrap();
    let mut gone = record(&fixture, "gone");
    gone["name"] = json!("gone");
    gone["token"] = json!("gone-generation");
    agent::atomic_json(&registry(&fixture).join("gone/agent.json"), &gone).unwrap();
    let text = render_tree(&manager.tree().unwrap());
    assert!(
        text.contains("worker (running; managed by lieutenant outside this registry or stopped)"),
        "{text}"
    );
    manager.coordinator_clear().unwrap();
    assert!(render_tree(&manager.tree().unwrap()).starts_with("(no coordinator designated"));
}

#[test]
fn restore_follows_the_designation_and_keeps_its_own_coordinator_on_a_conflict() {
    use crate::restore::RestoreConfig;
    let fixture = Fixture::new();
    agent::create_private_directory(&registry(&fixture), "agent registry", true, true).unwrap();
    let mut config = RestoreConfig::load(&registry(&fixture)).unwrap();
    config
        .follow_designation(Some("coordinator".to_owned()))
        .unwrap();
    assert_eq!(config.coordinator.as_deref(), Some("coordinator"));
    config.follow_designation(None).unwrap();
    assert_eq!(config.coordinator.as_deref(), Some("coordinator"));
    let error = config
        .follow_designation(Some("other".to_owned()))
        .unwrap_err();
    assert!(error.to_string().contains("disagree") || error.to_string().contains("designates"));
    assert_eq!(
        config.coordinator.as_deref(),
        Some("coordinator"),
        "restore.json wins"
    );
    let mut excluding = RestoreConfig::load(&registry(&fixture)).unwrap();
    excluding.exclude = vec!["stale".to_owned()];
    assert!(excluding
        .follow_designation(Some("stale".to_owned()))
        .is_err());
    assert_eq!(
        excluding.coordinator, None,
        "an excluded designation is not taken"
    );
}

#[test]
fn host_index_names_one_file_per_registry() {
    let dir = PathBuf::from("/tmp/index");
    let a = host_index_path(&dir, "/home/u/work/one/.agentctl");
    let b = host_index_path(&dir, "/home/u/work/two/.agentctl");
    assert_ne!(a, b);
    assert_eq!(
        a,
        host_index_path(&dir, "/home/u/work/one/.agentctl"),
        "same registry, same file"
    );
    assert!(a.extension().is_some_and(|ext| ext == "json"));
}

#[test]
fn harness_repo_origins_reads_dotgit_configs() {
    let root = std::env::temp_dir().join(format!(
        "agentctl-origins-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    let checkout = root.join("agent-utils");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    fs::write(
        checkout.join(".git/config"),
        "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = https://example.invalid/org/repo\n[remote \"upstream\"]\n\turl = https://example.invalid/other/repo\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("plain")).unwrap();
    assert_eq!(
        harness_repo_origins(&root),
        vec!["https://example.invalid/org/repo".to_string()]
    );
    let _ = fs::remove_dir_all(&root);
}
