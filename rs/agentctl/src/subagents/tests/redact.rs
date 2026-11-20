use super::*;

fn seed_history(fixture: &Fixture) -> PathBuf {
    let dir = fixture.root.join("registry/agents/worker");
    let processed = dir.join("queue/processed");
    fs::create_dir_all(&processed).unwrap();
    fs::write(
        processed.join("m1.json"),
        r#"{"text":"the SECRET and SECRET again","count":42,"SECRET":"key name stays"}"#,
    )
    .unwrap();
    let events = dir.join("events");
    fs::create_dir_all(&events).unwrap();
    fs::write(
        events.join("e.jsonl"),
        "{\"note\":\"a SECRET here\"}\nnot json SECRET\n",
    )
    .unwrap();
    dir
}

#[test]
fn redact_rewrites_only_string_values_with_backups_and_audit() {
    let fixture = Fixture::new();
    fixture.start(None);
    seed_history(&fixture);
    let dry = fixture
        .manager()
        .redact("worker", "SECRET", false, true)
        .unwrap();
    assert_eq!(dry["total_matches"], 3);
    // Dry run wrote nothing.
    assert!(!fixture.root.join("registry/backups").exists());
    let result = fixture
        .manager()
        .redact("worker", "SECRET", false, false)
        .unwrap();
    assert_eq!(result["total_matches"], 3);
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
    assert!(processed.contains("\"count\":42"), "{processed}");
    assert!(
        processed.contains("\"SECRET\":\"key name stays\""),
        "{processed}"
    );
    let audit =
        fs::read_to_string(fixture.root.join("registry/backups/redact-audit.jsonl")).unwrap();
    assert!(
        !audit.contains("SECRET"),
        "audit must not hold the literal: {audit}"
    );
    assert!(audit.contains("\"total_matches\":3"), "{audit}");
    let backup_dir = fixture
        .root
        .join("registry")
        .join(result["backup"].as_str().unwrap());
    assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 2);
}

#[test]
fn redact_refuses_mid_delivery_and_skips_archives_by_default() {
    let fixture = Fixture::new();
    fixture.start(None);
    let dir = seed_history(&fixture);
    // Archived copy is untouched without --include-archived.
    let archived = fixture
        .root
        .join("registry/archive/worker-tok/queue/processed");
    fs::create_dir_all(&archived).unwrap();
    fs::write(archived.join("a.json"), r#"{"text":"SECRET archived"}"#).unwrap();
    let result = fixture
        .manager()
        .redact("worker", "SECRET", false, false)
        .unwrap();
    assert_eq!(result["total_matches"], 3);
    assert!(fs::read_to_string(archived.join("a.json"))
        .unwrap()
        .contains("SECRET"));
    let included = fixture
        .manager()
        .redact("worker", "SECRET", true, false)
        .unwrap();
    assert_eq!(included["total_matches"], 1);
    // Mid-delivery (inflight entry) refuses.
    let inflight = dir.join("queue/inflight");
    fs::create_dir_all(&inflight).unwrap();
    fs::write(inflight.join("x.json"), "{}").unwrap();
    assert!(fixture
        .manager()
        .redact("worker", "SECRET", true, false)
        .is_err());
}
