//! #1340 agent-model-unknown: the model the harness itself recorded is
//! captured once into `resolved_model` and read back; an older record
//! without it still loads.

use super::*;

#[test]
fn capture_reads_the_model_the_harness_recorded_once() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let mut record = manager.load("worker").unwrap();
    assert_eq!(record.model, None);
    assert_eq!(record.resolved_model, None);
    let session = record
        .effective_native_session()
        .map(|native| native.value)
        .or_else(|| record.session_value.clone())
        .expect("a started Claude Code agent is pinned to its conversation");
    let home = fixture.root.join("claude-home");
    TEST_HARNESS_HOME.with(|slot| *slot.borrow_mut() = Some(home.clone()));
    let project: String = fixture
        .root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let directory = home.join(".claude/projects").join(project);
    fs::create_dir_all(&directory).unwrap();
    let transcript = directory.join(format!("{session}.jsonl"));
    fs::write(
        &transcript,
        "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n\
         {\"type\":\"assistant\",\"message\":{\"model\":\"claude-resolved-1\",\"content\":[]}}\n",
    )
    .unwrap();
    record.capture_resolved_model();
    assert_eq!(record.resolved_model.as_deref(), Some("claude-resolved-1"));
    // Once held it is never re-read: a later transcript names another model.
    fs::write(
        &transcript,
        "{\"type\":\"assistant\",\"message\":{\"model\":\"claude-other-2\",\"content\":[]}}\n",
    )
    .unwrap();
    record.capture_resolved_model();
    assert_eq!(record.resolved_model.as_deref(), Some("claude-resolved-1"));
    TEST_HARNESS_HOME.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn a_record_without_resolved_model_still_loads() {
    let fixture = Fixture::new();
    let manager = fixture.manager();
    manager
        .start(
            "worker",
            &fixture.root,
            StartOptions {
                harness: "claude".to_owned(),
                model: Some("claude-declared-1".to_owned()),
                ..StartOptions::default()
            },
        )
        .unwrap();
    let record = manager.load("worker").unwrap();
    assert_eq!(record.model.as_deref(), Some("claude-declared-1"));
    let mut value = serde_json::to_value(&record).unwrap();
    value.as_object_mut().unwrap().remove("resolved_model");
    let older: AgentRecord = serde_json::from_value(value).unwrap();
    assert_eq!(older.resolved_model, None);
    assert_eq!(older.model.as_deref(), Some("claude-declared-1"));
    // And the status row a reader sees names the declared model.
    let status = manager.status("worker").unwrap();
    assert_eq!(status["model"], "claude-declared-1");
}
