//! `#452 slack-live-ingest`: a Slack event-stream record that carries a message is published at
//! once, without reading the channel again, and a later read of the same message is the same
//! message. The relist then finds the edits and deletions the stream never carries.
//!
//! Driven through the server's own `/api/v1/live/tail` route against the loopback Slack fake,
//! whose call counters show which provider reads each step made.

mod slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::live::{poll_once, LiveKind, LiveMessage, Published};
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{READ_TOKEN, WRITE_TOKEN};

const CH: &str = "C0000MAIN1";
const OTHER: &str = "C0000OTHR2";
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";
const LIMIT: u16 = 50;

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// A canonical `ts` `offset` seconds from now, with a microsecond fraction.
fn now_ts(offset: i64, micros: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    format!(
        "{}.{micros:06}",
        i64::try_from(now).expect("a sane clock") + offset
    )
}

fn id_of(ts: &str) -> MessageId {
    vibe_talk::slack::message_id_from_ts(ts).expect("an id")
}

/// Five messages a minute apart ending two minutes ago, and Ada's `users.info`.
fn fixture() -> FakeState {
    let mut state = FakeState::default();
    let list: Vec<Value> = (0..5)
        .map(|i| {
            profiled(
                &now_ts(-420 + i * 60, 0),
                "UADA00001",
                "Ada",
                &format!("message {i}"),
            )
        })
        .collect();
    state.history.insert(CH.to_owned(), list);
    state.users.insert(
        "UGRACE001".to_owned(),
        json!({"id": "UGRACE001", "name": "grace", "profile": {"display_name": "Grace"}}),
    );
    state
}

fn deployment(fake: &Fake, extra: &str) -> AppState {
    deployment_with(fake, extra, "")
}

fn deployment_with(fake: &Fake, extra: &str, channels: &str) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[ingest]
token = "{INGEST_TOKEN}"
{extra}

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
live_poll_seconds = 3600

[[channels]]
id = "{CH}"
label = "team"
writable = true
{channels}
"#,
        base = fake.base,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

/// A `message.created` record as the stream writes it.
fn record(seq: u64, ts: &str, thread_ts: Option<&str>, user: &str, text: &str) -> Value {
    json!({
        "type": "message.created",
        "iris_seq_id": seq,
        "payload": {
            "container_id": CH,
            "slack_message": {
                "channel_id": CH,
                "message_ts": ts,
                "thread_ts": thread_ts,
                "slack_user_id": user,
                "bot_id": null,
                "app_id": null,
                "subtype": null,
                "text": text,
                "team_id": "T0000TEAM",
            },
        },
    })
}

async fn send(
    state: &AppState,
    method: &str,
    token: Option<&str>,
    content_type: &str,
    body: Vec<u8>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri("/api/v1/live/tail")
        .header("content-type", content_type);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request.body(Body::from(body)).expect("request"))
        .await
        .expect("the router answers");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn post(state: &AppState, payload: &Value) -> (StatusCode, Value) {
    send(
        state,
        "POST",
        Some(INGEST_TOKEN),
        "application/json",
        payload.to_string().into_bytes(),
    )
    .await
}

/// Everything the hub's replay tail holds for the channel, oldest first.
fn published(state: &AppState) -> Vec<LiveMessage> {
    state.live.subscribe(&channel(), None).replay
}

/// One relist pass, with no settling margin: every fixture message is older than its read.
async fn relist(state: &AppState) -> vibe_talk::relist::RelistPass {
    vibe_talk::relist::relist_once(
        state,
        None,
        vibe_talk::relist::RelistOptions {
            limit: LIMIT,
            threads: 4,
            interval: Duration::from_secs(60),
            settle: Duration::ZERO,
        },
        jiff::Timestamp::now().as_millisecond(),
    )
    .await
}

/// Seed the poller as its first tick does, and return its cursor.
async fn seeded(state: &AppState) -> Option<u64> {
    let mut cursor = None;
    poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the seed reads");
    cursor
}

