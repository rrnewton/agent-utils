//! Reply windows (#2338 chat-bridge-reply-window-retire): requests close and retire by
//! themselves, an unanswered one is listed exactly once, and a full population holds new messages
//! uncommitted until space frees. Every time is injected: the sweep takes its time as an
//! argument, and admission reads the per-thread test clock, so nothing depends on how fast the
//! host runs.

use super::*;
use crate::chat_runtime::reply_windows::{
    listed_keys, ClosedRequest, ClosureCause, ReplyWindowSettings, WindowSweep,
};
use crate::chat_service::owner_alarm::{Notice, OwnerAlarm, DEFAULT_AFTER};
use crate::chat_service::reply_windows::{capacity_alarm, sweep, tell_closures};

const HOUR: u64 = 3_600_000;

/// A request admitted at index 1 and delivered, with the cursor advanced past it by a second
/// (pending) request, under a configuration without acknowledgements.
fn delivered_request(name: &str) -> (BridgeState, String, PathBuf) {
    let (state, key, root) = state_with_old_request(name, config_without_reaction());
    state
        .set_delivery_phase(&key, RequestPhase::Delivered, None)
        .expect("mark delivered");
    (state, key, root)
}

/// Capture one reply for `key` and send it; the reply's send time.
fn answer(state: &BridgeState, key: &str, body: &str) -> u64 {
    let route = state
        .next_reply_route(key)
        .expect("route")
        .expect("open route");
    state
        .capture_replies(
            key,
            &format!(
                "<CHAT_REPLY_{id}>\n{body}\n</CHAT_REPLY_{id}>",
                id = route.identifier
            ),
        )
        .expect("capture reply");
    let mut transport = PermanentReplyTransport::default();
    state
        .publish_one(key, &mut transport)
        .expect("publish")
        .expect("provider receipt");
    let ordinal = state.read_request(key).expect("request").reply_count;
    state
        .read_reply(key, ordinal)
        .expect("reply")
        .sent_at_millis
        .expect("sent time")
}

