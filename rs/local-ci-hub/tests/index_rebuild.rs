//! Rebuild-from-authority test: an incrementally grown index, one killed
//! mid-build and resumed, and a cold rebuild must agree dump-for-dump and
//! answer-for-answer. Synthetic fixtures only.

use local_ci_hub::digest::sha256_of;
use local_ci_hub::event::parse_line;
use local_ci_hub::index::{Cursor, Index, ShardPos};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn result_line(event_id: &str, run_id: &str, emitted: &str, commit: &str, result: &str) -> String {
    format!(
        "{{\"schema\":\"validate-ledger/v1\",\"event_id\":\"{event_id}\",\"event_type\":\"run.result\",\
         \"emitted_at\":\"{emitted}\",\"team\":\"team-a\",\"host\":\"host-1\",\"run_id\":\"{run_id}\",\
         \"producer\":{{\"source\":\"observed\",\"tool\":\"fixture\",\"tool_version\":\"1\"}},\
         \"commit\":\"{commit}\",\"outcome\":\"pass\",\
         \"legacy_row\":{{\"run_id\":\"{run_id}\",\"commit\":\"{commit}\",\"result\":\"{result}\",\
         \"host\":\"host-1\",\"finished_at\":\"{emitted}\"}}}}"
    )
}

fn fixture_events(count: usize) -> Vec<local_ci_hub::event::RawEvent> {
    (0..count)
        .map(|i| {
            let commit = format!("{:040x}", i % 3);
            let line = result_line(
                &format!("evt-{i:03}"),
                &format!("run-{i:03}"),
                &format!("2026-11-{:02}T00:00:00Z", (i % 28) + 1),
                &commit,
                if i % 4 == 0 { "fail" } else { "pass" },
            );
            parse_line(&line).expect("fixture parses")
        })
        .collect()
}

fn cursor_at(events: &[local_ci_hub::event::RawEvent], head: &str) -> Cursor {
    let bytes: String = events.iter().map(|e| format!("{}\n", e.line)).collect();
    let mut shards = BTreeMap::new();
    shards.insert(
        "ledger/team-a/host-1/2026-11.jsonl".to_owned(),
        ShardPos {
            offset: bytes.len() as u64,
            prefix_sha256: sha256_of(bytes.as_bytes()),
        },
    );
    Cursor {
        authority: "test-store".to_owned(),
        head: head.to_owned(),
        shards,
    }
}

fn temp_index_path(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("local-ci-hub-index-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join("index.sqlite3")
}

#[test]
fn incremental_killed_resumed_and_cold_rebuild_agree() {
    let events = fixture_events(60);
    let batches: Vec<&[local_ci_hub::event::RawEvent]> =
        vec![&events[0..10], &events[10..35], &events[35..60]];
    let final_cursor = cursor_at(&events, "head-60");

    // Incremental: three batches, cursor advancing each time.
    let mut incremental = Index::in_memory("test-store", "/workspace").unwrap();
    let mut seen = 0;
    for batch in batches.iter() {
        seen += batch.len();
        let cursor = cursor_at(&events[..seen], &format!("head-{seen}"));
        incremental.apply(batch, &cursor).unwrap();
    }

    // Cold rebuild: one batch with the final cursor.
    let mut cold = Index::in_memory("test-store", "/workspace").unwrap();
    cold.apply(&events, &final_cursor).unwrap();

    assert_eq!(incremental.dump().unwrap(), cold.dump().unwrap());
    assert_eq!(
        incremental.dump_digest().unwrap(),
        cold.dump_digest().unwrap()
    );
    assert_eq!(incremental.cursor().unwrap().head, "head-60");

    // Killed mid-build and resumed: apply two batches to a file index,
    // drop it (the kill), reopen, apply the rest.
    let path = temp_index_path("resume");
    let _ = std::fs::remove_file(&path);
    {
        let mut partial = Index::open(&path, "test-store", "/workspace").unwrap();
        partial
            .apply(&events[..10], &cursor_at(&events[..10], "head-10"))
            .unwrap();
        partial
            .apply(&events[10..35], &cursor_at(&events[..35], "head-35"))
            .unwrap();
    }
    let mut resumed = Index::open(&path, "test-store", "/workspace").unwrap();
    assert_eq!(resumed.cursor().unwrap().head, "head-35");
    resumed.apply(&events[35..], &final_cursor).unwrap();
    assert_eq!(resumed.dump().unwrap(), cold.dump().unwrap());

    // Every query answer agrees across all three builds.
    for bucket in 0..3 {
        let commit = format!("{:040x}", bucket);
        let from = |index: &Index| -> Vec<String> {
            index
                .rows_for_commit(&commit)
                .unwrap()
                .iter()
                .map(local_ci_hub::pyjson::dumps_sorted)
                .collect()
        };
        assert_eq!(from(&incremental), from(&cold));
        assert_eq!(from(&resumed), from(&cold));
        assert!(!from(&cold).is_empty());
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn conflicting_batch_refuses_and_stores_nothing() {
    let events = fixture_events(5);
    let mut index = Index::in_memory("test-store", "/workspace").unwrap();
    index.apply(&events, &cursor_at(&events, "head-5")).unwrap();
    let before = index.dump_digest().unwrap();

    let mut conflicting = parse_line(&result_line(
        "evt-000",
        "run-000",
        "2026-11-01T00:00:00Z",
        &format!("{:040x}", 9),
        "pass",
    ))
    .unwrap();
    // Same event_id, different body (different commit in row + envelope).
    assert_ne!(conflicting.identity(), events[0].identity());
    let result = index.apply(
        std::slice::from_ref(&conflicting),
        &cursor_at(&events, "head-5"),
    );
    assert!(result.is_err());
    assert_eq!(
        index.dump_digest().unwrap(),
        before,
        "refused batch stores nothing"
    );
    let _ = &mut conflicting;
}

#[test]
fn version_mismatch_discards_and_rebuilds() {
    let path = temp_index_path("version");
    let _ = std::fs::remove_file(&path);
    let events = fixture_events(3);
    {
        let mut index = Index::open(&path, "test-store", "/workspace").unwrap();
        index.apply(&events, &cursor_at(&events, "head-3")).unwrap();
        assert_eq!(index.all_rows().unwrap().len(), 3);
    }
    // Corrupt the stored version directly, then reopen: discard + empty.
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "UPDATE meta SET value = 'ci-hub-index/v999' WHERE key = 'index_version'",
            [],
        )
        .unwrap();
    }
    let reopened = Index::open(&path, "test-store", "/workspace").unwrap();
    assert_eq!(reopened.all_rows().unwrap().len(), 0);
    assert_eq!(reopened.cursor().unwrap().head, "");
    // A different authority also discards.
    let other = Index::open(&path, "other-store", "/workspace").unwrap();
    assert_eq!(other.all_rows().unwrap().len(), 0);
    let _ = std::fs::remove_file(&path);
}