#[tokio::test]
async fn a_pushed_record_reaches_the_live_stream_without_a_provider_read() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    let mut cursor = seeded(&state).await;
    let history_reads = fake.calls("conversations.history");

    let ts = now_ts(0, 100);
    let (status, body) = post(
        &state,
        &record(41, &ts, None, "UGRACE001", "deploy &amp; <@UADA00001> look"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["outcome"], "published");

    // Published with no read of the channel at all: only the author's name was looked up.
    assert_eq!(fake.calls("conversations.history"), history_reads);
    assert_eq!(fake.calls("conversations.replies"), 0);
    let events = published(&state);
    assert_eq!(events.len(), 1, "{events:?}");
    let live = &events[0];
    assert_eq!(live.kind, LiveKind::Create);
    assert_eq!(
        live.event_id,
        id_of(&ts).as_str(),
        "the poller's own event id"
    );
    assert_eq!(live.message.id, id_of(&ts));
    assert_eq!(live.message.author, "Grace");
    assert_eq!(live.message.author_id.0, "UGRACE001");
    assert_eq!(live.message.content, "deploy & <@UADA00001> look");
    assert!(
        !live.message.spoken_content.is_empty(),
        "the body a voice says"
    );
    let thread = live.message.thread.as_ref().expect("a thread");
    assert!(thread.is_root);
    assert_eq!(thread.id, ts);

    // The poller's walk forward then reads the same message, and publishes nothing new.
    fake.with(|fake| {
        fake.history
            .get_mut(CH)
            .expect("history")
            .push(json!({"type": "message", "ts": ts, "user": "UGRACE001",
                "text": "deploy &amp; <@UADA00001> look"}));
    });
    let tick = poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the walk reads");
    assert_eq!((tick.published, tick.changed), (0, 0), "{tick:?}");
    assert_eq!(published(&state).len(), 1);

    // A retry of the same record is acknowledged as known, and changes nothing.
    let (status, body) = post(
        &state,
        &record(41, &ts, None, "UGRACE001", "deploy &amp; <@UADA00001> look"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "known");
    assert_eq!(published(&state).len(), 1);
}

#[tokio::test]
async fn a_read_that_says_more_than_the_push_goes_out_as_an_update_and_a_read_first_wins() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    let mut cursor = seeded(&state).await;

    // Pushed: the stream carries no file. The read that follows shares one, so the page is
    // given the read's copy as an update, never a second create.
    let ts = now_ts(0, 200);
    post(&state, &record(50, &ts, None, "UGRACE001", "the report")).await;
    fake.with(|fake| {
        let file = slack_fake::file("F0001", "report.pdf", "PDF", "http://files.invalid/");
        fake.history
            .get_mut(CH)
            .expect("history")
            .push(slack_fake::file_post(
                &ts,
                "UGRACE001",
                "Grace",
                "the report",
                vec![file],
            ));
    });
    let tick = poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the walk reads");
    assert_eq!((tick.published, tick.changed), (0, 1), "{tick:?}");
    let events = published(&state);
    assert_eq!(
        events.iter().map(|live| live.kind).collect::<Vec<_>>(),
        [LiveKind::Create, LiveKind::Update]
    );
    assert!(events[1].message.content.contains("report.pdf"));
    assert_ne!(events[1].event_id, events[0].event_id);

    // Read first: the poller publishes it, and the record arriving late is known.
    let late = now_ts(1, 300);
    fake.with(|fake| {
        fake.history.get_mut(CH).expect("history").push(profiled(
            &late,
            "UGRACE001",
            "Grace",
            "quick one",
        ));
    });
    let tick = poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the walk reads");
    assert_eq!(tick.published, 1);
    let (_, body) = post(&state, &record(51, &late, None, "UGRACE001", "quick one")).await;
    assert_eq!(body["outcome"], "known");
    assert_eq!(published(&state).len(), 3);
}

#[tokio::test]
async fn a_replayed_record_at_or_behind_the_seed_is_history_and_costs_no_lookup() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");

    // Before any seed, only a message this recent is news.
    let old = now_ts(-3_600, 0);
    let (status, body) = post(&state, &record(1, &old, None, "UGRACE001", "an hour ago")).await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::OK, json!("history"))
    );

    // After the seed, anything at or behind it is history: here the newest seeded message.
    seeded(&state).await;
    let seeded_ts = fake.with(|fake| {
        fake.history[CH]
            .last()
            .and_then(|message| message["ts"].as_str())
            .expect("a newest message")
            .to_owned()
    });
    let lookups = fake.calls("users.info");
    let (_, body) = post(
        &state,
        &record(2, &seeded_ts, None, "UGRACE001", "message 4"),
    )
    .await;
    assert_eq!(body["outcome"], "history");
    assert_eq!(
        fake.calls("users.info"),
        lookups,
        "history is told by its id alone"
    );
    assert!(published(&state).is_empty());
}

#[tokio::test]
async fn a_pushed_reply_is_published_as_a_reply_and_marks_its_thread_active() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    seeded(&state).await;
    let root = fake.with(|fake| fake.history[CH][4]["ts"].as_str().expect("ts").to_owned());
    let ts = now_ts(0, 400);
    let (status, body) = post(&state, &record(60, &ts, Some(&root), "UGRACE001", "on it")).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let events = published(&state);
    let thread = events[0].message.thread.as_ref().expect("a thread");
    assert_eq!((thread.id.as_str(), thread.is_root), (root.as_str(), false));
    let now = jiff::Timestamp::now().as_millisecond();
    assert_eq!(state.live.active_threads(&channel(), now - 60_000), [root]);
    // A reply read apart is never offered for clearing unread, as the poller's own are not.
    assert_eq!(state.live.upstream_clear().forget_all(), 0);
}

