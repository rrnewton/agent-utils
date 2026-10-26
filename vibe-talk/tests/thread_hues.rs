//! `#427 server-thread-hues`: the colours a thread is drawn in are decided by the server.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};
use vibe_talk::threads::{
    MessageThread, ThreadSummary, TimelinePage, TimelineRequest, TimelineView,
};

/// A snowflake at `minutes` past a fixed morning, so root times order as written.
fn snowflake(minutes: u64) -> String {
    ((1_700_000_000_000_u64 - 1_420_070_400_000 + minutes * 60_000) << 22).to_string()
}

fn message(id: &str, thread: &str, root: bool, minutes: u64) -> Message {
    Message {
        edited_timestamp: None,
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(WRITE_CHANNEL.to_owned()),
        author: "Reader".to_owned(),
        author_id: UserId("7".to_owned()),
        author_is_bot: false,
        timestamp: format!("2023-11-14T22:{:02}:00Z", 13 + minutes),
        spoken_time: String::new(),
        reply_to: None,
        content: format!("in {thread}"),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        thread: Some(MessageThread {
            hue: None,
            display_name: None,
            summary: None,
            id: thread.to_owned(),
            root_message_id: Some(MessageId(snowflake(minutes))),
            is_root: root,
            reply_count: Some(1),
            reply_count_exact: true,
        }),
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
        upstream_app_url: None,
        op_key: None,
        observed_ms: None,
    }
}

/// Three threads, roots a minute apart, one reply each after them all.
fn cast() -> Vec<Message> {
    let mut messages = Vec::new();
    for (i, thread) in ["thread-a", "thread-b", "thread-c"].iter().enumerate() {
        let minutes = u64::try_from(i).expect("small");
        messages.push(message(&snowflake(minutes), thread, true, minutes));
    }
    for (i, thread) in ["thread-a", "thread-b", "thread-c"].iter().enumerate() {
        let minutes = u64::try_from(i).expect("small");
        messages.push(message(&snowflake(10 + minutes), thread, false, minutes));
    }
    messages
}

#[derive(Default)]
struct Backend {
    live_after: Mutex<Vec<Message>>,
}

#[async_trait::async_trait]
impl ChatClient for Backend {
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "Reader".to_owned(),
            workspace: None,
        })
    }

    async fn fetch_page(
        &self,
        _channel: &ChannelId,
        _limit: u16,
        _before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        // The live walk: what arrived after the seed's newest.
        let held = self.live_after.lock().unwrap();
        let after = after.and_then(MessageId::numeric).unwrap_or(0);
        Ok(held
            .iter()
            .filter(|message| message.id.numeric().is_some_and(|id| id > after))
            .cloned()
            .collect())
    }

    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let messages = cast();
        let threads = ["thread-a", "thread-b", "thread-c"]
            .iter()
            .enumerate()
            .map(|(i, thread)| ThreadSummary {
                hue: None,
                id: (*thread).to_owned(),
                root: Some(messages[i].clone()),
                title: (*thread).to_owned(),
                reply_count: Some(1),
                reply_count_exact: true,
                updated_at: messages[i].timestamp.clone(),
                display_name: None,
                summary: None,
            })
            .collect();
        Ok(TimelinePage {
            messages: if request.view == TimelineView::Threads {
                vec![]
            } else {
                messages
            },
            threads: if request.view == TimelineView::Threads {
                threads
            } else {
                vec![]
            },
            has_threads: true,
            ..TimelinePage::default()
        })
    }

    async fn fetch_recent(
        &self,
        _channel: &ChannelId,
        _limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        // The poller's seed: existing history, the roots alone.
        Ok(cast().into_iter().take(3).collect())
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("not in this fixture".to_owned()))
    }
}

fn harness() -> (
    axum::Router,
    Arc<Backend>,
    Arc<FakeStore>,
    vibe_talk::state::AppState,
) {
    let store = Arc::new(FakeStore::new());
    let (mut state, _) = testing::state_with(store.clone());
    let backend = Arc::new(Backend::default());
    state.replace_chat(backend.clone());
    let app = vibe_talk::http::router(state.clone());
    (app, backend, store, state)
}

