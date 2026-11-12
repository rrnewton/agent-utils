//! Store contract fixtures: fan-out acks, cursor rules, bounded fetch,
//! and the git connector over a synthetic repository.

use local_ci_hub::index::Cursor;
use local_ci_hub::store::{
    AckStatus, BatchLine, FanOut, GitStore, MemoryStore, ReadLimits, RefusalClass, ResetReason,
    Role, Store,
};
use std::path::Path;
use std::process::Command;

fn lines(ids: &[&str]) -> Vec<BatchLine> {
    ids.iter()
        .map(|id| BatchLine {
            event_id: (*id).to_owned(),
            line: format!("{{\"event_id\":\"{id}\"}}"),
        })
        .collect()
}

#[test]
fn memory_append_duplicate_conflict_and_paging() {
    let mut store = MemoryStore::new("mem", Role::Record);
    let acks = store.append(&lines(&["e1", "e2"])).unwrap();
    assert_eq!(acks[0].status, AckStatus::Stored);
    let acks = store.append(&lines(&["e1"])).unwrap();
    assert_eq!(acks[0].status, AckStatus::Duplicate);
    let conflict = vec![BatchLine {
        event_id: "e1".to_owned(),
        line: "{\"event_id\":\"e1\",\"x\":1}".to_owned(),
    }];
    let acks = store.append(&conflict).unwrap();
    assert!(matches!(
        acks[0].status,
        AckStatus::Refused {
            class: RefusalClass::ConflictingEventId,
            ..
        }
    ));

    let fresh = Cursor::new("mem", "");
    let page = store
        .read_from(
            &fresh,
            ReadLimits {
                max_events: 1,
                max_bytes: 1 << 20,
            },
        )
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert!(page.more);
    let page2 = store
        .read_from(&page.next_cursor, ReadLimits::default())
        .unwrap();
    assert_eq!(page2.events.len(), 1);
    assert!(!page2.more);
    assert_eq!(page2.cursor_reset, None);
}

#[test]
fn squash_rebinds_with_reset_and_rewrite_refuses() {
    let mut store = MemoryStore::new("mem", Role::Record);
    store.append(&lines(&["e1", "e2", "e3"])).unwrap();
    let head = store.head().unwrap();
    store.squash();
    let page = store.read_from(&head, ReadLimits::default()).unwrap();
    assert_eq!(page.cursor_reset, Some(ResetReason::HeadRewrite));
    assert!(page.events.is_empty(), "pure squash rescans nothing");
    assert_ne!(
        page.next_cursor.head, head.head,
        "cursor rebinds to the new head"
    );

    // Prefix corruption under a fast-forward head is a store rewrite.
    let mut store2 = MemoryStore::new("mem", Role::Record);
    store2.append(&lines(&["e1"])).unwrap();
    let mut cursor = store2.head().unwrap();
    for pos in cursor.shards.values_mut() {
        pos.prefix_sha256 = "0".repeat(64);
    }
    let err = store2
        .read_from(&cursor, ReadLimits::default())
        .unwrap_err();
    assert_eq!(err.class, RefusalClass::StoreRewrite);
}

#[test]
fn fetch_is_bounded_and_verified() {
    let mut store = MemoryStore::new("mem", Role::Local);
    let digest = store.put_object(b"hello evidence".to_vec());
    let stream = store.fetch_object(&digest, 1024).unwrap();
    assert_eq!(stream.declared_len(), 14);
    assert_eq!(stream.verify().unwrap(), b"hello evidence");

    let err = store.fetch_object(&digest, 4).unwrap_err();
    assert_eq!(err.class, RefusalClass::ObjectTooLarge);
    let err = store.fetch_object(&"f".repeat(64), 1024).unwrap_err();
    assert_eq!(err.class, RefusalClass::ObjectMissing);
}

#[test]
fn fanout_acks_are_independent() {
    let record = Box::new(MemoryStore::new("rec", Role::Record));
    let publish = Box::new(MemoryStore::new("pub", Role::Publish));
    let mut fanout = FanOut::new(vec![record, publish], "rec");
    let results = fanout.append_all(&lines(&["e1"]));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "rec");
    assert_eq!(results[0].1.as_ref().unwrap()[0].status, AckStatus::Stored);
    assert_eq!(results[1].1.as_ref().unwrap()[0].status, AckStatus::Stored);
    let page = fanout
        .read_from(&Cursor::new("rec", ""), ReadLimits::default())
        .unwrap();
    assert_eq!(page.events.len(), 1);
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_0", "user.email")
        .env("GIT_CONFIG_VALUE_0", "t@example.com")
        .env("GIT_CONFIG_KEY_1", "user.name")
        .env("GIT_CONFIG_VALUE_1", "t")
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?} failed");
}

fn write_shard(repo: &Path, rel: &str, contents: &str) {
    let path = repo.join(rel);
    std::fs::create_dir_all(path.parent().expect("parent")).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn git_store_reads_objects_and_survives_squash() {
    let repo = std::env::temp_dir().join(format!("local-ci-hub-git-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main", "."]);
    let shard = "ledger/team-a/host-1/2026-11.jsonl";
    let line1 = "{\"event_id\":\"g1\"}\n";
    let line2 = "{\"event_id\":\"g2\"}\n";
    write_shard(&repo, shard, line1);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: one"]);
    write_shard(&repo, shard, &format!("{line1}{line2}"));
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: two"]);

    let store = GitStore::new("git-lcd", Role::Record, repo.clone(), "HEAD");
    let fresh = Cursor::new("git-lcd", "");
    let page = store.read_from(&fresh, ReadLimits::default()).unwrap();
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].line, "{\"event_id\":\"g1\"}");
    let head = store.head().unwrap();
    assert_eq!(
        head.shards[shard].offset,
        (line1.len() + line2.len()) as u64
    );

    // Squash: parentless commit with the same tree. Prefixes still verify,
    // so the read rebinds with head-rewrite and rescans nothing.
    git(&repo, &["checkout", "-q", "--orphan", "squashed"]);
    git(&repo, &["commit", "-qm", "toy: squash"]);
    git(&repo, &["branch", "-qD", "main"]);
    git(&repo, &["branch", "-qm", "main"]);
    let page = store.read_from(&head, ReadLimits::default()).unwrap();
    assert_eq!(page.cursor_reset, Some(ResetReason::HeadRewrite));
    assert!(page.events.is_empty());

    // Append on the new head, then rewrite history again with changed
    // bytes: the read rescans from the first unverifiable byte.
    let line3 = "{\"event_id\":\"g3\"}\n";
    write_shard(&repo, shard, &format!("{line1}{line2}{line3}"));
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "toy: three"]);
    let page = store
        .read_from(&page.next_cursor, ReadLimits::default())
        .unwrap();
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].line, "{\"event_id\":\"g3\"}");

    // Git appends refuse per event and store nothing.
    let mut store = store;
    let acks = store.append(&lines(&["gx"])).unwrap();
    assert!(matches!(acks[0].status, AckStatus::Refused { .. }));

    let _ = std::fs::remove_dir_all(&repo);
}
