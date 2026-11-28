//! Provider gaps never halt the bridge (#2440 chat-bridge-never-halt, D5): a gap is admitted at
//! its far side, the agent gets ONE pane line and the owner ONE alarm notice, and the reconcile
//! is shown, unfinished, until the provider has stayed alive long enough for the rescan to fill
//! the window.

use super::*;
use crate::chat_runtime::gap_reconcile::{GAP_RECONCILE_FILE, GAP_RECONCILE_SETTLE};
use crate::chat_service::gap_reconcile::gap_reconcile_step;
use crate::chat_service::owner_alarm::{Notice, Place};

/// A bridge that committed one owner message at `cursor-safe`, then a provider gap whose far
/// side is `cursor-gap`.
fn gap_admitted(name: &str) -> (BridgeState, PathBuf) {
    let root = temporary(name);
    let state = BridgeState::initialize(&root, config()).expect("initialize state");
    let admission = state
        .admit_batch(&indexed_delivery_at(1, 1, "cursor-safe", "before the gap"))
        .expect("safe boundary");
    state.confirm_batch_commit(&admission).expect("commit");
    let gap = state
        .admit_batch(&gap_delivery(2, "cursor-gap", "delivery-gap"))
        .expect("the gap is admitted");
    state.confirm_batch_commit(&gap).expect("commit the gap");
    (state, root)
}

type Told = Vec<(String, String)>;

fn step(state: &BridgeState, now: u64, fail: bool, told: &mut Told, owner: &mut Vec<Notice>) {
    let mut post = |notice: Notice| owner.push(notice);
    gap_reconcile_step(
        state,
        now,
        &mut |body, id| {
            if fail {
                return Err("queue locked".to_owned());
            }
            told.push((body.to_owned(), id.to_owned()));
            Ok(())
        },
        Some(&mut post),
        &mut |_| {},
    );
}