async fn call(
    app: &axum::Router,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn timeline(view: &str) -> String {
    format!("/api/v1/channels/{WRITE_CHANNEL}/timeline?view={view}")
}

/// Each thread's hue as a body carries it, asserting its messages and summary agree.
fn hues_in(body: &Value) -> Vec<(String, Option<u64>)> {
    let mut found: Vec<(String, Option<u64>)> = Vec::new();
    let mut note = |id: &Value, hue: &Value| {
        let id = id.as_str().expect("id").to_owned();
        let hue = hue.as_u64();
        if let Some((_, held)) = found.iter().find(|(known, _)| *known == id) {
            assert_eq!(*held, hue, "{id} carries two hues at once");
        } else {
            found.push((id, hue));
        }
    };
    for message in body["messages"].as_array().cloned().unwrap_or_default() {
        note(&message["thread"]["id"], &message["thread"]["hue"]);
    }
    for summary in body["threads"].as_array().cloned().unwrap_or_default() {
        note(&summary["id"], &summary["hue"]);
        note(
            &summary["root"]["thread"]["id"],
            &summary["root"]["thread"]["hue"],
        );
    }
    found
}

#[tokio::test]
async fn a_write_scope_read_colours_every_thread_and_the_colours_stand() {
    let (app, _backend, store, _state) = harness();
    let (status, body) = call(&app, "GET", &timeline("flat"), WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let first = hues_in(&body);
    assert_eq!(first.len(), 3);
    let hues: Vec<u64> = first
        .iter()
        .map(|(id, hue)| hue.unwrap_or_else(|| panic!("{id} was served without a hue")))
        .collect();
    assert_eq!(hues[0], 210, "the first thread is the first hue");
    // Consecutive roots stand at least 45 degrees apart (`#414 thread-hue-spread`).
    for pair in hues.windows(2) {
        let apart = pair[0].abs_diff(pair[1]) % 360;
        assert!(apart.min(360 - apart) >= 45, "{hues:?}");
    }
    // The thread cards carry the same colours...
    let (status, cards) = call(&app, "GET", &timeline("threads"), WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{cards}");
    let mut card_hues = hues_in(&cards);
    card_hues.sort();
    let mut expected = first.clone();
    expected.sort();
    assert_eq!(card_hues, expected);
    // ...a second read changes nothing, and the store holds exactly those.
    let (_status, again) = call(&app, "GET", &timeline("flat"), WRITE_TOKEN, None).await;
    assert_eq!(hues_in(&again), first);
    let held = store
        .thread_hues(&ChannelId(WRITE_CHANNEL.to_owned()))
        .await
        .expect("read back");
    assert_eq!(
        held.iter()
            .map(|row| (row.thread_id.clone(), Some(u64::from(row.hue))))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(id, hue)| (id.clone(), *hue))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_read_scope_read_colours_nothing_and_then_carries_what_was_assigned() {
    let (app, _backend, store, _state) = harness();
    // The read token met these threads first: served uncoloured, and nothing was written —
    // no read-scope credential ever writes anything durable.
    let (status, body) = call(&app, "GET", &timeline("flat"), READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        hues_in(&body).iter().all(|(_, hue)| hue.is_none()),
        "a read-scope read was served hues it should not have assigned: {body}"
    );
    assert!(
        store
            .thread_hues(&ChannelId(WRITE_CHANNEL.to_owned()))
            .await
            .expect("read back")
            .is_empty(),
        "a read-scope read filed hues"
    );
    // A write-scope read colours them; the read token then carries those very colours.
    let (_status, written) = call(&app, "GET", &timeline("flat"), WRITE_TOKEN, None).await;
    let (status, read) = call(&app, "GET", &timeline("flat"), READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(hues_in(&read), hues_in(&written));
}

#[tokio::test]
async fn a_browser_may_seed_its_colours_once_and_never_recolour() {
    let (app, _backend, _store, _state) = harness();
    let path = format!("/api/v1/channels/{WRITE_CHANNEL}/thread-hues");
    let offer = json!({"hues": [
        {"thread_id": "thread-a", "hue": 12.5, "last_drawn_ms": 100},
        {"thread_id": "thread-b", "hue": 200, "last_drawn_ms": 200}
    ]});
    // Write scope only.
    let (status, _body) = call(&app, "POST", &path, READ_TOKEN, Some(offer.clone())).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(&app, "POST", &path, WRITE_TOKEN, Some(offer)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["seeded"], 2);
    // The page's fractional hue rounds to the nearest whole degree.
    assert_eq!(body["hues"]["thread-a"], 13);
    assert_eq!(body["hues"]["thread-b"], 200);
    // Reads carry the seeded colours — today's colours survived — and colour only thread-c.
    let (_status, read) = call(&app, "GET", &timeline("flat"), WRITE_TOKEN, None).await;
    let held = hues_in(&read);
    assert_eq!(
        held.iter()
            .find(|(id, _)| id == "thread-a")
            .map(|(_, hue)| *hue),
        Some(Some(13))
    );
    // A second browser's different memory takes nothing already decided.
    let (status, body) = call(
        &app,
        "POST",
        &path,
        WRITE_TOKEN,
        Some(json!({"hues": [{"thread_id": "thread-a", "hue": 99}]})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["seeded"], 0);
    assert_eq!(body["hues"]["thread-a"], 13);
    // A hue outside the wheel is refused, not wrapped silently by the store.
    let (status, _body) = call(
        &app,
        "POST",
        &path,
        WRITE_TOKEN,
        Some(json!({"hues": [{"thread_id": "thread-x", "hue": 720}]})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_thread_met_live_is_coloured_as_it_arrives() {
    let (_app, backend, store, state) = harness();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let hub = &state.live;
    let mut cursor = None;
    // The seeding tick reads existing history and colours none of it.
    vibe_talk::live::poll_once(backend.as_ref(), hub, &channel, 50, &mut cursor)
        .await
        .expect("seed tick");
    assert!(
        store
            .thread_hues(&channel)
            .await
            .expect("read back")
            .is_empty(),
        "the seeding tick coloured existing history"
    );
    // A reply arrives in a thread nobody has coloured: the tick that publishes it colours it,
    // and what is published already carries the colour.
    backend
        .live_after
        .lock()
        .unwrap()
        .push(message(&snowflake(30), "thread-a", false, 0));
    let subscription = hub.subscribe(&channel, None);
    drop(subscription);
    vibe_talk::live::poll_once(backend.as_ref(), hub, &channel, 50, &mut cursor)
        .await
        .expect("live tick");
    let held = store.thread_hues(&channel).await.expect("read back");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].thread_id, "thread-a");
    let replay = hub.subscribe(&channel, None).replay;
    let live = replay
        .iter()
        .find(|live| live.message.id.as_str() == snowflake(30))
        .expect("the arrival is in the tail");
    assert_eq!(
        live.message.thread.as_ref().and_then(|thread| thread.hue),
        Some(held[0].hue),
        "what was published does not carry the colour assigned to it"
    );
}
