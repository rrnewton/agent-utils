use super::*;

const SECRET: &str = "SECRET-123456";

fn seed_history(fixture: &Fixture) -> PathBuf {
    let dir = fixture.root.join("registry/agents/worker");
    let processed = dir.join("queue/processed");
    fs::create_dir_all(&processed).unwrap();
    fs::write(
        processed.join("m1.json"),
        format!(
            r#"{{"text":"the {SECRET} and {SECRET} again","count":42,"{SECRET}":"key stays"}}"#
        ),
    )
    .unwrap();
    let events = dir.join("events");
    fs::create_dir_all(&events).unwrap();
    fs::write(
        events.join("e.jsonl"),
        format!("{{\"note\":\"a {SECRET} here\"}}\nnot json {SECRET}\n"),
    )
    .unwrap();
    dir
}

fn stop_worker(fixture: &Fixture) {
    let mut record = fixture.manager().load("worker").unwrap();
    record.lifecycle = "stopped".to_owned();
    fixture.manager().save(&record).unwrap();
}

#[test]
fn redact_rewrites_only_string_values_with_backups_and_audit() {
    let fixture = Fixture::new();
    fixture.start(None);
    seed_history(&fixture);
    stop_worker(&fixture);
    let dry = fixture
        .manager()
        .redact("worker", SECRET, false, true, false)
        .unwrap();
    assert_eq!(dry["total_matches"], 3);
    assert_eq!(dry["left_behind"]["unparseable_lines"], 1);
    assert!(!fixture.root.join("registry/backups").exists());
    let result = fixture
        .manager()
        .redact("worker", SECRET, false, false, false)
        .unwrap();
    assert_eq!(result["total_matches"], 3);
    assert_eq!(result["clean"], false);
    let processed = fs::read_to_string(
        fixture
            .root
            .join("registry/agents/worker/queue/processed/m1.json"),
    )
    .unwrap();
    assert!(
        processed.contains("[redacted] and [redacted] again"),
        "{processed}"
    );
    let document: Value = serde_json::from_str(&processed).unwrap();
    assert_eq!(document["count"], 42);
    assert_eq!(document[SECRET], "key stays");
    // The key itself is reported left behind, not rewritten.
    assert_eq!(result["left_behind"]["keys"], 1);
    let audit =
        fs::read_to_string(fixture.root.join("registry/backups/redact-audit.jsonl")).unwrap();
    assert!(
        !audit.contains(SECRET),
        "audit must not hold the literal: {audit}"
    );
    assert_eq!(audit.lines().count(), 2, "started then completed: {audit}");
    let backup_dir = fixture
        .root
        .join("registry")
        .join(result["backup"].as_str().unwrap());
    assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 2);
}

#[test]
fn redact_refuses_events_while_running_and_short_literals() {
    let fixture = Fixture::new();
    fixture.start(None);
    seed_history(&fixture);
    // Running agent: the events rewrite is refused (blocker 1).
    assert!(fixture
        .manager()
        .redact("worker", SECRET, false, false, false)
        .is_err());
    assert!(fixture
        .manager()
        .redact("worker", "short", false, true, false)
        .is_err());
    assert!(fixture
        .manager()
        .redact("worker", "running", false, true, false)
        .is_err());
}

#[test]
fn redact_refuses_mid_delivery_and_skips_archives_by_default() {
    let fixture = Fixture::new();
    fixture.start(None);
    let dir = seed_history(&fixture);
    stop_worker(&fixture);
    let archived = fixture
        .root
        .join("registry/archive/worker-1795-1234/queue/processed");
    fs::create_dir_all(&archived).unwrap();
    fs::write(
        archived.join("a.json"),
        format!(r#"{{"text":"{SECRET} archived"}}"#),
    )
    .unwrap();
    let result = fixture
        .manager()
        .redact("worker", SECRET, false, false, false)
        .unwrap();
    assert_eq!(result["total_matches"], 3);
    assert!(fs::read_to_string(archived.join("a.json"))
        .unwrap()
        .contains(SECRET));
    let included = fixture
        .manager()
        .redact("worker", SECRET, true, false, false)
        .unwrap();
    assert_eq!(included["total_matches"], 1);
    let inflight = dir.join("queue/inflight");
    fs::create_dir_all(&inflight).unwrap();
    fs::write(inflight.join("x.json"), "{}").unwrap();
    assert!(fixture
        .manager()
        .redact("worker", SECRET, true, false, false)
        .is_err());
}

#[test]
fn redact_supports_a_retired_agent() {
    let fixture = Fixture::new();
    fixture.start(None);
    seed_history(&fixture);
    stop_worker(&fixture);
    // Retire by hand: move the agent directory into the archive.
    let archive = fixture.root.join("registry/archive/worker-1795-1234");
    fs::create_dir_all(archive.parent().unwrap()).unwrap();
    fs::rename(fixture.root.join("registry/agents/worker"), &archive).unwrap();
    let result = fixture
        .manager()
        .redact("worker", SECRET, false, false, false)
        .unwrap();
    assert_eq!(result["total_matches"], 3);
}

#[test]
fn redact_preserves_file_modes() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.start(None);
    let dir = seed_history(&fixture);
    stop_worker(&fixture);
    let events = dir.join("events/e.jsonl");
    let processed = dir.join("queue/processed/m1.json");
    fs::set_permissions(&events, fs::Permissions::from_mode(0o600)).unwrap();
    fs::set_permissions(&processed, fs::Permissions::from_mode(0o640)).unwrap();
    fixture
        .manager()
        .redact("worker", SECRET, false, false, false)
        .unwrap();
    assert_eq!(
        fs::metadata(&events).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&processed).unwrap().permissions().mode() & 0o777,
        0o640
    );
}