#[test]
fn a_provider_gap_puts_one_alarm_line_in_the_pane_and_one_owner_notice() {
    let (state, root) = gap_admitted("gap-reconcile-alarm");
    let record = state.gap_reconcile().expect("reconcile");
    let now = record.observed_at_millis;
    let (mut told, mut owner) = (Told::new(), Vec::new());
    // A failed queue is tried again at the next check; the owner notice goes out at once.
    step(&state, now, true, &mut told, &mut owner);
    assert!(told.is_empty());
    assert_eq!(owner.len(), 1);
    assert!(!state.gap_reconcile().unwrap().agent_told);
    assert!(state.gap_reconcile().unwrap().owner_told);
    step(&state, now + 30_000, false, &mut told, &mut owner);
    assert_eq!(told.len(), 1, "{told:?}");
    let (body, id) = &told[0];
    assert_eq!(
        id,
        &format!("chat-gap-reconciled-{}", record.observed_at_millis)
    );
    assert!(
        body.starts_with("[chat-bridge ALARM] The chat provider reported a gap in its event stream (fixture history was truncated)."),
        "{body}"
    );
    assert!(
        body.contains("kept running from the gap's far side"),
        "{body}"
    );
    assert!(body.contains(&record.window_text()), "{body}");
    assert!(body.contains("the rescan reads the last 24 h"), "{body}");
    // The window starts at the last commit, a moment ago: all of it is in reach.
    assert!(!body.contains("cannot be backfilled"), "{body}");
    assert_eq!(owner[0].place, Place::TopLevel);
    assert!(
        owner[0]
            .text
            .starts_with("[chat-bridge WARNING] the chat provider lost events in a gap"),
        "{}",
        owner[0].text
    );
    // Once per gap, however many checks, and after a restart.
    for offset in [60_000, 90_000, 3_600_000] {
        step(&state, now + offset, false, &mut told, &mut owner);
    }
    let reopened = BridgeState::open(&root).expect("restart");
    step(&reopened, now + 120_000, false, &mut told, &mut owner);
    assert_eq!(told.len(), 1);
    assert_eq!(owner.len(), 1);
    let stored = reopened.gap_reconcile().unwrap();
    assert!(stored.agent_told && stored.owner_told);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_window_older_than_the_history_says_it_cannot_be_backfilled() {
    // No commit before the gap: the window's start is unknown, so part of it may be older than
    // the rescan's day of history, and both notices say so.
    let root = temporary("gap-reconcile-unknown-window");
    let state = BridgeState::initialize(&root, config()).expect("initialize state");
    let gap = state
        .admit_batch(&gap_delivery(1, "cursor-gap", "delivery-gap"))
        .expect("a gap as the first batch");
    state.confirm_batch_commit(&gap).expect("commit");
    let record = state.gap_reconcile().expect("reconcile");
    assert_eq!(record.window_from_millis, None);
    assert!(record.window_exceeds_history());
    let (mut told, mut owner) = (Told::new(), Vec::new());
    step(
        &state,
        record.observed_at_millis,
        false,
        &mut told,
        &mut owner,
    );
    assert!(told[0].0.contains("cannot be backfilled"), "{}", told[0].0);
    assert!(owner[0].text.contains("send it again"), "{}", owner[0].text);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_reconcile_is_unfinished_until_told_and_the_provider_stays_alive_past_the_settle_time() {
    let (state, root) = gap_admitted("gap-reconcile-finished");
    let now = unix_millis();
    let status = state.status().expect("status");
    assert_eq!(status["gap_reconcile"]["finished"], false);
    assert_eq!(status["gap_reconcile"]["agent_told"], false);
    // Told, and the provider took an item just now: still inside the settle time.
    let record = state.gap_reconcile().unwrap();
    state
        .mark_gap_reconcile_told(record.observed_at_millis, true, true)
        .expect("told");
    state.note_provider_alive();
    assert_eq!(state.status().unwrap()["gap_reconcile"]["finished"], false);
    // The gap seen longer ago than the settle time, the provider alive since: finished.
    let settle = u64::try_from(GAP_RECONCILE_SETTLE.as_millis()).unwrap();
    let mut raw: Value =
        read_document(&root.join(GAP_RECONCILE_FILE), 64 * 1024).expect("raw record");
    raw["observed_at_millis"] = json!(now - settle - 60_000);
    write_document(&root.join(GAP_RECONCILE_FILE), &raw).expect("age the gap");
    let status = state.status().unwrap();
    assert_eq!(status["gap_reconcile"]["finished"], true, "{status}");
    // Shown for a day after the gap, then no more.
    raw["observed_at_millis"] = json!(now - 25 * 3_600_000);
    write_document(&root.join(GAP_RECONCILE_FILE), &raw).expect("a day old");
    assert_eq!(state.status().unwrap()["gap_reconcile"], Value::Null);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn an_unknown_key_in_the_reconcile_record_survives_a_rewrite() {
    // A newer bridge may add a key; marking the record told keeps it.
    let (state, root) = gap_admitted("gap-reconcile-forward");
    let path = root.join(GAP_RECONCILE_FILE);
    let mut raw: Value = read_document(&path, 64 * 1024).expect("raw record");
    raw["added_later"] = json!("kept");
    write_document(&path, &raw).expect("add a key");
    let record = state.gap_reconcile().expect("still read");
    state
        .mark_gap_reconcile_told(record.observed_at_millis, true, false)
        .expect("told");
    let raw: Value = read_document(&path, 64 * 1024).expect("raw record");
    assert_eq!(raw["added_later"], "kept");
    assert_eq!(raw["agent_told"], true);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn end_to_end_a_gap_is_admitted_alarmed_once_and_its_message_delivered_once() {
    // #2440 D5 acceptance: the pump keeps running past a provider gap, the cursor moves to the
    // far side, one alarm goes out, the message sent inside the gap arrives once by a rescan
    // rescue, and the stream's late copy of it is absorbed by the message key.
    let root = temporary("gap-reconcile-end-to-end");
    let state = BridgeState::initialize(&root, config()).expect("initialize state");
    let before = state
        .admit_batch(&indexed_delivery_at(1, 1, "cursor-safe", "before the gap"))
        .expect("safe boundary");
    state.confirm_batch_commit(&before).expect("commit");
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    let mut backend = Backend {
        items: Some(VecDeque::from([
            SubscriptionItem::Batch(gap_delivery(1, "cursor-gap", "delivery-gap")),
            // The rescan's rescue of the message sent during the gap.
            SubscriptionItem::Batch(indexed_delivery_at(
                2,
                7,
                "cursor-gap;rescued:7",
                "sent during the gap",
            )),
            // The same message, carried late by the stream.
            SubscriptionItem::Batch(indexed_delivery_at(
                3,
                7,
                "cursor-late",
                "sent during the gap",
            )),
        ])),
        acknowledged: Arc::clone(&acknowledged),
        next_calls: Arc::new(AtomicU64::new(0)),
        fail_ack: false,
        sabotage_receipt_directory: None,
    };
    let request = state.subscribe_request().expect("request");
    let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");
    let mut new_keys = Vec::new();
    for _ in 0..3 {
        match consume_one(&mut subscription, &state).expect("the pump keeps running") {
            ConsumedItem::Batch(admission) => new_keys.extend(admission.new_request_keys),
            other => panic!("expected a batch, got {other:?}"),
        }
    }
    assert_eq!(acknowledged.lock().unwrap().len(), 3);
    assert_eq!(state.cursor().unwrap().as_deref(), Some("cursor-late"));
    // One request for the message inside the gap, from the rescue; the late copy adds none.
    assert_eq!(new_keys.len(), 1, "{new_keys:?}");
    let record = state.gap_reconcile().expect("reconcile");
    assert_eq!(record.proposed_cursor, "cursor-gap");
    let (mut told, mut owner) = (Told::new(), Vec::new());
    for offset in [0, 30_000, 60_000] {
        step(
            &state,
            record.observed_at_millis + offset,
            false,
            &mut told,
            &mut owner,
        );
    }
    assert_eq!(told.len(), 1);
    assert_eq!(owner.len(), 1);
    fs::remove_dir_all(root).expect("cleanup");
}
