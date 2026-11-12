//! Conformance checks over the frozen `ci-hub-store-wire/v1` transcripts.
//!
//! M1 ships the contract document plus these synthetic transcripts; M2
//! implements the connector against the same fixtures. This test keeps the
//! transcripts well-formed: sequential request ids, frozen refusal
//! vocabulary, complete hello limits, and legal fetch phasing.

use std::collections::BTreeSet;
use std::path::PathBuf;

const REFUSAL_CLASSES: [&str; 11] = [
    "unavailable",
    "auth",
    "rate-limited",
    "bad-request",
    "version-unsupported",
    "conflicting-event-id",
    "cursor-invalid",
    "store-rewrite",
    "object-missing",
    "object-too-large",
    "object-digest-mismatch",
];

const LIMIT_KEYS: [&str; 7] = [
    "max_events_per_append",
    "max_append_bytes",
    "max_events_per_read",
    "max_read_bytes",
    "max_line_bytes",
    "max_event_line_bytes",
    "max_object_bytes",
];

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire")
}

fn check_class(value: &serde_json::Value, where_: &str) {
    if let Some(class) = value.get("class").and_then(|c| c.as_str()) {
        assert!(
            REFUSAL_CLASSES.contains(&class),
            "{where_}: unknown refusal class {class}"
        );
    }
    if let Some(acks) = value.get("acks").and_then(|a| a.as_array()) {
        for ack in acks {
            let status = ack.get("status").and_then(|s| s.as_str()).unwrap_or("");
            assert!(
                ["stored", "duplicate", "refused"].contains(&status),
                "{where_}: unknown ack status {status}"
            );
            check_class(ack, where_);
            if status == "refused" {
                assert!(
                    ack.get("class").is_some(),
                    "{where_}: refused ack without class"
                );
            }
        }
    }
}

#[test]
fn wire_transcripts_are_well_formed() {
    let dir = fixture_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("wire fixtures directory exists")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    assert!(
        files.len() >= 10,
        "expected the full transcript set, found {}",
        files.len()
    );

    let expected: BTreeSet<&str> = [
        "hello.jsonl",
        "hello-version-refusal.jsonl",
        "append.jsonl",
        "append-partial.jsonl",
        "append-overlong.jsonl",
        "head-read.jsonl",
        "read-byte-budget.jsonl",
        "read-cursor-reset.jsonl",
        "read-store-rewrite.jsonl",
        "fetch-ok.jsonl",
        "fetch-abort.jsonl",
        "fetch-errors.jsonl",
        "refusal-isolation.jsonl",
    ]
    .into_iter()
    .collect();
    let found: BTreeSet<String> = files
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for name in &expected {
        assert!(found.contains(*name), "missing transcript {name}");
    }

    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).expect("transcript readable");
        let mut open_request: Option<String> = None;
        let mut fetch_phase: Option<String> = None;
        for (index, line) in text.lines().enumerate() {
            let where_ = format!("{name}:{}", index + 1);
            let value: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{where_}: invalid JSON: {e}"));
            let dir = value.get("dir").and_then(|d| d.as_str()).unwrap_or("");
            match dir {
                "note" => {
                    assert!(value.get("text").is_some(), "{where_}: note without text");
                }
                "req" => {
                    assert!(
                        open_request.is_none(),
                        "{where_}: second request while one is unacked (strictly sequential)"
                    );
                    let id = value
                        .get("id")
                        .and_then(|i| i.as_str())
                        .expect("request id");
                    open_request = Some(id.to_owned());
                    let op = value.get("op").and_then(|o| o.as_str()).unwrap_or("");
                    assert!(
                        ["hello", "append", "head", "read_from", "fetch"].contains(&op),
                        "{where_}: unknown op {op}"
                    );
                    if op == "fetch" {
                        fetch_phase = Some("await-begin".to_owned());
                    }
                }
                "res" => {
                    let id = value
                        .get("id")
                        .and_then(|i| i.as_str())
                        .expect("response id");
                    assert_eq!(
                        open_request.as_deref(),
                        Some(id),
                        "{where_}: response id does not match the open request"
                    );
                    check_class(&value, &where_);
                    if let Some(limits) = value.get("limits") {
                        for key in LIMIT_KEYS {
                            assert!(
                                limits.get(key).is_some(),
                                "{where_}: hello limits missing {key}"
                            );
                        }
                    }
                    match value.get("phase").and_then(|p| p.as_str()) {
                        Some("begin") => {
                            assert_eq!(
                                fetch_phase.as_deref(),
                                Some("await-begin"),
                                "{where_}: stray begin"
                            );
                            assert_eq!(
                                value.get("ok"),
                                Some(&serde_json::Value::Bool(true)),
                                "{where_}: begin carries ok:true"
                            );
                            fetch_phase = Some("streaming".to_owned());
                        }
                        Some("chunk") => {
                            assert_eq!(
                                fetch_phase.as_deref(),
                                Some("streaming"),
                                "{where_}: chunk outside stream"
                            );
                            assert!(
                                value.get("ok").is_none(),
                                "{where_}: chunk lines carry no ok field"
                            );
                        }
                        Some("end") | Some("abort") => {
                            assert_eq!(
                                fetch_phase.as_deref(),
                                Some("streaming"),
                                "{where_}: end/abort outside stream"
                            );
                            assert!(
                                value.get("ok").is_none(),
                                "{where_}: end/abort lines carry no ok field"
                            );
                            fetch_phase = None;
                            open_request = None;
                        }
                        Some(other) => panic!("{where_}: unknown fetch phase {other}"),
                        None => {
                            assert!(
                                fetch_phase.is_none()
                                    || value.get("ok") == Some(&serde_json::Value::Bool(false)),
                                "{where_}: single-line response inside a fetch stream"
                            );
                            fetch_phase = None;
                            open_request = None;
                        }
                    }
                }
                other => panic!("{where_}: unknown dir {other:?}"),
            }
        }
        assert!(
            open_request.is_none(),
            "{name}: transcript ends with an unacked request"
        );
    }
}