#[test]
fn an_answered_request_closes_and_retires_a_day_after_its_last_reply_and_not_before() {
    let (state, key, root) = delivered_request("window-answered");
    let sent = answer(&state, &key, "done");
    let quiet = state.reply_window_rules().answered_quiet_millis;
    assert_eq!(quiet, 24 * HOUR);

    assert_eq!(
        state
            .close_settled_replies(sent + quiet - 1)
            .expect("early sweep"),
        WindowSweep::default()
    );
    assert!(!state.read_request(&key).expect("still open").reply_closed);

    let swept = state.close_settled_replies(sent + quiet).expect("sweep");
    assert_eq!((swept.answered, swept.unanswered, swept.retired), (1, 0, 1));
    assert!(state.retired_key(&key).expect("retired guard").is_some());
    // An answered request needs no notice, so nothing is listed.
    assert!(state.closure_batches().expect("outbox").0.is_empty());
    assert!(!root.join(reply_windows::CLOSURE_OUTBOX_FILE).exists());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_later_reply_restarts_the_answered_window() {
    let (state, key, root) = delivered_request("window-answered-later");
    let first = answer(&state, &key, "starting");
    let quiet = state.reply_window_rules().answered_quiet_millis;
    let nonce = state.read_request(&key).expect("request").reply_nonce;
    state
        .submit_reply(&key, &format!("{nonce}_2"), "landed")
        .expect("second reply");
    let mut transport = PermanentReplyTransport::default();
    state
        .publish_one(&key, &mut transport)
        .expect("publish")
        .expect("receipt");
    let second = state
        .read_reply(&key, 2)
        .expect("second reply")
        .sent_at_millis
        .expect("sent");
    // Pretend the second reply came an hour after the first: the window counts from it.
    let mut reply = state.read_reply(&key, 2).expect("second reply");
    reply.sent_at_millis = Some(first.max(second) + HOUR);
    write_document(&state.reply_path(&key, 2), &reply).expect("move the second reply");
    assert_eq!(
        state
            .close_settled_replies(first + quiet)
            .expect("sweep from the first reply"),
        WindowSweep::default()
    );
    let swept = state
        .close_settled_replies(first.max(second) + HOUR + quiet)
        .expect("sweep from the last reply");
    assert_eq!(swept.answered, 1);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn pending_uncertain_and_mid_reply_requests_never_close_however_old() {
    // Pending: the second request of the fixture, never delivered.
    let (state, delivered, root) = delivered_request("window-never");
    // Mid-reply: a captured reply that is not sent yet.
    let route = state
        .next_reply_route(&delivered)
        .expect("route")
        .expect("open route");
    state
        .capture_replies(
            &delivered,
            &format!(
                "<CHAT_REPLY_{id}>\nhalf way\n</CHAT_REPLY_{id}>",
                id = route.identifier
            ),
        )
        .expect("capture an unsent reply");
    // Uncertain: a third request whose delivery may or may not have happened.
    let uncertain = state
        .admit_batch(&indexed_delivery(3, 3))
        .expect("third admission")
        .new_request_keys
        .remove(0);
    state
        .admit_batch(&indexed_delivery(4, 4))
        .expect("advance cursor");
    state
        .set_delivery_phase(
            &uncertain,
            RequestPhase::DeliveryUncertain,
            Some("the pane did not show the prompt"),
        )
        .expect("mark uncertain");
    let far = unix_millis() + 365 * 24 * HOUR;
    assert_eq!(
        state.close_settled_replies(far).expect("sweep"),
        WindowSweep::default()
    );
    for (key, _) in state.request_records().expect("records") {
        assert!(!key.reply_closed, "{} closed", key.key);
    }
    assert!(state.closure_batches().expect("outbox").0.is_empty());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn an_unanswered_owner_request_closed_by_the_sweep_is_listed_exactly_once() {
    let (state, key, root) = delivered_request("window-unanswered");
    let admitted = state
        .read_request(&key)
        .expect("request")
        .admitted_at_millis;
    let age = state.reply_window_rules().unanswered_max_age_millis;
    assert_eq!(age, 48 * HOUR);
    // Never at a day, the reply reminders' window, nor just before two days.
    for at in [admitted + 24 * HOUR, admitted + age - 1] {
        assert_eq!(
            state.close_settled_replies(at).expect("early sweep"),
            WindowSweep::default()
        );
    }

    let mut logged = Vec::new();
    sweep(&state, admitted + age, &mut |line| logged.push(line));
    assert!(state.retired_key(&key).expect("retired guard").is_some());
    let (batches, dropped) = state.closure_batches().expect("outbox");
    assert_eq!(dropped, 0);
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].cause, ClosureCause::Age);
    assert_eq!(listed_keys(&batches), BTreeSet::from([key.clone()]));
    let listed = &batches[0].requests[0];
    assert!(listed.from_owner);
    assert_eq!(listed.first_line, "request 1");
    // A second sweep finds nothing more to list.
    sweep(&state, admitted + 2 * age, &mut |line| logged.push(line));
    assert_eq!(state.closure_batches().expect("outbox").0.len(), 1);
    assert_eq!(logged.len(), 1, "{logged:?}");

    let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
    let mut agent_notices: Vec<(String, String)> = Vec::new();
    let mut owner_notices: Vec<Notice> = Vec::new();
    let mut lines = Vec::new();
    for _ in 0..2 {
        tell_closures(
            &state,
            Some(&mut alarm),
            &mut |body, id| {
                agent_notices.push((body.to_owned(), id.to_owned()));
                Ok(())
            },
            &mut |_, notices| owner_notices.extend(notices),
            &mut |line| lines.push(line),
        );
    }
    let listings: Vec<_> = lines
        .iter()
        .filter(|line| line.contains("closed UNANSWERED request"))
        .collect();
    assert_eq!(listings.len(), 1, "{lines:?}");
    assert!(listings[0].contains(&key) && listings[0].contains("\"request 1\""));
    assert_eq!(agent_notices.len(), 1, "{agent_notices:?}");
    let (body, id) = &agent_notices[0];
    assert!(id.starts_with("chat-window-closed-"), "{id}");
    assert!(
        body.contains(&listed.reply_id) && body.contains("\"request 1\""),
        "{body}"
    );
    assert!(body.contains("from the owner"), "{body}");
    assert_eq!(owner_notices.len(), 1, "{owner_notices:?}");
    let text = &owner_notices[0].text;
    assert!(
        text.starts_with("[chat-bridge WARNING] 1 of your messages"),
        "{text}"
    );
    assert!(
        text.contains("no reply for 48 h") && text.contains("\"request 1\""),
        "{text}"
    );
    // Every step is done, so the outbox is gone.
    assert!(state.closure_batches().expect("outbox").0.is_empty());
    assert!(!root.join(reply_windows::CLOSURE_OUTBOX_FILE).exists());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_failed_agent_notice_is_tried_again_and_the_log_and_owner_notice_are_not_repeated() {
    let (state, key, root) = delivered_request("window-notice-retry");
    let admitted = state
        .read_request(&key)
        .expect("request")
        .admitted_at_millis;
    state
        .close_settled_replies(admitted + 48 * HOUR)
        .expect("sweep");
    let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
    let mut attempts = 0;
    let mut owner = 0;
    let mut listings = 0;
    for _ in 0..3 {
        tell_closures(
            &state,
            Some(&mut alarm),
            &mut |_, _| {
                attempts += 1;
                if attempts == 1 {
                    Err("the queue is locked".to_owned())
                } else {
                    Ok(())
                }
            },
            &mut |_, notices| owner += notices.len(),
            &mut |line| listings += usize::from(line.contains("closed UNANSWERED")),
        );
    }
    assert_eq!((attempts, owner, listings), (2, 1, 1));
    assert!(state.closure_batches().expect("outbox").0.is_empty());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn configured_windows_replace_the_defaults_and_bad_ones_are_refused() {
    let mut configuration = config_without_reaction();
    configuration.reply_windows = Some(ReplyWindowSettings {
        answered_quiet_seconds: Some(3_600),
        unanswered_max_age_seconds: Some(7_200),
        ..ReplyWindowSettings::default()
    });
    configuration.validate().expect("valid windows");
    let (state, key, root) = state_with_old_request("window-configured", configuration.clone());
    let rules = state.reply_window_rules();
    assert_eq!(rules.answered_quiet_millis, HOUR);
    assert_eq!(rules.unanswered_max_age_millis, 2 * HOUR);
    assert_eq!(rules.pressure_unanswered_age_millis, 24 * HOUR);
    state
        .set_delivery_phase(&key, RequestPhase::Delivered, None)
        .expect("mark delivered");
    let admitted = state
        .read_request(&key)
        .expect("request")
        .admitted_at_millis;
    assert_eq!(
        state
            .close_settled_replies(admitted + 2 * HOUR)
            .expect("sweep")
            .unanswered,
        1
    );
    fs::remove_dir_all(root).expect("cleanup");

    for bad in [0, 59, 400 * 24 * 3_600] {
        configuration.reply_windows = Some(ReplyWindowSettings {
            answered_quiet_seconds: Some(bad),
            ..ReplyWindowSettings::default()
        });
        assert!(configuration.validate().is_err(), "{bad}");
    }
}

fn backend(batch: DeliveryBatch) -> (Backend, Arc<Mutex<Vec<String>>>) {
    let acknowledged = Arc::new(Mutex::new(Vec::new()));
    (
        Backend {
            items: Some(VecDeque::from([SubscriptionItem::Batch(batch)])),
            acknowledged: Arc::clone(&acknowledged),
            next_calls: Arc::new(AtomicU64::new(0)),
            fail_ack: false,
            sabotage_receipt_directory: None,
        },
        acknowledged,
    )
}

/// Receive `batch` through a fresh subscription from the saved cursor, as the provider worker
/// does after each reconnect (each subscription numbers its batches from 1).
fn consume(state: &BridgeState, batch: DeliveryBatch) -> (Result<ConsumedItem>, Vec<String>) {
    let (mut backend, acknowledged) = backend(batch);
    let request = state.subscribe_request().expect("subscription request");
    let mut subscription = ChatSubscription::open(&mut backend, &request).expect("subscribe");
    let consumed = consume_one(&mut subscription, state);
    drop(subscription);
    let acknowledged = acknowledged.lock().expect("ack lock").clone();
    (consumed, acknowledged)
}

#[test]
fn a_full_population_retires_answered_requests_first_then_holds_new_messages_until_space_frees() {
    let root = temporary("window-capacity");
    let state =
        BridgeState::initialize(&root, config_without_reaction()).expect("initialize state");
    // One answered request, admitted through the stream and committed.
    let first = state
        .admit_batch(&indexed_delivery_at(1, 1, "cursor-1", "answered request"))
        .expect("admit the first request");
    state.confirm_batch_commit(&first).expect("commit");
    let answered = first.new_request_keys[0].clone();
    state
        .set_delivery_phase(&answered, RequestPhase::Delivered, None)
        .expect("deliver the first request");
    answer(&state, &answered, "done");

    // The rest of the population: delivered requests with no reply yet, admitted at `base`.
    let base = unix_millis();
    let mut oldest_unanswered = None;
    for index in 2..=MAX_REQUESTS {
        let batch = indexed_delivery_at(
            index,
            index,
            &format!("cursor-old-{index}"),
            &format!("unanswered request {index}\nsecond line"),
        );
        let CommittableEvent::MessageCreated(message) = &batch.events()[0] else {
            panic!("message fixture");
        };
        let mut record =
            RequestRecord::from_saved_message(SavedMessage::from_inbound(message), None)
                .expect("record");
        record.admitted_cursor = Some(format!("cursor-old-{index}"));
        record.phase = RequestPhase::Delivered;
        // The lower the index, the older the request.
        record.admitted_at_millis = base + index;
        record.delivery_started_at_millis = Some(base + index);
        record.delivered_at_millis = Some(base + index);
        if oldest_unanswered.is_none() {
            oldest_unanswered = Some(record.key.clone());
        }
        write_document(&state.request_path(&record.key), &record).expect("write fixture");
    }
    agent::sync_directory(&root.join("requests")).expect("sync fixtures");
    let records = state.request_records().expect("full population");
    assert_eq!(records.len(), usize::try_from(MAX_REQUESTS).expect("cap"));
    let mut checkpoint = state.read_checkpoint().expect("checkpoint");
    checkpoint.request_count = MAX_REQUESTS;
    checkpoint.request_bytes = records.iter().map(|(_, bytes)| *bytes).sum();
    write_document(&root.join("checkpoint.json"), &checkpoint).expect("full checkpoint");
    let state = BridgeState::open(&root).expect("reopen the full state");

    // An hour later the next owner message arrives: the answered request makes room, whatever
    // its age, and no unanswered one (an hour old) is touched.
    crate::prompt_time::test_clock::set(base + HOUR, 0, "UTC");
    let (consumed, acknowledged) = consume(
        &state,
        indexed_delivery_at(1, 50_001, "cursor-next", "next owner message"),
    );
    let ConsumedItem::Batch(admission) = consumed.expect("admitted after a retirement") else {
        panic!("expected a batch");
    };
    assert_eq!(admission.new_request_keys.len(), 1);
    assert_eq!(acknowledged, vec!["delivery-1-50001".to_owned()]);
    assert!(state
        .retired_key(&answered)
        .expect("retired guard")
        .is_some());
    let (batches, _) = state.closure_batches().expect("outbox");
    assert_eq!(batches.len(), 1);
    assert_eq!(batches[0].cause, ClosureCause::Capacity);
    assert_eq!(batches[0].answered_closed, 1);
    assert!(batches[0].requests.is_empty());
    assert!(state.request_capacity_hold().is_none());

    // The next one finds nothing that may close: it is held, uncommitted, and the cursor stays.
    let held = indexed_delivery_at(1, 50_002, "cursor-held", "held owner message");
    for refusal in 1..=2 {
        let (consumed, acknowledged) = consume(&state, held.clone());
        let error = consumed.expect_err("refused at the limit").to_string();
        assert!(error.contains("population limit reached"), "{error}");
        assert!(
            error.contains("1 new message(s) held uncommitted"),
            "{error}"
        );
        assert!(acknowledged.is_empty(), "nothing committed upstream");
        assert_eq!(
            state.cursor().expect("cursor").as_deref(),
            Some("cursor-next")
        );
        assert_eq!(
            state
                .subscribe_request()
                .expect("request")
                .resume_from()
                .map(ProviderCursor::as_str),
            Some("cursor-next"),
            "a reconnect resumes before the held message"
        );
        let hold = state.request_capacity_hold().expect("hold recorded");
        assert_eq!(hold.refusals, refusal);
        assert_eq!(hold.held_since_millis, base + HOUR);
        assert_eq!(hold.waiting_messages, 1);
        assert_eq!(hold.request_count, MAX_REQUESTS);
    }
    let status = state.status().expect("status");
    assert_eq!(status["request_capacity"]["hold"]["refusals"], 2);
    assert_eq!(status["request_capacity"]["request_count"], MAX_REQUESTS);

    // A day after admission the oldest unanswered request may close under pressure: the held
    // message is admitted on the next delivery, and that request is listed.
    crate::prompt_time::test_clock::set(base + 2 + 24 * HOUR, 0, "UTC");
    let (consumed, acknowledged) = consume(&state, held);
    let ConsumedItem::Batch(admission) = consumed.expect("admitted once space frees") else {
        panic!("expected a batch");
    };
    assert_eq!(admission.new_request_keys.len(), 1);
    assert_eq!(acknowledged, vec!["delivery-1-50002".to_owned()]);
    assert_eq!(
        state.cursor().expect("cursor").as_deref(),
        Some("cursor-held")
    );
    assert!(state.request_capacity_hold().is_none());
    let oldest = oldest_unanswered.expect("oldest key");
    assert!(state.retired_key(&oldest).expect("retired guard").is_some());
    let (batches, _) = state.closure_batches().expect("outbox");
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[1].cause, ClosureCause::Capacity);
    assert_eq!(listed_keys(&batches[1..]), BTreeSet::from([oldest]));
    assert_eq!(batches[1].requests[0].first_line, "unanswered request 2");
    assert_eq!(
        state.read_checkpoint().expect("checkpoint").request_count,
        MAX_REQUESTS
    );
    crate::prompt_time::test_clock::clear();
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_full_store_puts_one_alarm_line_in_the_agents_pane_at_once() {
    // #2440 chat-bridge-never-halt, lead-014: the full store held owner messages for hours with
    // only a log line; the agent now gets ONE chat-visible alarm at once, per hold.
    use crate::chat_service::reply_windows::capacity_hold_step;
    let (state, _, root) = delivered_request("window-capacity-pane");
    let checkpoint = state.read_checkpoint().expect("checkpoint");
    let start = 1_790_000_000_000;
    let mut told_hold = None;
    let mut told: Vec<(String, String)> = Vec::new();
    let step =
        |told_hold: &mut Option<u64>, at: u64, told: &mut Vec<(String, String)>, fail: bool| {
            capacity_hold_step(
                told_hold,
                state.request_capacity_hold().as_ref(),
                at,
                &mut |body, id| {
                    if fail {
                        return Err("queue locked".to_owned());
                    }
                    told.push((body.to_owned(), id.to_owned()));
                    Ok(())
                },
                &mut |_| {},
            );
        };
    // No hold: nothing.
    step(&mut told_hold, start, &mut told, false);
    assert!(told.is_empty());
    {
        let lock = agent::open_private_lock(&root.join(".state.lock"), "test state lock")
            .expect("open state lock");
        lock.lock_exclusive().expect("lock state");
        state
            .note_capacity_refusal_locked(&checkpoint, 12, start)
            .expect("record the hold");
    }
    // A failed queue is tried again at the next check.
    step(&mut told_hold, start + 30_000, &mut told, true);
    assert!(told.is_empty());
    step(&mut told_hold, start + 60_000, &mut told, false);
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(
        told[0]
            .0
            .starts_with("[chat-bridge ALARM] The chat bridge's request store is FULL ("),
        "{}",
        told[0].0
    );
    assert!(told[0]
        .0
        .contains("12 new owner message(s) are held unacknowledged"));
    assert_eq!(told[0].1, format!("chat-capacity-hold-{start}"));
    // Once per hold, however many checks see it.
    for offset in [90_000, 120_000, HOUR] {
        step(&mut told_hold, start + offset, &mut told, false);
    }
    assert_eq!(told.len(), 1);
    // The hold clears, then a new one: told again.
    {
        let lock = agent::open_private_lock(&root.join(".state.lock"), "test state lock")
            .expect("open state lock");
        lock.lock_exclusive().expect("lock state");
        state.clear_capacity_hold_locked().expect("clear");
    }
    step(&mut told_hold, start + 2 * HOUR, &mut told, false);
    {
        let lock = agent::open_private_lock(&root.join(".state.lock"), "test state lock")
            .expect("open state lock");
        lock.lock_exclusive().expect("lock state");
        state
            .note_capacity_refusal_locked(&checkpoint, 1, start + 3 * HOUR)
            .expect("record a new hold");
    }
    step(&mut told_hold, start + 3 * HOUR, &mut told, false);
    assert_eq!(told.len(), 2);
    assert_eq!(
        told[1].1,
        format!("chat-capacity-hold-{}", start + 3 * HOUR)
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn the_full_store_warning_repeats_hourly_and_clears_once() {
    let (state, _, root) = delivered_request("window-capacity-alarm");
    let checkpoint = state.read_checkpoint().expect("checkpoint");
    let start = 1_790_000_000_000;
    {
        let lock = agent::open_private_lock(&root.join(".state.lock"), "test state lock")
            .expect("open state lock");
        lock.lock_exclusive().expect("lock state");
        state
            .note_capacity_refusal_locked(&checkpoint, 3, start)
            .expect("record the hold");
    }
    let mut alarm = OwnerAlarm::new(DEFAULT_AFTER);
    let mut posted: Vec<Vec<Notice>> = Vec::new();
    for at in [start, start + HOUR - 1, start + HOUR] {
        capacity_alarm(&state, &mut alarm, at, &mut |_, notices| {
            posted.push(notices)
        });
    }
    assert_eq!(posted.len(), 2, "{posted:?}");
    for notices in &posted {
        assert_eq!(notices.len(), 1);
        let text = &notices[0].text;
        assert!(
            text.starts_with("[chat-bridge WARNING] New messages to"),
            "{text}"
        );
        assert!(
            text.contains("3 new message(s) wait unacknowledged"),
            "{text}"
        );
    }
    // A restarted bridge carries on from the saved episode: no new warning inside the hour.
    let mut restarted = OwnerAlarm::new(DEFAULT_AFTER);
    capacity_alarm(
        &state,
        &mut restarted,
        start + HOUR + 1,
        &mut |_, notices| {
            posted.push(notices);
        },
    );
    assert_eq!(posted.len(), 2, "{posted:?}");

    state.clear_capacity_hold_locked().expect("clear the hold");
    for at in [start + 2 * HOUR, start + 3 * HOUR] {
        capacity_alarm(&state, &mut restarted, at, &mut |_, notices| {
            posted.push(notices)
        });
    }
    assert_eq!(posted.len(), 3, "{posted:?}");
    assert!(
        posted[2][0].text.starts_with("[chat-bridge] Resolved"),
        "{:?}",
        posted[2]
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_request_whose_every_reply_failed_is_unanswered_and_listed_when_it_closes() {
    // Answered means a reply reached the chat (phase Sent); one whose replies all failed never
    // did, so it waits the unanswered window and is listed (#2338, consumer condition 1).
    let (state, key, root) = delivered_request("window-all-failed");
    let route = state
        .next_reply_route(&key)
        .expect("route")
        .expect("open route");
    state
        .capture_replies(
            &key,
            &format!(
                "<CHAT_REPLY_{id}>\ndoomed\n</CHAT_REPLY_{id}>",
                id = route.identifier
            ),
        )
        .expect("capture");
    let mut transport = PermanentReplyTransport {
        refuse_text: Some("doomed".to_owned()),
        ..PermanentReplyTransport::default()
    };
    assert!(state.publish_one(&key, &mut transport).is_err());
    assert_eq!(
        state.read_reply(&key, 1).expect("reply").phase,
        ReplyPhase::Failed
    );
    let admitted = state
        .read_request(&key)
        .expect("request")
        .admitted_at_millis;
    // Not closed as answered a day later, nor before the unanswered window ends.
    assert_eq!(
        state
            .close_settled_replies(admitted + 48 * HOUR - 1)
            .expect("sweep"),
        WindowSweep::default()
    );
    let swept = state
        .close_settled_replies(admitted + 48 * HOUR)
        .expect("sweep");
    assert_eq!((swept.answered, swept.unanswered), (0, 1));
    let (batches, _) = state.closure_batches().expect("outbox");
    assert_eq!(listed_keys(&batches), BTreeSet::from([key.clone()]));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn the_agent_is_warned_at_the_high_water_mark_once_and_told_below_the_low_water_mark() {
    use crate::chat_service::reply_windows::{high_water_step, HighWater};
    let rules = ReplyWindowSettings::default().rules();
    assert_eq!(
        (rules.high_water_percent, rules.low_water_percent),
        (90, 80)
    );
    let limit = MAX_REQUESTS;
    let mut high_water = HighWater::default();
    let mut told: Vec<(String, String)> = Vec::new();
    let step = |high_water: &mut HighWater,
                count: u64,
                at: u64,
                told: &mut Vec<(String, String)>,
                fail: bool| {
        high_water_step(
            high_water,
            count,
            limit,
            rules,
            at,
            &mut |body, id| {
                if fail {
                    return Err("queue locked".to_owned());
                }
                told.push((body.to_owned(), id.to_owned()));
                Ok(())
            },
            &mut |_| {},
        );
    };
    let start = 1_790_000_000_000;
    // Below the mark: nothing. 1843 of 2048 is 89.99%.
    step(&mut high_water, 1_843, start, &mut told, false);
    assert!(told.is_empty());
    // A failed queue at the crossing is tried again at the next check.
    step(&mut high_water, 1_844, start + 30_000, &mut told, true);
    assert!(told.is_empty());
    step(&mut high_water, 1_844, start + 60_000, &mut told, false);
    assert_eq!(told.len(), 1);
    assert!(
        told[0].0.starts_with(
            "[chat-bridge WARNING] The chat bridge's request store is 90% full (1844 of 2048"
        ),
        "{}",
        told[0].0
    );
    assert!(told[0].1.starts_with("chat-high-water-up-"));
    // Still full, fuller, or between the marks: no more notices.
    for (count, offset) in [(1_900, 90_000), (2_000, 120_000), (1_700, 150_000)] {
        step(&mut high_water, count, start + offset, &mut told, false);
    }
    assert_eq!(told.len(), 1);
    // Below 80%: one all-clear.
    step(&mut high_water, 1_638, start + 180_000, &mut told, false);
    assert_eq!(told.len(), 2);
    assert!(
        told[1].0.contains("back below 80% (1638 of 2048"),
        "{}",
        told[1].0
    );
    step(&mut high_water, 1_600, start + 210_000, &mut told, false);
    assert_eq!(told.len(), 2);
    // Full again within the hour: held back until the hour has passed, then told once.
    step(&mut high_water, 1_900, start + 240_000, &mut told, false);
    assert_eq!(told.len(), 2);
    step(
        &mut high_water,
        1_900,
        start + 60_000 + HOUR,
        &mut told,
        false,
    );
    assert_eq!(told.len(), 3);
}

/// Three delivered requests with no reply and a fourth, pending, after them.
fn three_unanswered(name: &str) -> (BridgeState, Vec<String>, String, PathBuf) {
    let root = temporary(name);
    let state = BridgeState::initialize(&root, config_without_reaction()).expect("initialize");
    let mut keys = Vec::new();
    for index in 1..=3 {
        let key = state
            .admit_batch(&indexed_delivery(index, index))
            .expect("admit")
            .new_request_keys
            .remove(0);
        keys.push(key);
    }
    let pending = state
        .admit_batch(&indexed_delivery(4, 4))
        .expect("admit the pending one")
        .new_request_keys
        .remove(0);
    for key in &keys {
        state
            .set_delivery_phase(key, RequestPhase::Delivered, None)
            .expect("deliver");
    }
    (state, keys, pending, root)
}

#[test]
fn a_sweep_cut_short_by_a_crash_finishes_after_the_restart_and_lists_each_request_once() {
    // Count the retirement boundaries one retirement crosses.
    let (state, keys, _, root) = three_unanswered("window-crash-measure");
    let latest = keys
        .iter()
        .map(|key| state.read_request(key).expect("request").admitted_at_millis)
        .max()
        .expect("keys");
    state
        .close_settled_replies(latest + 48 * HOUR)
        .expect("sweep");
    let per_retirement = state
        .retirement_boundary_count
        .load(std::sync::atomic::Ordering::SeqCst)
        / 3;
    assert!(per_retirement >= 2, "{per_retirement}");
    fs::remove_dir_all(root).expect("cleanup");

    // The same sweep, cut short in the middle of the second retirement.
    let (state, keys, pending, root) = three_unanswered("window-crash");
    let latest = keys
        .iter()
        .map(|key| state.read_request(key).expect("request").admitted_at_millis)
        .max()
        .expect("keys");
    let cursor = state.cursor().expect("cursor");
    *state
        .retirement_fault_after
        .lock()
        .expect("retirement fault lock") = Some(per_retirement + per_retirement / 2);
    assert!(state.close_settled_replies(latest + 48 * HOUR).is_err());
    drop(state);

    // The restart: the state reopens as a new `chat run` opens it, and the next sweep finishes.
    let state = BridgeState::open(&root).expect("reopen after the crash");
    state
        .close_settled_replies(latest + 48 * HOUR)
        .expect("sweep after the restart");
    for key in &keys {
        assert!(
            state.retired_key(key).expect("retired guard").is_some(),
            "{key}"
        );
    }
    let (batches, _) = state.closure_batches().expect("outbox");
    let listed: Vec<&str> = batches
        .iter()
        .flat_map(|batch| batch.requests.iter().map(|request| request.key.as_str()))
        .collect();
    assert_eq!(listed.len(), 3, "each listed once: {listed:?}");
    assert_eq!(
        listed_keys(&batches),
        keys.iter().cloned().collect::<BTreeSet<_>>()
    );
    // Nothing else was touched: the pending request and the cursor are as they were.
    let record = state.read_request(&pending).expect("pending request kept");
    assert_eq!(record.phase, RequestPhase::Pending);
    assert!(!record.reply_closed);
    assert_eq!(state.cursor().expect("cursor"), cursor);
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn every_request_admitted_before_a_restart_survives_it_and_the_first_sweep() {
    // A deploy restarts `chat run` over a live state: requests pending, delivered, answered, and
    // mid-reply all reload as they were, and the new binary's first sweep closes none of them.
    let (state, delivered, root) = delivered_request("window-cutover");
    let answered = state
        .admit_batch(&indexed_delivery(3, 3))
        .expect("admit")
        .new_request_keys
        .remove(0);
    let mid_reply = state
        .admit_batch(&indexed_delivery(4, 4))
        .expect("admit")
        .new_request_keys
        .remove(0);
    let pending = state
        .admit_batch(&indexed_delivery(5, 5))
        .expect("admit")
        .new_request_keys
        .remove(0);
    for key in [&answered, &mid_reply] {
        state
            .set_delivery_phase(key, RequestPhase::Delivered, None)
            .expect("deliver");
    }
    answer(&state, &answered, "done");
    let route = state
        .next_reply_route(&mid_reply)
        .expect("route")
        .expect("open");
    state
        .capture_replies(
            &mid_reply,
            &format!(
                "<CHAT_REPLY_{id}>\nnot sent yet\n</CHAT_REPLY_{id}>",
                id = route.identifier
            ),
        )
        .expect("capture");
    let before: BTreeMap<String, RequestRecord> = state
        .request_records()
        .expect("records")
        .into_iter()
        .map(|(record, _)| (record.key.clone(), record))
        .collect();
    let cursor = state.cursor().expect("cursor");
    drop(state);

    let state = BridgeState::open(&root).expect("the new binary opens the state");
    let swept = state
        .close_settled_replies(unix_millis())
        .expect("first sweep");
    assert_eq!(swept, WindowSweep::default());
    let after: BTreeMap<String, RequestRecord> = state
        .request_records()
        .expect("records")
        .into_iter()
        .map(|(record, _)| (record.key.clone(), record))
        .collect();
    assert_eq!(after, before);
    for key in [&delivered, &answered, &mid_reply, &pending] {
        assert!(after.contains_key(key.as_str()), "{key}");
    }
    assert_eq!(after[&pending].phase, RequestPhase::Pending);
    assert_eq!(state.cursor().expect("cursor"), cursor);
    assert_eq!(
        state
            .subscribe_request()
            .expect("request")
            .resume_from()
            .map(|cursor| cursor.as_str().to_owned()),
        cursor
    );
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn a_request_the_sweep_retires_is_never_reminded_afterwards() {
    // #2331 reply reminders and #2338 reply windows on the same requests. A configured unanswered
    // window of 2 h closes requests well inside the reminders' one-day window. Once the sweep
    // retires them, no reminder may name them again, whether one already did or not, and the
    // reminder record forgets them, across a restart too.
    use crate::chat_runtime::reply_reminders::ReminderCandidates;
    let mut configuration = config_without_reaction();
    configuration.reply_windows = Some(ReplyWindowSettings {
        unanswered_max_age_seconds: Some(7_200),
        ..ReplyWindowSettings::default()
    });
    configuration.validate().expect("valid windows");
    let (state, reminded, root) = state_with_old_request("window-reminders", configuration);
    let admitted = state
        .read_request(&reminded)
        .expect("request")
        .admitted_at_millis;
    // Reminders started before either request was delivered.
    state
        .update_reply_reminders(admitted.saturating_sub(1), None, |_| (false, ()))
        .expect("start reminders");
    let unreminded = state
        .admit_batch(&indexed_delivery(3, 3))
        .expect("admit")
        .new_request_keys
        .remove(0);
    for key in [&reminded, &unreminded] {
        state
            .set_delivery_phase(key, RequestPhase::Delivered, None)
            .expect("deliver");
    }
    let before = state.reply_reminder_candidates().expect("candidates");
    let reply_id = before
        .candidates
        .iter()
        .find(|candidate| candidate.key == reminded)
        .expect("a candidate")
        .reply_id
        .clone();
    let delivered = before
        .candidates
        .iter()
        .map(|candidate| candidate.delivered_at_millis)
        .max()
        .expect("delivered");
    // One of them was reminded half an hour after delivery.
    let reminded_at = delivered + HOUR / 2;
    let (record, ()) = state
        .update_reply_reminders(reminded_at, Some(&before.retained), |record| {
            record.reminded.push(RemindedRequest {
                key: reminded.clone(),
                reply_id,
                at_millis: reminded_at,
            });
            (true, ())
        })
        .expect("record the reminder");
    let at = delivered + 3 * HOUR;
    let eligible = |found: &ReminderCandidates| {
        found
            .eligible(&record, at)
            .into_iter()
            .map(|candidate| candidate.key)
            .collect::<Vec<_>>()
    };
    // Without the sweep, the other one would still be due a reminder at that time.
    assert_eq!(eligible(&before), std::slice::from_ref(&unreminded));

    let mut logged = Vec::new();
    sweep(&state, at, &mut |line| logged.push(line));
    // The older request retires at once. The newer one holds the cursor of an uncommitted
    // admission, so it closes but stays in the store: closed is enough to stop its reminders.
    assert!(state
        .retired_key(&reminded)
        .expect("retired guard")
        .is_some());
    assert!(
        state
            .read_request(&unreminded)
            .expect("closed request")
            .reply_closed
    );
    let after = state.reply_reminder_candidates().expect("candidates");
    assert!(after.candidates.is_empty(), "{:?}", after.candidates);
    assert!(!after.retained.contains(&reminded));
    assert!(eligible(&after).is_empty());
    // The next write of the record drops the retired request it had reminded.
    let (pruned, ()) = state
        .update_reply_reminders(at, Some(&after.retained), |_| (true, ()))
        .expect("prune the record");
    assert!(!pruned.reminded(&reminded), "{:?}", pruned.reminded);
    // A restart reads the same store: nothing to remind, then or later.
    let reopened = BridgeState::open(&root).expect("reopen");
    let found = reopened
        .reply_reminder_candidates()
        .expect("candidates after a restart");
    assert!(found.candidates.is_empty(), "{:?}", found.candidates);
    assert!(eligible(&found).is_empty());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn the_closure_outbox_merges_batches_past_its_bound_and_never_drops_one() {
    // #2430 C1: past 64 untold batches the oldest was dropped and only counted, so its closed
    // requests were never told to the agent or the owner. The two oldest are now merged, under
    // a new ID, and every closed request is still listed.
    let (state, _key, _root) = delivered_request("window-outbox-bound");
    let request = |n: u64| ClosedRequest {
        key: format!("bound-key-{n}"),
        reply_id: format!("{n}"),
        message_number: None,
        first_line: format!("request {n}"),
        sender_id: "U0OWNER0001".to_owned(),
        from_owner: true,
        admitted_at_millis: n,
        unknown: crate::forward_compat::KeptKeys::default(),
    };
    let mut first_ids = Vec::new();
    for n in 0..70_u64 {
        state
            .append_closure_batch_locked(ClosureCause::Age, 1_000 + n, 0, vec![request(n)])
            .expect("append");
        if n < 2 {
            first_ids.push(
                state.closure_batches().expect("outbox").0[n as usize]
                    .id
                    .clone(),
            );
        }
    }
    let (batches, dropped) = state.closure_batches().expect("outbox");
    assert_eq!(dropped, 0, "nothing dropped");
    assert!(batches.len() <= 64, "{}", batches.len());
    assert_eq!(
        listed_keys(&batches).len(),
        70,
        "every closed request is still listed"
    );
    let oldest = &batches[0];
    assert_eq!(
        oldest.requests[0].key, "bound-key-0",
        "oldest requests first"
    );
    assert!(
        !first_ids.contains(&oldest.id),
        "a merged batch has a new ID"
    );
    assert!(!oldest.agent_notified && !oldest.owner_notified && !oldest.logged);
}