#[tokio::test]
async fn records_are_checked_and_authenticated_before_anything_is_published() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    let good = record(70, &now_ts(0, 500), None, "UGRACE001", "fine");

    let (status, _) = send(
        &state,
        "POST",
        None,
        "application/json",
        good.to_string().into(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &state,
        "POST",
        Some(WRITE_TOKEN),
        "application/json",
        good.to_string().into(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a browser token is not the adapter's"
    );
    let (status, _) = send(
        &state,
        "POST",
        Some(INGEST_TOKEN),
        "text/plain",
        good.to_string().into(),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let oversized = vec![b' '; vibe_talk::slack::tail::MAX_RECORD_BYTES + 1];
    let (status, _) = send(
        &state,
        "POST",
        Some(INGEST_TOKEN),
        "application/json",
        oversized,
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let mut bad_user = good.clone();
    bad_user["payload"]["slack_message"]["slack_user_id"] = json!("<script>");
    let (status, body) = post(&state, &bad_user).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(!body.to_string().contains("<script>"), "{body}");

    let mut elsewhere = good.clone();
    elsewhere["payload"]["container_id"] = json!(OTHER);
    elsewhere["payload"]["slack_message"]["channel_id"] = json!(OTHER);
    let (status, body) = post(&state, &elsewhere).await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::OK, json!("unlisted"))
    );

    let (status, body) = post(&state, &json!({"type": "HEARTBEAT"})).await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::OK, json!("ignored"))
    );
    assert!(published(&state).is_empty());
}

#[tokio::test]
async fn a_gap_relists_every_fed_channel_and_a_reaction_wakes_the_poller() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");

    let (status, body) = post(
        &state,
        &json!({"type": "GAP", "lost_from_iris_seq_id": 3, "lost_to_iris_seq_id": 9}),
    )
    .await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::ACCEPTED, json!("gap"))
    );
    assert_eq!(
        state
            .live
            .take_relist_requests()
            .into_iter()
            .collect::<Vec<_>>(),
        [channel()]
    );

    let (status, body) = post(
        &state,
        &json!({"type": "reaction.created", "iris_seq_id": 10,
            "payload": {"container_id": CH, "slack_reaction": {"name": "eyes"}}}),
    )
    .await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::ACCEPTED, json!("hinted"))
    );
    assert!(
        !state.live.queue_hint(None, &channel()),
        "the reaction already queued a read of the channel"
    );
}

#[tokio::test]
async fn an_adapter_asks_which_conversations_to_tail_under_its_own_credential() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment_with(
        &fake,
        "",
        &format!("\n[[channels]]\nid = \"{OTHER}~1700000000.000100\"\nlabel = \"one thread\"\n"),
    );
    let (status, _) = send(&state, "GET", None, "application/json", Vec::new()).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send(
        &state,
        "GET",
        Some(INGEST_TOKEN),
        "application/json",
        Vec::new(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["conversations"], json!([CH, OTHER]));
}

#[tokio::test]
async fn the_relist_publishes_the_edit_and_the_deletion_the_stream_never_carried() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "relist_seconds = 60");
    seeded(&state).await;
    let (kept, gone) = (now_ts(0, 600), now_ts(0, 700));
    for (seq, ts, text) in [(80, &kept, "first draft"), (81, &gone, "oops")] {
        post(&state, &record(seq, ts, None, "UGRACE001", text)).await;
        fake.with(|fake| {
            fake.history
                .get_mut(CH)
                .expect("history")
                .push(json!({"type": "message", "ts": ts, "user": "UGRACE001", "text": text}));
        });
    }
    // Upstream, the first is edited and the second deleted. Neither is in the stream.
    fake.with(|fake| {
        let list = fake.history.get_mut(CH).expect("history");
        list.retain(|message| message["ts"] != json!(gone));
        let edited = list
            .iter_mut()
            .find(|message| message["ts"] == json!(kept))
            .expect("the kept message");
        edited["text"] = json!("final text");
        edited["edited"] = json!({"user": "UGRACE001", "ts": now_ts(0, 900)});
    });
    let pass = relist(&state).await;
    assert_eq!(
        (pass.reads, pass.failed, pass.updates, pass.deletes),
        (1, 0, 1, 1),
        "{pass:?}"
    );
    let events = published(&state);
    let changes: Vec<(LiveKind, &str)> = events[2..]
        .iter()
        .map(|live| (live.kind, live.message.id.as_str()))
        .collect();
    assert_eq!(
        changes,
        [
            (LiveKind::Update, id_of(&kept).as_str()),
            (LiveKind::Delete, id_of(&gone).as_str())
        ]
    );
    assert_eq!(events[2].message.content, "final text");

    // A second pass finds nothing more, and a late copy of the deleted message is not news.
    let again = relist(&state).await;
    assert_eq!((again.updates, again.deletes), (0, 0), "{again:?}");
    let (_, body) = post(&state, &record(81, &gone, None, "UGRACE001", "oops")).await;
    assert_eq!(body["outcome"], "known");
}

