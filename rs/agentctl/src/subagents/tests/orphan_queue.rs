use super::*;

const TIMEOUT: Duration = Duration::from_secs(30);

fn registry(fixture: &Fixture) -> PathBuf {
    fixture.root.join("registry")
}

/// Leave `agent_name` as a queue with no record, the way a team delivery addressed to a retired
/// agent does: the deliverer prepares the layout and enqueues into `inbox.v2/` by path.
fn orphan_queue(fixture: &Fixture, agent_name: &str) -> PathBuf {
    let registry = registry(fixture);
    agent_queue::registry::prepare(&registry).unwrap();
    let directory = agent_queue::registry::directory(&registry, agent_name).unwrap();
    agent_queue::queue::enqueue_into(
        &directory.join("queue"),
        agent_queue::queue::Lane::InboxV2,
        Some("tm-team-0001"),
        false,
        &agent_queue::fsio::Blocking,
        |identifier| agent_queue::queue::plain_item(identifier, "late team message"),
    )
    .unwrap();
    assert!(!directory.join("agent.json").exists());
    directory
}

fn queued_item(directory: &Path) -> PathBuf {
    directory.join("queue/inbox.v2/tm-team-0001.json")
}

/// Write `bytes` as `agent_name`'s `agent.json` in a fresh private directory.
fn raw_record(fixture: &Fixture, agent_name: &str, bytes: &[u8], mode: u32) -> PathBuf {
    let registry = registry(fixture);
    agent_queue::registry::prepare(&registry).unwrap();
    let directory = agent_queue::registry::directory(&registry, agent_name).unwrap();
    DirBuilder::new().mode(0o700).create(&directory).unwrap();
    let path = directory.join("agent.json");
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
    path
}

fn start_named(fixture: &Fixture, agent_name: &str, resume: Option<&str>) -> Result<Value> {
    fixture.manager().start(
        agent_name,
        &fixture.root,
        StartOptions {
            workspace_id: Some("workspace".to_owned()),
            harness: "codex".to_owned(),
            resume: resume.map(str::to_owned),
            ..StartOptions::default()
        },
    )
}

fn findings(report: &Value, agent_name: &str) -> Vec<String> {
    report["records"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["name"] == agent_name)
        .flat_map(|row| row["findings"].as_array().unwrap().clone())
        .map(|finding| finding.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn start_succeeds_beside_a_directory_that_holds_only_a_queue() {
    let fixture = Fixture::new();
    let orphan = orphan_queue(&fixture, "retired");
    // The launched harness reports its native session, so start checks that no other record
    // owns it: that registry-wide scan must pass over a directory with no record.
    let started = start_named(&fixture, "worker", None).unwrap();
    assert_eq!(started["name"], "worker");
    assert!(started["session_value"].is_string(), "{started}");
    // The orphan is left exactly as found: nothing adopts, moves, or deletes its queue.
    assert!(queued_item(&orphan).is_file());
    assert!(!orphan.join("agent.json").exists());
}

#[test]
fn start_with_a_saved_conversation_succeeds_beside_a_queue_only_directory() {
    let fixture = Fixture::new();
    let orphan = orphan_queue(&fixture, "retired");
    let started = start_named(&fixture, "worker", Some("saved-conversation")).unwrap();
    assert_eq!(started["name"], "worker");
    assert!(queued_item(&orphan).is_file());
}

#[test]
fn a_queue_only_directory_owns_no_session_but_a_real_owner_still_refuses() {
    let fixture = Fixture::new();
    fixture.start(None);
    let session = fixture.manager().get("worker").unwrap()["session_value"]
        .as_str()
        .unwrap()
        .to_owned();
    orphan_queue(&fixture, "retired");
    let error = start_named(&fixture, "second", Some(&session)).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("native session is already registered as \"worker\""),
        "{error}"
    );
}

#[test]
fn start_refuses_the_name_of_a_queue_only_directory_and_keeps_its_queue() {
    let fixture = Fixture::new();
    let orphan = orphan_queue(&fixture, "retired");
    let error = start_named(&fixture, "retired", None).unwrap_err();
    assert!(error.to_string().contains("already registered"), "{error}");
    assert!(queued_item(&orphan).is_file());
    assert!(!orphan.join("agent.json").exists());
}

#[test]
fn start_fails_closed_on_a_record_that_exists_but_cannot_be_trusted() {
    for (label, bytes, mode) in [
        ("invalid JSON", b"{not json".as_slice(), 0o600),
        ("not private", b"{}".as_slice(), 0o644),
        ("empty", b"".as_slice(), 0o600),
    ] {
        let fixture = Fixture::new();
        orphan_queue(&fixture, "retired");
        raw_record(&fixture, "broken", bytes, mode);
        let error = start_named(&fixture, "worker", None).unwrap_err();
        assert!(
            error.to_string().contains("broken"),
            "{label}: an untrusted record must stop the scan: {error}"
        );
    }
}

#[test]
fn start_fails_closed_on_a_dangling_record_link() {
    let fixture = Fixture::new();
    let registry = registry(&fixture);
    agent_queue::registry::prepare(&registry).unwrap();
    let directory = agent_queue::registry::directory(&registry, "linked").unwrap();
    DirBuilder::new().mode(0o700).create(&directory).unwrap();
    std::os::unix::fs::symlink(
        fixture.root.join("absent.json"),
        directory.join("agent.json"),
    )
    .unwrap();
    let error = start_named(&fixture, "worker", None).unwrap_err();
    assert!(error.to_string().contains("linked"), "{error}");
}

#[test]
fn adopt_and_revive_all_pass_over_a_queue_only_directory() {
    let fixture = Fixture::new();
    orphan_queue(&fixture, "retired");
    let adopted = fixture.adopt();
    assert_eq!(adopted["name"], "foreign");
    let result = fixture.manager().revive_all(true, TIMEOUT).unwrap();
    let names: Vec<&str> = result["agents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["name"].as_str())
        .collect();
    assert!(!names.contains(&"retired"), "{result}");
}

#[test]
fn doctor_reports_a_queue_only_directory_as_record_absent() {
    let fixture = Fixture::new();
    orphan_queue(&fixture, "retired");
    raw_record(&fixture, "broken", b"{not json", 0o600);
    let report = fixture.manager().doctor(false).unwrap();
    assert_eq!(findings(&report, "retired"), ["record-absent"], "{report}");
    assert_eq!(
        findings(&report, "broken"),
        ["record-unreadable"],
        "{report}"
    );
    assert_eq!(report["clean"], false);
}
