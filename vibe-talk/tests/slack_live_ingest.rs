//! `#452 slack-live-ingest`: a Slack event-stream record that carries a message is published at
//! once, without reading the channel again, and a later read of the same message is the same
//! message. The relist then finds the edits and deletions the stream never carries.
//!
//! Driven through the server's own `/api/v1/live/tail` route against the loopback Slack fake,
//! whose call counters show which provider reads each step made.

use crate::slack_fake;

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
    // A retry before anything reads it is acknowledged as known.
    let (status, body) = post(
        &state,
        &record(41, &ts, None, "UGRACE001", "deploy &amp; <@UADA00001> look"),
    )
    .await;
    assert_eq!(
        (status, body["outcome"].clone()),
        (StatusCode::OK, json!("known"))
    );

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

    // A retry of the same record now is history: the poller has walked past it. Nothing changes.
    let (status, body) = post(
        &state,
        &record(41, &ts, None, "UGRACE001", "deploy &amp; <@UADA00001> look"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "history");
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
    // The record arriving after the poller walked past its message is history, published to
    // nobody.
    let (_, body) = post(&state, &record(51, &late, None, "UGRACE001", "quick one")).await;
    assert_eq!(body["outcome"], "history");
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

/// A record in the shape the stream was captured writing: the container names the workspace and
/// the conversation, and the payload carries fields this server does not read. `#452
/// slack-live-ingest`, the follow-up to the first deployment, where such a record was refused.
fn captured(seq: u64, ts: &str, slack_extra: Value) -> Value {
    let mut slack = json!({
        "team_id": "T0000TEAM", "channel_id": CH, "message_ts": ts,
        "slack_user_id": "UGRACE001", "slack_event_id": "Ev0001", "app_id": "A0APP000001",
        "text": "a captured shape", "channel_type": "channel", "blocks_json": "[]"
    });
    for (key, value) in slack_extra.as_object().expect("an object") {
        slack[key] = value.clone();
    }
    json!({
        "seq": seq, "ts": 1.0, "type": "message.created", "source": "slack",
        "iris_seq_id": seq, "event_ts": 1_792_637_725_398_u64,
        "payload": {
            "event_id": "evt-0001", "source": "slack", "event_type": "message.created",
            "entity_type": "message", "entity_id": format!("T0000TEAM:{CH}:{ts}"),
            "container_type": "channel", "container_id": format!("T0000TEAM:{CH}"),
            "actor_id": 1, "actor_name": "Grace",
            "context": {"snippet": "a captured shape"},
            "viewer_relevance": {"is_actor_self": false},
            "slack_message": slack
        }
    })
}

#[tokio::test]
async fn the_shapes_the_stream_really_writes_are_published_or_ignored_never_refused() {
    let capture = vibe_talk::testing::LogCapture::info_only();
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    seeded(&state).await;
    let root = fake.with(|fake| fake.history[CH][4]["ts"].as_str().expect("ts").to_owned());
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a message",
            captured(100, &now_ts(0, 1_001), json!({})),
            "published",
        ),
        (
            "a reply",
            captured(101, &now_ts(0, 1_002), json!({"thread_ts": root})),
            "published",
        ),
        (
            "a broadcast",
            captured(
                102,
                &now_ts(0, 1_003),
                json!({"thread_ts": root, "subtype": "thread_broadcast"}),
            ),
            "published",
        ),
        (
            "a bot's post",
            captured(
                103,
                &now_ts(0, 1_004),
                json!({"slack_user_id": null, "bot_id": "B0BOT00001", "subtype": "bot_message"}),
            ),
            "published",
        ),
        (
            "a file share",
            captured(104, &now_ts(0, 1_005), json!({"subtype": "file_share"})),
            "published",
        ),
        (
            "a join",
            captured(105, &now_ts(0, 1_006), json!({"subtype": "channel_join"})),
            "ignored",
        ),
        (
            "a creation with no Slack message",
            json!({"type": "message.created", "iris_seq_id": 106,
                "payload": {"container_id": format!("T0000TEAM:{CH}")}}),
            "ignored",
        ),
        (
            "a reaction",
            json!({"type": "reaction.created", "iris_seq_id": 107,
                "payload": {"event_type": "reaction.created",
                    "container_id": format!("T0000TEAM:{CH}"), "reaction": "eyes"}}),
            "hinted",
        ),
        (
            "a reaction naming no conversation",
            json!({"type": "reaction.deleted", "iris_seq_id": 108, "payload": {}}),
            "ignored",
        ),
        ("a client notice", json!({"type": "SUBSCRIBED"}), "ignored"),
        (
            "another kind",
            json!({"type": "conversation.updated", "iris_seq_id": 109,
                "payload": {"container_id": format!("T0000TEAM:{CH}")}}),
            "ignored",
        ),
    ];
    for (what, record, outcome) in cases {
        let (status, body) = post(&state, &record).await;
        assert!(status.is_success(), "{what}: {status} {body}");
        assert_eq!(body["outcome"], outcome, "{what}: {body}");
    }
    let creates = published(&state)
        .into_iter()
        .filter(|live| live.kind == LiveKind::Create)
        .count();
    assert_eq!(
        creates, 5,
        "every message, reply, broadcast, bot post and file share"
    );
    assert!(
        !capture.text().contains("refused a stream record"),
        "{}",
        capture.text()
    );
}