#[tokio::test]
async fn the_relist_reads_a_recently_active_thread_for_an_edited_reply() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "relist_seconds = 60");
    seeded(&state).await;
    let root = fake.with(|fake| fake.history[CH][4]["ts"].as_str().expect("ts").to_owned());
    let reply = now_ts(0, 800);
    post(
        &state,
        &record(90, &reply, Some(&root), "UGRACE001", "typo hree"),
    )
    .await;
    fake.with(|fake| {
        slack_fake::add_reply(fake, CH, &root, &reply, "typo here");
        let message = fake
            .replies
            .get_mut(&(CH.to_owned(), root.clone()))
            .and_then(|replies| replies.last_mut())
            .expect("the reply");
        message["user"] = json!("UGRACE001");
        message
            .as_object_mut()
            .expect("object")
            .remove("user_profile");
    });
    let replies_before = fake.calls("conversations.replies");
    let pass = relist(&state).await;
    assert_eq!(
        pass.reads, 2,
        "the newest page and the active thread: {pass:?}"
    );
    assert!(fake.calls("conversations.replies") > replies_before);
    assert_eq!(pass.updates, 1, "{pass:?}");
    let last = published(&state).pop().expect("an update");
    assert_eq!(
        (last.kind, last.message.content.as_str()),
        (LiveKind::Update, "typo here")
    );
}

#[test]
fn a_push_that_loses_the_race_to_a_read_is_known_under_the_feed_lock() {
    // The hub alone: the same message offered by the stream and by a read, in either order,
    // goes out once.
    let hub = vibe_talk::live::LiveHub::new();
    let ch = channel();
    let ts = now_ts(0, 1);
    let message = vibe_talk::model::Message {
        id: id_of(&ts),
        channel_id: ch.clone(),
        author: "Grace".to_owned(),
        author_id: vibe_talk::model::UserId("UGRACE001".to_owned()),
        author_is_bot: false,
        timestamp: "2026-10-23T00:00:00Z".to_owned(),
        spoken_time: String::new(),
        edited_timestamp: None,
        reply_to: None,
        content: "hi".to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
        thread: None,
    };
    let now = jiff::Timestamp::now().as_millisecond();
    assert_eq!(
        hub.publish_pushed(&ch, message.clone(), false, now),
        Published::New
    );
    assert_eq!(
        hub.publish_pushed(&ch, message.clone(), false, now),
        Published::Known
    );
    hub.publish(&ch, message);
    assert_eq!(hub.subscribe(&ch, None).replay.len(), 1);
}

#[tokio::test]
async fn a_message_too_new_for_the_provider_to_list_yet_is_never_taken_for_a_deleted_one() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "relist_seconds = 60");
    seeded(&state).await;
    // Pushed a moment ago, and not listed by the provider yet.
    post(
        &state,
        &record(95, &now_ts(0, 950), None, "UGRACE001", "just now"),
    )
    .await;
    let pass = vibe_talk::relist::relist_once(
        &state,
        None,
        vibe_talk::relist::RelistOptions {
            limit: LIMIT,
            threads: 4,
            interval: Duration::from_secs(60),
            settle: vibe_talk::relist::RELIST_SETTLE,
        },
        jiff::Timestamp::now().as_millisecond(),
    )
    .await;
    assert_eq!((pass.updates, pass.deletes), (0, 0), "{pass:?}");
    assert_eq!(published(&state).len(), 1);
}

#[test]
fn the_records_unit_lets_the_adapter_choose_the_channels_and_the_position() {
    let unit = include_str!("../systemd/vibe-talk-live-tail.service.example");
    let exec_start: Vec<&str> = unit
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("ExecStart="))
        .collect();
    assert_eq!(exec_start.len(), 1, "one literal event source");
    for needed in [
        "live-hint-adapter.py --records ",
        "--state-file ",
        " -- /REPLACE_WITH_VENDOR_CLI conversation tail ",
        "--channel-id={channels}",
        "--catchup={catchup}",
        "--include-iris-seq-id",
        "--output=json",
    ] {
        assert!(exec_start[0].contains(needed), "{needed}");
    }
    assert!(
        !exec_start[0].contains("--channel "),
        "every conversation vibe-talk names is tailed"
    );
    for absent in ["NoNewPrivileges=", "PrivateTmp="] {
        assert!(
            !unit.lines().any(|line| line.trim().starts_with(absent)),
            "the vendor CLI's managed runtime aborts under {absent}"
        );
    }
}