#[tokio::test]
async fn a_refused_record_is_logged_by_field_and_rule_and_never_by_its_text() {
    let capture = vibe_talk::testing::LogCapture::info_only();
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    let secret = "PRIVATE-WORDS-MUST-NOT-BE-LOGGED";
    let refused = captured(
        200,
        &now_ts(0, 2_001),
        json!({"slack_user_id": secret, "text": secret}),
    );
    let (status, _) = post(&state, &refused).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut elsewhere = captured(201, &now_ts(0, 2_002), json!({"text": secret}));
    elsewhere["payload"]["container_id"] = json!("T0000TEAM:C0000OTHR2");
    let (status, _) = post(&state, &elsewhere).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = send(
        &state,
        "POST",
        Some(INGEST_TOKEN),
        "application/json",
        format!("{{\"text\": \"{secret}").into_bytes(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let logged = capture.text();
    let warnings: Vec<&str> = logged
        .lines()
        .filter(|line| line.contains("WARN") && line.contains("refused a stream record"))
        .collect();
    assert_eq!(warnings.len(), 3, "{logged}");
    assert!(
        warnings[0].contains("field=\"slack_user_id\""),
        "{}",
        warnings[0]
    );
    assert!(
        warnings[0].contains("is not a Slack user id"),
        "{}",
        warnings[0]
    );
    assert!(
        warnings[1].contains("field=\"container_id\""),
        "{}",
        warnings[1]
    );
    assert!(warnings[2].contains("is not valid JSON"), "{}", warnings[2]);
    assert!(!logged.contains(secret), "{logged}");
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
        // As Slack lists it: Grace's own reply, carrying her profile, which names her exactly
        // as the lookup behind the pushed record did.
        message["user"] = json!("UGRACE001");
        message["user_profile"] = json!({"display_name": "Grace", "real_name": "Grace Real"});
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
        upstream_app_url: None,
        op_key: None,
        observed_ms: None,
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
    hub.publish(&ch, message.clone());
    assert_eq!(hub.subscribe(&ch, None).replay.len(), 1);

    // And the other order: read first, then the stream's copy.
    let other = ChannelId(OTHER.to_owned());
    let mut read = message;
    read.channel_id = other.clone();
    hub.publish(&other, read.clone());
    assert_eq!(
        hub.publish_pushed(&other, read, false, now),
        Published::Known
    );
    assert_eq!(hub.subscribe(&other, None).replay.len(), 1);
}

#[tokio::test]
async fn a_stream_replaying_past_the_hubs_memory_never_announces_what_the_poller_walked_past() {
    let fake = Fake::start(fixture(), 20).await;
    let state = deployment(&fake, "");
    let mut cursor = seeded(&state).await;
    // A message the poller reads and publishes, then more than the hub remembers creating.
    let walked = now_ts(-60, 10);
    fake.with(|fake| {
        fake.history.get_mut(CH).expect("history").push(profiled(
            &walked,
            "UGRACE001",
            "Grace",
            "walked past",
        ));
    });
    poll_once(
        state.chat.as_ref(),
        &state.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("the walk reads");
    let hub = &state.live;
    let now = jiff::Timestamp::now().as_millisecond();
    for n in 0..=vibe_talk::live::CREATED_MEMORY {
        let ts = now_ts(-50, i64::try_from(n).expect("small"));
        let message = vibe_talk::model::Message {
            id: id_of(&ts),
            ..published(&state)[0].message.clone()
        };
        hub.publish_pushed(&channel(), message, false, now);
    }
    // The stream resumes from an hour-old position and replays the walked message: history,
    // although the hub no longer remembers creating it.
    let (_, body) = post(
        &state,
        &record(3, &walked, None, "UGRACE001", "walked past"),
    )
    .await;
    assert_eq!(body["outcome"], "history");
    // A message older than the stale bound is history whatever the poller has walked.
    let stale = now_ts(-20 * 60, 0);
    assert!(hub.is_history(&channel(), &id_of(&stale), true, now));
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

/// `#452 slack-live-ingest` and `#453 cache-backfill`, measured against the loopback Slack fake:
/// the backfill's reads and its yielding, the shadow's servable rate for the newest timeline page
/// with and without the relist and the backfill, and how long a message takes from the stream to
/// the live hub by a content-free hint and by a pushed record.
///
/// The functional tests run in the ordinary suite. The two measurements print their figures and
/// are `#[ignore]`d, because they take seconds of wall-clock time and their numbers, not a pass,
/// are the point: `cargo test --test integration slack_live_ingest::measure -- --ignored --nocapture`.
mod measure {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::slack_fake::{profiled, Fake, FakeState, TOKEN};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt as _;
    use serde_json::{json, Value};
    use tower::ServiceExt as _;
    use vibe_talk::backfill::{Backfill, BackfillStep};
    use vibe_talk::model::ChannelId;
    use vibe_talk::providers::ChatRouter;
    use vibe_talk::relist::RelistOptions;
    use vibe_talk::state::AppState;
    use vibe_talk::testing::{self, ManualClock, TempDir, READ_TOKEN, WRITE_TOKEN};

    const CH: &str = "C0000MAIN1";
    const INGEST_TOKEN: &str = "test-ingest-token-0000000000";
    const LIMIT: u16 = 50;
    const WAIT: Duration = Duration::from_secs(20);

    fn channel() -> ChannelId {
        ChannelId(CH.to_owned())
    }

    fn wall_secs() -> i64 {
        i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_secs(),
        )
        .expect("a sane clock")
    }

    /// `count` messages a minute apart, the newest `newest_ago` seconds ago.
    fn history(count: i64, newest_ago: i64) -> FakeState {
        let mut state = FakeState::default();
        let base = wall_secs() - newest_ago - (count - 1) * 60;
        let list: Vec<Value> = (0..count)
            .map(|i| {
                profiled(
                    &format!("{}.000000", base + i * 60),
                    "UADA00001",
                    "Ada",
                    &format!("message {i}"),
                )
            })
            .collect();
        state.history.insert(CH.to_owned(), list);
        state
    }

    /// [`history`], with every `every`th message a system event the client never shows: a
    /// channel's history as Slack lists it, so that a page of `limit` entries holds fewer
    /// messages than that.
    fn history_with_joins(count: i64, newest_ago: i64, every: i64) -> FakeState {
        let mut state = history(count, newest_ago);
        for (i, entry) in state
            .history
            .get_mut(CH)
            .expect("history")
            .iter_mut()
            .enumerate()
        {
            if i64::try_from(i).expect("small") % every == every - 1 {
                entry["subtype"] = json!("channel_join");
            }
        }
        state
    }

    fn config(fake: &Fake, cache_table: &str, poll_seconds: u64) -> String {
        format!(
            r#"
    [auth]
    read_token = "{READ_TOKEN}"
    write_token = "{WRITE_TOKEN}"

    [ingest]
    token = "{INGEST_TOKEN}"

    [[providers]]
    key = "slack"
    kind = "slack"
    api_base = "{base}"
    token = "{TOKEN}"
    owner_user_id = "UOWNER001"
    live_poll_seconds = {poll_seconds}

    [[channels]]
    id = "{CH}"
    label = "team"
    writable = true
    {cache_table}
    "#,
            base = fake.base,
        )
    }

    fn deployment(fake: &Fake, cache_table: &str, poll_seconds: u64) -> AppState {
        let (mut state, _discord, _store) =
            testing::state_with_store_from_toml(&config(fake, cache_table, poll_seconds));
        let router = ChatRouter::from_config(&state.config).expect("providers build");
        state.replace_providers(Arc::new(router));
        state
    }

    async fn cached(fake: &Fake, tag: &str, table: &str) -> (AppState, Arc<ManualClock>, TempDir) {
        let dir = TempDir::new(tag);
        let clock = ManualClock::at(wall_secs() * 1_000);
        let state = deployment(fake, table, 3600);
        let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
        (state, clock, dir)
    }

    fn held(state: &AppState) -> u64 {
        let cache = state.message_cache.cache().expect("a cache");
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
        cache.counts().served().expect("counts").messages
    }

    async fn request(
        state: &AppState,
        method: &str,
        uri: &str,
        token: &str,
        body: &Value,
    ) -> StatusCode {
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", format!("Bearer {token}"));
        let body = if method == "POST" {
            builder = builder.header("content-type", "application/json");
            Body::from(body.to_string())
        } else {
            Body::empty()
        };
        let response = vibe_talk::http::router(state.clone())
            .oneshot(builder.body(body).expect("request"))
            .await
            .expect("the router answers");
        let status = response.status();
        let _ = response.into_body().collect().await;
        status
    }

    fn record(seq: u64, ts: &str, text: &str) -> Value {
        json!({
            "type": "message.created",
            "iris_seq_id": seq,
            "payload": {"container_id": CH, "slack_message": {
                "channel_id": CH, "message_ts": ts, "slack_user_id": "UADA00001", "text": text}},
        })
    }

    /// Add one message to the fake now, and return its `ts`.
    fn arrive(fake: &Fake, n: usize, micros: i64) -> String {
        let ts = format!("{}.{micros:06}", wall_secs());
        fake.with(|fake| {
            fake.history.get_mut(CH).expect("history").push(profiled(
                &ts,
                "UADA00001",
                "Ada",
                &format!("arrival {n}"),
            ));
        });
        ts
    }

    #[tokio::test]
    async fn the_backfill_walks_older_pages_into_the_cache_and_off_makes_no_call() {
        let fake = Fake::start(history(120, 60), 20).await;
        let (state, _clock, _dir) = cached(
            &fake,
            "backfill-walk",
            "\n[message_cache]\nbackfill = \"adaptive\"\n",
        )
        .await;
        let before = fake.calls("conversations.history");
        let mut backfill = Backfill::new();
        let mut reads = 0;
        loop {
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => reads += 1,
                BackfillStep::Idle => break,
                other => panic!("unexpected step {other:?}"),
            }
            assert!(
                reads <= vibe_talk::backfill::BACKFILL_PAGES,
                "the walk ends"
            );
        }
        // Pages of 50 rows (each read through as many 20-row bridge calls as it takes): three hold
        // all 120 messages, and the third says nothing is older.
        assert_eq!(reads, 3);
        assert!(fake.calls("conversations.history") - before >= reads);
        assert_eq!(held(&state), 120);
        // The walk as a whole is one listing: Main is covered from the oldest message it read up to
        // the newest end, older pages included, not only the newest page.
        let oldest_ms = fake.with(|fake| {
            let ts = fake.history[CH][0]["ts"].as_str().expect("ts").to_owned();
            vibe_talk::slack::message_id_from_ts(&ts)
                .and_then(|id| id.created_at_ms())
                .expect("a time")
        });
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        let covered = cache
            .coverage(&chan, &vibe_talk::cache::Scope::Main)
            .served()
            .expect("coverage");
        assert!(
            covered.iter().any(|interval| interval.t0_ms <= oldest_ms),
            "the walk's span is covered down to its oldest message: {covered:?}"
        );
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Idle
        );
        backfill.rewalk();
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Read
        );

        // Off: no call at all.
        let off_fake = Fake::start(history(10, 60), 20).await;
        let (off, _clock, _dir) = cached(
            &off_fake,
            "backfill-off",
            "\n[message_cache]\nbackfill = \"off\"\n",
        )
        .await;
        let calls = off_fake.calls("conversations.history");
        assert_eq!(
            Backfill::new().step(&off, LIMIT, Duration::ZERO).await,
            BackfillStep::Off
        );
        assert_eq!(off_fake.calls("conversations.history"), calls);
    }

    /// `#502 cache-serve-coverage`. A walk that is over, whose channel the cache has erased since
    /// it began, proved nothing that is still held: it walks again at once, not six hours on.
    #[tokio::test]
    async fn a_walk_over_before_an_erase_of_its_channel_walks_again() {
        let fake = Fake::start(history(60, 60), 20).await;
        let (state, _clock, _dir) = cached(
            &fake,
            "backfill-erased",
            "\n[message_cache]\nbackfill = \"adaptive\"\n",
        )
        .await;
        let mut backfill = Backfill::new();
        while backfill.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Idle
        );
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        cache.erase_channels(std::slice::from_ref(&chan));
        assert!(cache.quiesce_blocking(WAIT));
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Read,
            "the erased channel's walk starts over"
        );
        while backfill.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
        assert!(cache.quiesce_blocking(WAIT));
        assert_eq!(held(&state), 60, "the walk filled the channel again");
    }

    #[tokio::test]
    async fn the_backfill_yields_while_anything_else_reads_the_provider() {
        let mut fixture = history(10, 60);
        fixture.delay = Some(Duration::from_millis(600));
        let fake = Fake::start(fixture, 20).await;
        let (state, _clock, _dir) = cached(
            &fake,
            "backfill-yield",
            "\n[message_cache]\nbackfill = \"adaptive\"\n",
        )
        .await;
        // A page's read is in flight: the backfill does not add its own.
        let reader = {
            let state = state.clone();
            tokio::spawn(async move {
                request(
                    &state,
                    "GET",
                    &format!("/api/v1/channels/{CH}/timeline?view=main&limit=50"),
                    WRITE_TOKEN,
                    &Value::Null,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut backfill = Backfill::new();
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::from_secs(2)).await,
            BackfillStep::Busy
        );
        assert_eq!(reader.await.expect("joined"), StatusCode::OK);
        // Just after it, still busy: the provider has not been idle for the margin yet.
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::from_secs(2)).await,
            BackfillStep::Busy
        );
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Read
        );
    }

    // ---- `#595 cache-full-history` -------------------------------------------------------------

    const FULL: &str = "\n[message_cache]\nbackfill = \"full\"\nfull_reads_per_minute = 50\n";

    /// `count` messages `spacing` seconds apart, the newest a minute ago: a long history.
    fn long_history(count: i64, spacing: i64) -> FakeState {
        let mut state = FakeState::default();
        let base = wall_secs() - 60 - (count - 1) * spacing;
        let list: Vec<Value> = (0..count)
            .map(|i| {
                profiled(
                    &format!("{}.000000", base + i * spacing),
                    "UADA00001",
                    "Ada",
                    &format!("message {i}"),
                )
            })
            .collect();
        state.history.insert(CH.to_owned(), list);
        state
    }

    fn frontier(state: &AppState) -> Option<vibe_talk::cache::Frontier> {
        let cache = state.message_cache.cache().expect("a cache");
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        cache.frontier(&chan).served().expect("served")
    }

    /// Step until the full walk has read `pages` pages, the clock moved past each read's pace.
    async fn full_pages(
        backfill: &mut Backfill,
        state: &AppState,
        clock: &ManualClock,
        pages: usize,
    ) -> usize {
        let mut read = 0;
        let mut steps = 0;
        while read < pages {
            steps += 1;
            assert!(steps < 10_000, "the walk stalled");
            match backfill.step(state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => read += 1,
                BackfillStep::Waiting => clock.advance(1_200),
                BackfillStep::Idle => break,
                other => panic!("unexpected step {other:?}"),
            }
        }
        read
    }

    /// The full walk reads a channel's whole history, page after page back to its beginning, with
    /// no age limit; where it stands is kept in the cache file, so a backfill begun afresh (a
    /// restart) goes on from there rather than from the newest page; and once it reaches the
    /// beginning the channel is complete, Main covered from the scope's start.
    #[tokio::test]
    async fn the_full_walk_reads_a_long_history_to_its_beginning_and_resumes_after_a_restart() {
        // 1,000 messages three hours apart: 125 days, ten pages of 100.
        let fake = Fake::start(long_history(1_000, 3 * 3600), 100).await;
        let (state, clock, _dir) = cached(&fake, "full-walk", FULL).await;
        let mut backfill = Backfill::new();
        // The recent walk comes first: back to fourteen days, three pages of 50.
        for _ in 0..3 {
            assert_eq!(
                backfill.step(&state, LIMIT, Duration::ZERO).await,
                BackfillStep::Read
            );
        }
        assert_eq!(frontier(&state), None, "the full walk has not begun");
        assert_eq!(full_pages(&mut backfill, &state, &clock, 4).await, 4);
        let stopped = frontier(&state).expect("a record");
        assert_eq!(stopped.cycle_pages, 4);
        assert!(stopped.cursor.is_some());
        assert_eq!(stopped.complete_ms, None);
        // A restart: a backfill that remembers nothing goes on from the record.
        let mut backfill = Backfill::new();
        for _ in 0..3 {
            assert_eq!(
                backfill.step(&state, LIMIT, Duration::ZERO).await,
                BackfillStep::Read,
                "a start walks the recent days again"
            );
        }
        let before = fake.calls("conversations.history");
        assert_eq!(full_pages(&mut backfill, &state, &clock, 1).await, 1);
        let resumed = frontier(&state).expect("a record");
        assert_eq!(
            resumed.cycle_pages, 5,
            "the walk went on, not from the newest page"
        );
        assert_eq!(resumed.cycle_started_ms, stopped.cycle_started_ms);
        assert!(resumed.below_ms < stopped.below_ms, "it read further back");
        assert!(
            fake.calls("conversations.history") - before <= 2,
            "one page: the row past it that says more remain may take a second call"
        );
        // On to the beginning: five more pages, the last of them saying nothing is older.
        assert_eq!(full_pages(&mut backfill, &state, &clock, 5).await, 5);
        let done = frontier(&state).expect("a record");
        assert_eq!(done.cursor, None);
        assert_eq!(done.cycle_pages, 10);
        assert_eq!(done.complete_pages, Some(10));
        assert!(done.complete_ms.is_some());
        assert_eq!(held(&state), 1_000, "nothing was too old to keep");
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        let covered = cache
            .coverage(&chan, &vibe_talk::cache::Scope::Main)
            .served()
            .expect("coverage");
        assert_eq!(covered.first().map(|i| i.t0_ms), Some(0), "{covered:?}");
        assert!(
            covered
                .windows(2)
                .all(|pair| pair[0].t1_ms >= pair[1].t0_ms),
            "the walk's pages join into one stretch: {covered:?}"
        );
        let history = vibe_talk::backfill::history(&state).expect("a report");
        assert_eq!(history.mode, "full");
        assert_eq!(history.channels.len(), 1);
        let channel_history = &history.channels[0];
        assert!(channel_history.complete);
        assert!(!channel_history.walking);
        assert_eq!(channel_history.label.as_deref(), Some("team"));
        assert_eq!(channel_history.messages, 1_000);
        // Nothing more is due until the next walk, `full_cycle_hours` after this one began.
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Idle
        );
        clock.advance(72 * 3600 * 1000);
        let mut walked = 0;
        loop {
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => walked += 1,
                BackfillStep::Waiting => clock.advance(1_200),
                BackfillStep::Idle => break,
                other => panic!("unexpected step {other:?}"),
            }
            if frontier(&state).is_some_and(|f| f.cycle_pages == 1 && f.cursor.is_some()) {
                break;
            }
        }
        assert!(walked >= 1, "the next walk began");
        let next = frontier(&state).expect("a record");
        assert_eq!(next.cycle_pages, 1, "from the newest page");
        assert_eq!(
            next.complete_pages,
            Some(10),
            "keeping what the last walk took"
        );
        assert!(
            vibe_talk::backfill::history(&state)
                .expect("a report")
                .channels[0]
                .complete,
            "still complete: the last walk to reach the beginning began less than a week ago"
        );
    }

    /// Each thread the full walk finds is read whole, a long one page by page, before it steps
    /// back again; its reads keep to `full_reads_per_minute` of the provider; and like every
    /// backfill read they wait while anything else reads the provider.
    #[tokio::test]
    async fn the_full_walk_reads_threads_whole_at_its_pace_and_yields() {
        let mut fixture = long_history(250, 3600);
        let mut list = fixture.history.remove(CH).expect("history");
        for (index, count) in [(10_usize, 3_usize), (120, 150), (240, 2)] {
            let root_secs: i64 = list[index]["ts"]
                .as_str()
                .expect("ts")
                .split('.')
                .next()
                .expect("seconds")
                .parse()
                .expect("a number");
            let replies: Vec<(String, String)> = (0..count)
                // 170 seconds apart: settled below the newest one, and never at the instant of a
                // message of the channel's own, an hour apart.
                .map(|n| {
                    let at = root_secs + 170 * (i64::try_from(n).expect("small") + 1);
                    (format!("{at}.000000"), format!("reply {n}"))
                })
                .collect();
            super::slack_fake::thread(&mut fixture, CH, &mut list[index], &replies);
        }
        fixture.history.insert(CH.to_owned(), list);
        let fake = Fake::start(fixture, 100).await;
        let table = "\n[message_cache]\nbackfill = \"full\"\nfull_reads_per_minute = 6\n";
        let (state, clock, _dir) = cached(&fake, "full-threads", table).await;
        let mut backfill = Backfill::new();
        // The recent walk: back to fourteen days, which is all 250 hours of it, in five pages.
        let mut recent = 0;
        while recent < 20 {
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => recent += 1,
                _ => break,
            }
            if frontier(&state).is_some() {
                break;
            }
        }
        // Pace: one full read, then nothing for ten seconds of the provider's clock.
        let replies_before = fake.calls("conversations.replies");
        let mut paced = 0;
        while frontier(&state).is_none() || paced == 0 {
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => paced += 1,
                BackfillStep::Waiting => break,
                other => panic!("unexpected step {other:?}"),
            }
        }
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Waiting,
            "a second full read at once"
        );
        clock.advance(9_000);
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Waiting,
            "a second full read nine seconds on"
        );
        clock.advance(1_000);
        // Yielding: while a page's read is in flight, the full walk adds none of its own.
        fake.with(|fake| fake.delay = Some(Duration::from_millis(600)));
        let reader = {
            let state = state.clone();
            tokio::spawn(async move {
                request(
                    &state,
                    "GET",
                    &format!("/api/v1/channels/{CH}/timeline?view=main&limit=50"),
                    WRITE_TOKEN,
                    &Value::Null,
                )
                .await
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::from_secs(2)).await,
            BackfillStep::Busy
        );
        assert_eq!(reader.await.expect("joined"), StatusCode::OK);
        fake.with(|fake| fake.delay = None);
        // On to the beginning, every thread read whole on the way.
        let mut steps = 0;
        while frontier(&state).is_none_or(|f| f.complete_ms.is_none()) {
            steps += 1;
            assert!(steps < 10_000, "the walk stalled");
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Read => {}
                BackfillStep::Waiting => clock.advance(10_000),
                other => panic!("unexpected step {other:?}"),
            }
        }
        assert!(
            fake.calls("conversations.replies") - replies_before >= 4,
            "three threads, the long one in two pages"
        );
        assert_eq!(
            held(&state),
            250 + 3 + 150 + 2,
            "every reply of every thread is held"
        );
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        let long_root =
            fake.with(|fake| fake.history[CH][120]["ts"].as_str().expect("ts").to_owned());
        let thread_covered = cache
            .coverage(&chan, &vibe_talk::cache::Scope::Thread(long_root))
            .served()
            .expect("coverage");
        assert!(
            thread_covered
                .windows(2)
                .all(|pair| pair[0].t1_ms >= pair[1].t0_ms)
                && !thread_covered.is_empty(),
            "the long thread's two pages join into one stretch: {thread_covered:?}"
        );
    }

    /// A full walk stopped part way starts again from the newest page when the cache no longer
    /// holds where it stopped: after an erase of the channel, which takes its record with it, and
    /// after its Main coverage is gone (here an erase of that scope; in service, the week's
    /// revalidation after a long stop, or eviction), which leaves the record behind.
    #[tokio::test]
    async fn a_full_walk_whose_stopping_place_is_gone_starts_again_from_the_newest_page() {
        let fake = Fake::start(long_history(1_000, 3 * 3600), 100).await;
        let (state, clock, _dir) = cached(&fake, "full-gone", FULL).await;
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        let mut backfill = Backfill::new();
        for _ in 0..3 {
            backfill.step(&state, LIMIT, Duration::ZERO).await;
        }
        assert_eq!(full_pages(&mut backfill, &state, &clock, 3).await, 3);
        assert_eq!(frontier(&state).expect("a record").cycle_pages, 3);
        // The Main scope's claims go; the record stays, and says where the walk stopped.
        cache.erase_scope(&chan, &vibe_talk::cache::Scope::Main);
        assert!(cache.quiesce_blocking(WAIT));
        assert_eq!(frontier(&state).expect("kept").cycle_pages, 3);
        let mut backfill = Backfill::new();
        let mut steps = 0;
        while frontier(&state).expect("a record").cycle_pages == 3 {
            steps += 1;
            assert!(steps < 100, "the walk never went on");
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Waiting => clock.advance(1_200),
                BackfillStep::Read | BackfillStep::Idle => {}
                other => panic!("unexpected step {other:?}"),
            }
        }
        assert_eq!(
            frontier(&state).expect("a record").cycle_pages,
            1,
            "the walk started again from the newest page, not from a stretch the cache lost"
        );
        // An erase of the channel takes the record with it.
        assert_eq!(full_pages(&mut backfill, &state, &clock, 2).await, 2);
        cache.erase_channels(std::slice::from_ref(&chan));
        assert!(cache.quiesce_blocking(WAIT));
        assert_eq!(frontier(&state), None, "the record went with the channel");
        let mut steps = 0;
        while frontier(&state).is_none() {
            steps += 1;
            assert!(steps < 100, "the walk never began again");
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Waiting => clock.advance(1_200),
                BackfillStep::Read | BackfillStep::Idle => {}
                other => panic!("unexpected step {other:?}"),
            }
        }
        assert_eq!(frontier(&state).expect("a record").cycle_pages, 1);
    }

    /// A continuation the provider no longer takes (a Discord cursor lives five minutes, and not
    /// across a restart) is asked for twice, and then the walk starts again from the newest page
    /// rather than asking for ever.
    #[tokio::test]
    async fn a_full_walk_whose_continuation_keeps_failing_starts_again_from_the_newest_page() {
        let fake = Fake::start(long_history(1_000, 3 * 3600), 100).await;
        let (state, clock, _dir) = cached(&fake, "full-refused", FULL).await;
        let mut backfill = Backfill::new();
        for _ in 0..3 {
            backfill.step(&state, LIMIT, Duration::ZERO).await;
        }
        assert_eq!(full_pages(&mut backfill, &state, &clock, 3).await, 3);
        fake.with(|fake| {
            fake.fail
                .insert("conversations.history".to_owned(), "invalid_cursor".to_owned());
        });
        clock.advance(1_200);
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Failed
        );
        clock.advance(61_000);
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Failed
        );
        fake.with(|fake| {
            fake.fail.remove("conversations.history");
        });
        clock.advance(1_200);
        assert_eq!(full_pages(&mut backfill, &state, &clock, 1).await, 1);
        assert_eq!(
            frontier(&state).expect("a record").cycle_pages,
            1,
            "after two refusals the walk began again from the newest page"
        );
    }

    /// A walk that gave up a thread reaches the beginning without making the channel complete.
    #[tokio::test]
    async fn a_full_walk_that_gave_up_a_thread_does_not_make_the_channel_complete() {
        let mut fixture = long_history(50, 3600);
        let mut list = fixture.history.remove(CH).expect("history");
        let root_ts = list[20]["ts"].as_str().expect("ts").to_owned();
        let replies = vec![(format!("{}.000000", wall_secs() - 30), "a reply".to_owned())];
        super::slack_fake::thread(&mut fixture, CH, &mut list[20], &replies);
        fixture.history.insert(CH.to_owned(), list);
        fixture.fail_threads.insert(root_ts, 500);
        let fake = Fake::start(fixture, 100).await;
        let (state, clock, _dir) = cached(&fake, "full-gave-up", FULL).await;
        let mut backfill = Backfill::new();
        let mut steps = 0;
        while frontier(&state).is_none_or(|f| f.cursor.is_some()) {
            steps += 1;
            assert!(steps < 200, "the walk never ended");
            match backfill.step(&state, LIMIT, Duration::ZERO).await {
                BackfillStep::Waiting => clock.advance(1_200),
                // The channel waits ten minutes after each failed read of the thread.
                BackfillStep::Failed | BackfillStep::Idle => clock.advance(600_000),
                BackfillStep::Read => {}
                other => panic!("unexpected step {other:?}"),
            }
        }
        let ended = frontier(&state).expect("a record");
        assert!(ended.cycle_gaps >= 1, "{ended:?}");
        assert_eq!(ended.complete_ms, None, "{ended:?}");
        let history = vibe_talk::backfill::history(&state).expect("a report");
        assert!(!history.channels[0].complete);
        assert_eq!(history.summary().complete, 0);
    }

    /// A stream that lost records, or came back after a silence, asks for each of its channels' hot
    /// walks at once, rather than at the next interval.
    #[tokio::test]
    async fn a_stream_gap_or_a_silence_asks_for_a_hot_walk_at_once() {
        let fake = Fake::start(history(120, 60), 20).await;
        let (state, clock, _dir) = cached(&fake, "rewalk-gap", "\n[message_cache]\n").await;
        let mut backfill = Backfill::new();
        while backfill.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
        clock.advance(60_000);
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Idle,
            "nothing is due a minute after a start's walk"
        );
        state.live.request_rewalk(&channel());
        assert_eq!(
            backfill.step(&state, LIMIT, Duration::ZERO).await,
            BackfillStep::Read,
            "the hot walk ran at once"
        );
        while backfill.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
        // The ingest route: a record after half an hour's silence asks the same.
        let t = 1_800_000_000_000;
        assert!(
            !state.live.note_stream_record(t),
            "the first record ends no silence"
        );
        assert!(!state.live.note_stream_record(t + 29 * 60 * 1_000));
        assert!(state.live.note_stream_record(t + 60 * 60 * 1_000));
    }

    /// Backfill page reads, and Web API calls, in one day of a channel that gets a message every
    /// `every` seconds and already holds 30 days of them, under `table`: counted from the end of
    /// the first hour, so the walk a start makes is left out. Each simulated minute the clock moves
    /// on, a message arrives when one is due, and the backfill steps until nothing is due.
    async fn day_of_reads(tag: &str, every: i64, table: &str) -> (usize, usize) {
        let fake = Fake::start(long_history(30 * 86_400 / every, every), 20).await;
        let (state, clock, _dir) = cached(&fake, tag, table).await;
        let mut backfill = Backfill::new();
        let (mut reads, mut calls_at_start) = (0, 0);
        let mut next_message = clock.now() / 1_000 + every - 60;
        for minute in 0..(25 * 60) {
            clock.advance(60_000);
            let now_secs = clock.now() / 1_000;
            if now_secs >= next_message {
                fake.with(|fake| {
                    fake.history.get_mut(CH).expect("history").push(profiled(
                        &format!("{now_secs}.000000"),
                        "UADA00001",
                        "Ada",
                        "arrival",
                    ));
                });
                next_message += every;
            }
            if minute == 60 {
                calls_at_start = fake.calls("conversations.history");
            }
            loop {
                match backfill.step(&state, LIMIT, Duration::ZERO).await {
                    BackfillStep::Read => {
                        if minute >= 60 {
                            reads += 1;
                        }
                    }
                    BackfillStep::Idle => break,
                    other => panic!("unexpected step {other:?}"),
                }
            }
        }
        (reads, fake.calls("conversations.history") - calls_at_start)
    }

    /// `#595 cache-full-history`: what the tiered re-walk costs against the single walk it
    /// replaced, on the loopback fake, in pages of 50 a day (each page three calls of the fake's
    /// 20 rows). Before: the whole recent stretch (14 days, at most 20 pages) every six hours.
    /// After: the last 24 hours every hour, and the whole recent stretch once a day.
    #[tokio::test]
    async fn measure_backfill_reads_a_day_before_and_after_the_tiers() {
        // `max_days = 14`, as on the live instance: the recent walk goes as far back as that, as
        // the single walk did, and with both intervals at six hours this is that walk exactly.
        const BEFORE: &str = "\n[message_cache]\nmax_days = 14\nrewalk_recent_hours = 6\n\
                              rewalk_hot_minutes = 360\n";
        const AFTER: &str = "\n[message_cache]\nmax_days = 14\n";
        let mut figures = Vec::new();
        for (channel, every) in [("busy", 300), ("quiet", 7_200)] {
            let before = day_of_reads(&format!("tiers-before-{channel}"), every, BEFORE).await;
            let after = day_of_reads(&format!("tiers-after-{channel}"), every, AFTER).await;
            eprintln!(
                "backfill reads a day, {channel} channel (a message every {every} s): before {} \
                 pages ({} calls), after {} pages ({} calls)",
                before.0, before.1, after.0, after.1
            );
            figures.push((channel, before, after));
        }
        // Busy: 288 messages a day, so the recent stretch is past 20 pages and the last day is
        // six. Before, four walks of 20; after, 23 of six and one of 20.
        let (_, busy_before, busy_after) = figures[0];
        assert_eq!(busy_before.0, 4 * 20, "{figures:?}");
        assert_eq!(busy_after.0, 23 * 6 + 20, "{figures:?}");
        // Quiet: 12 a day, so the recent stretch is four pages and the last day one.
        let (_, quiet_before, quiet_after) = figures[1];
        assert_eq!(quiet_before.0, 4 * 4, "{figures:?}");
        assert_eq!(quiet_after.0, 23 + 4, "{figures:?}");
    }

    /// The shadow's count for the newest timeline page.
    fn timeline_counts(state: &AppState) -> vibe_talk::cache::shadow::ConsumerCounts {
        state
            .message_cache
            .shadow_counts()
            .into_iter()
            .find(|counts| counts.consumer == "timeline_fallback")
            .expect("the timeline consumer")
    }

    async fn settled(state: &AppState) {
        assert!(state.message_cache.shadow_settled(WAIT).await);
        if let Some(cache) = state.message_cache.cache() {
            assert!(cache.quiesce_blocking(WAIT));
        }
    }

    /// What a page's own write-scope read of the newest Main page writes to the cache, without the
    /// shadow evaluation beside it: the fill every reader's read makes.
    async fn page_fill(state: &AppState) {
        let request = vibe_talk::threads::TimelineRequest {
            limit: LIMIT,
            ..vibe_talk::threads::TimelineRequest::default()
        };
        let fill = state
            .message_cache
            .begin_for(vibe_talk::auth::Scope::Write, &channel());
        let page = vibe_talk::chat::ChatClient::fetch_timeline(
            state.providers.as_ref(),
            &channel(),
            &request,
        )
        .await
        .expect("the page reads");
        if let Some(fill) = fill {
            fill.offer_timeline(
                &request,
                vibe_talk::cache::observe::TimelineRead::Newest,
                &page,
            );
        }
    }

    /// One workload: `rounds` times, a message arrives (pushed by the stream) and then a reader asks
    /// for the newest Main page, whose verdict the shadow counts (asked with the read token, so the
    /// ask's own read writes nothing it could race); the reader's own write-scope read then fills the
    /// cache, as a page's does. With `backfill`, the backfill walks first; with `relist`, the relist
    /// runs between each arrival and the ask, as it does whenever its interval is shorter than the
    /// reader's.
    async fn servable(
        tag: &str,
        backfill: bool,
        relist: bool,
        rounds: usize,
    ) -> vibe_talk::cache::shadow::ConsumerCounts {
        let fake = Fake::start(history(200, 3_600), 100).await;
        let (state, clock, _dir) =
            cached(&fake, tag, "\n[message_cache]\nbackfill = \"adaptive\"\n").await;
        let mut cursor = None;
        vibe_talk::live::poll_once(
            state.chat.as_ref(),
            &state.live,
            &channel(),
            LIMIT,
            &mut cursor,
        )
        .await
        .expect("the seed");
        page_fill(&state).await;
        if backfill {
            let mut walk = Backfill::new();
            while walk.step(&state, LIMIT, Duration::ZERO).await == BackfillStep::Read {}
        }
        let timeline = format!("/api/v1/channels/{CH}/timeline?view=main&limit=50");
        for round in 0..rounds {
            clock.advance(30_000);
            tokio::time::sleep(Duration::from_millis(1_100)).await;
            let ts = arrive(&fake, round, 100 + i64::try_from(round).expect("small"));
            let status = request(
                &state,
                "POST",
                "/api/v1/live/tail",
                INGEST_TOKEN,
                &record(
                    u64::try_from(round).expect("small") + 1,
                    &ts,
                    &format!("arrival {round}"),
                ),
            )
            .await;
            assert!(status.is_success(), "{status}");
            clock.advance(1_000);
            if relist {
                vibe_talk::relist::relist_once(
                    &state,
                    None,
                    RelistOptions {
                        limit: LIMIT,
                        threads: 4,
                        interval: Duration::from_secs(60),
                        settle: Duration::ZERO,
                    },
                    wall_secs() * 1_000,
                )
                .await;
            }
            settled(&state).await;
            // The cache's clock is the test's: the ask comes a second after what was written for it,
            // as it would by any real clock.
            clock.advance(1_000);
            assert_eq!(
                request(&state, "GET", &timeline, READ_TOKEN, &Value::Null).await,
                StatusCode::OK
            );
            settled(&state).await;
            page_fill(&state).await;
            settled(&state).await;
        }
        timeline_counts(&state)
    }

    /// The rows the server's own readers ask for: `[chat] default_fetch_limit`, which a deployment
    /// may set below the page's own [`LIMIT`].
    const SERVER_LIMIT: u16 = 40;

    /// One round of [`servable`]'s workload with the relist reading [`SERVER_LIMIT`] rows: a
    /// message arrives through the stream, the relist reads the channel, and a reader asks for the
    /// newest Main page of [`LIMIT`] (with the read token, so the ask's own read writes nothing),
    /// then fills the cache as a page's read does.
    async fn relisted_round(state: &AppState, fake: &Fake, clock: &ManualClock, round: usize) {
        clock.advance(30_000);
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        let ts = arrive(fake, round, 100 + i64::try_from(round).expect("small"));
        let status = request(
            state,
            "POST",
            "/api/v1/live/tail",
            INGEST_TOKEN,
            &record(
                u64::try_from(round).expect("small") + 1,
                &ts,
                &format!("arrival {round}"),
            ),
        )
        .await;
        assert!(status.is_success(), "{status}");
        clock.advance(1_000);
        vibe_talk::relist::relist_once(
            state,
            None,
            RelistOptions {
                limit: SERVER_LIMIT,
                threads: 4,
                interval: Duration::from_secs(60),
                settle: Duration::ZERO,
            },
            wall_secs() * 1_000,
        )
        .await;
        settled(state).await;
        clock.advance(1_000);
        let timeline = format!("/api/v1/channels/{CH}/timeline?view=main&limit={LIMIT}");
        assert_eq!(
            request(state, "GET", &timeline, READ_TOKEN, &Value::Null).await,
            StatusCode::OK
        );
        settled(state).await;
        page_fill(state).await;
        settled(state).await;
    }

    /// The backfill's whole walk, at [`SERVER_LIMIT`] rows a page.
    async fn backfill_walk(state: &AppState, walk: &mut Backfill) {
        while walk.step(state, SERVER_LIMIT, Duration::ZERO).await == BackfillStep::Read {}
    }

    /// A server starting on the cache file in `dir` as `main` starts one: the backfill's first step
    /// comes before the provider's account has been checked against the file, and the poller's
    /// first tick waits for that check ([`vibe_talk::cache::Sink::first_checks`]); the walk then
    /// goes on to its end.
    async fn started(fake: &Fake, dir: &TempDir, clock: &Arc<ManualClock>) -> AppState {
        let table = "\n[message_cache]\nbackfill = \"adaptive\"\n";
        let state = testing::with_message_cache_unverified(
            deployment(fake, table, 3600),
            dir.path(),
            clock.clone(),
        );
        let mut walk = Backfill::new();
        // Before the check: a read now would write nothing, and a walk begun now would prove
        // nothing, however it ended, and would not be walked again for six hours.
        assert_eq!(
            walk.step(&state, SERVER_LIMIT, Duration::ZERO).await,
            BackfillStep::Unchecked
        );
        state.message_cache.first_checks(WAIT).await;
        assert!(
            state
                .message_cache
                .identities_verified(Duration::ZERO)
                .await
        );
        let mut cursor = None;
        vibe_talk::live::poll_once(
            state.chat.as_ref(),
            &state.live,
            &channel(),
            SERVER_LIMIT,
            &mut cursor,
        )
        .await
        .expect("the first tick");
        let began_ms = clock.now();
        backfill_walk(&state, &mut walk).await;
        settled(&state).await;
        // The walk proved its span in this process: what lies under the relist's shorter pages.
        let cache = state.message_cache.cache().expect("a cache");
        let chan = state.message_cache.chan_key(&channel()).expect("routed");
        let covered = cache
            .coverage(&chan, &vibe_talk::cache::Scope::Main)
            .served()
            .expect("coverage");
        assert!(
            covered.iter().any(|interval| {
                interval.source == vibe_talk::cache::coverage::Source::Walk
                    && interval.listed_ms >= began_ms
            }),
            "the walk's listing was written: {covered:?}"
        );
        state
    }

    /// `#502 cache-serve-coverage`. A start must not cost the newest page its servability for
    /// longer than a relist. `main` starts the backfill, the poller and the relist before the
    /// provider's account has been checked against the file, so the first reads they begin are
    /// left out of it and leave the channel in doubt; and the server's readers ask for fewer rows
    /// than the page does, so what lies under their pages is proven only by the backfill's walk.
    #[tokio::test]
    async fn the_newest_page_is_servable_soon_after_a_start_and_a_restart() {
        let fake = Fake::start(history_with_joins(200, 3_600, 7), 100).await;
        let dir = TempDir::new("restart-coverage");
        let clock = ManualClock::at(wall_secs() * 1_000);
        let before = {
            let state = started(&fake, &dir, &clock).await;
            for round in 0..3 {
                relisted_round(&state, &fake, &clock, round).await;
            }
            let counts = timeline_counts(&state);
            if let Some(cache) = state.message_cache.uninstall() {
                assert!(cache.quiesce_blocking(WAIT));
            }
            counts
        };
        clock.advance(5_000);
        let state = started(&fake, &dir, &clock).await;
        for round in 3..6 {
            relisted_round(&state, &fake, &clock, round).await;
        }
        let after = timeline_counts(&state);
        for counts in [&before, &after] {
            assert_eq!(
                (
                    counts.asked,
                    counts.would_serve,
                    counts.missing_rows,
                    counts.extra_rows
                ),
                (3, 3, 0, 0),
                "servable from the first relist after a start: {before:?} then {after:?}"
            );
        }
    }

    #[tokio::test]
    #[ignore = "a measurement: run with --ignored --nocapture"]
    async fn measure_the_servable_rate_of_the_newest_timeline_page() {
        let rounds = 10;
        let mut rates = Vec::new();
        for (name, backfill, relist) in [
            ("fill-only", false, false),
            ("backfill", true, false),
            ("relist", false, true),
            ("relist+backfill", true, true),
        ] {
            let counts = servable(&format!("measure-{name}"), backfill, relist, rounds).await;
            println!(
                "timeline_fallback {name:>16}: {}/{} servable, {} agreed; misses {:?}; gaps {:?}",
                counts.would_serve, counts.asked, counts.agreed, counts.misses, counts.gaps
            );
            rates.push(counts.would_serve);
        }
        assert!(
            rates[3] > rates[0],
            "the relist and the backfill make more reads servable"
        );
    }

    /// How long one message takes from its stream record reaching vibe-talk to the live hub, by
    /// hint (the poller reads the channel through the bridge) and by pushed record.
    async fn arrival_to_hub(
        push: bool,
        delay: Duration,
        messages: usize,
        gap: Duration,
    ) -> Vec<Duration> {
        let mut fixture = history(5, 120);
        fixture.delay = Some(delay);
        let fake = Fake::start(fixture, 100).await;
        let state = deployment(&fake, "", 3600);
        let poller = tokio::spawn(vibe_talk::live::poll_forever(
            state.clone(),
            None,
            Arc::clone(&state.providers.entries()[0].client),
            LIMIT,
            Duration::from_secs(3600),
        ));
        // Let the poller seed before anything arrives.
        let deadline = Instant::now() + WAIT;
        while state.live.poller_cursor(&channel()).is_none() {
            assert!(Instant::now() < deadline, "the poller seeded");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mut subscription = state.live.subscribe(&channel(), None);
        let mut latencies = Vec::new();
        for n in 0..messages {
            let ts = arrive(&fake, n, 500 + i64::try_from(n).expect("small"));
            let started = Instant::now();
            let status = if push {
                request(
                    &state,
                    "POST",
                    "/api/v1/live/tail",
                    INGEST_TOKEN,
                    &record(
                        u64::try_from(n).expect("small") + 1,
                        &ts,
                        &format!("arrival {n}"),
                    ),
                )
                .await
            } else {
                request(
                    &state,
                    "POST",
                    "/api/v1/live/hints",
                    INGEST_TOKEN,
                    &json!({"channel_id": CH}),
                )
                .await
            };
            assert!(status.is_success(), "{status}");
            let id = vibe_talk::slack::message_id_from_ts(&ts).expect("an id");
            loop {
                let live = tokio::time::timeout(WAIT, subscription.receiver.recv())
                    .await
                    .expect("published in time")
                    .expect("the hub is open");
                if live.message.id == id {
                    break;
                }
            }
            latencies.push(started.elapsed());
            tokio::time::sleep(gap).await;
        }
        poller.abort();
        latencies
    }

    fn summary(latencies: &[Duration]) -> String {
        let mut ms: Vec<u128> = latencies.iter().map(Duration::as_millis).collect();
        ms.sort_unstable();
        format!(
            "n={} min={}ms median={}ms max={}ms",
            ms.len(),
            ms[0],
            ms[ms.len() / 2],
            ms[ms.len() - 1]
        )
    }

    #[tokio::test]
    #[ignore = "a measurement: run with --ignored --nocapture"]
    async fn measure_arrival_to_visible_by_hint_and_by_pushed_record() {
        for delay_ms in [0_u64, 300, 1_500] {
            let delay = Duration::from_millis(delay_ms);
            for gap_ms in [6_000_u64, 1_000] {
                let gap = Duration::from_millis(gap_ms);
                let hinted = arrival_to_hub(false, delay, 4, gap).await;
                let pushed = arrival_to_hub(true, delay, 4, gap).await;
                println!(
                    "bridge read {delay_ms:>5}ms, arrivals {gap_ms:>5}ms apart: hint {} | pushed {}",
                    summary(&hinted),
                    summary(&pushed)
                );
            }
        }
    }
}
