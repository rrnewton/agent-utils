//! `#320 slack-edited-mark`: the time Slack says a message was last edited reaches the page.
//!
//! `conversations.history` and `conversations.replies` carry `edited: {user, ts}` on a message that
//! was edited. Every read here carries it as `edited_timestamp`, the edit's instant: history, Main,
//! Flat, a thread, the newest messages, the live poller, a forward read and the HTTP API. A message
//! Slack lists without it, or with an `edited` whose `ts` is not one, has none, so a bridge that
//! cannot supply edits reads exactly as before.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, thread, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::live::{poll_once, LiveHub};
use vibe_talk::model::{ChannelId, Message};
use vibe_talk::providers::ChatRouter;
use vibe_talk::slack::ts_from_message_id;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::{TimelineRequest, TimelineView};
use vibe_talk::timeline_forward::{generic_since, position_of};

const CH: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;

/// The fixture's base instant, 2023-11-14T22:13:20Z, plus `seconds`, as `edited_timestamp` says it.
fn at(seconds: i64) -> String {
    let minute = 13 + (20 + seconds) / 60;
    let second = (20 + seconds) % 60;
    format!("2023-11-14T22:{minute:02}:{second:02}.000000Z")
}

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// The message timestamps of the fixture.
struct Stamps {
    /// A thread root edited at 35 s.
    root: String,
    /// Its reply, edited at 33 s.
    edited_reply: String,
    /// Its other reply, never edited.
    plain_reply: String,
    /// A message edited at 45 s.
    edited: String,
    /// A message whose `edited` carries no `ts`, as a bridge might send it.
    vague: String,
    /// A message never edited.
    plain: String,
}

/// Seven messages ten seconds apart; the one at 30 s roots a thread of two replies.
fn fixture() -> (FakeState, Stamps) {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..7)
        .map(|i| profiled(&ts(i * 10, 0), "UADA00001", "Ada", &format!("message {i}")))
        .collect();
    let stamps = Stamps {
        root: ts(30, 0),
        edited_reply: ts(31, 0),
        plain_reply: ts(32, 0),
        edited: ts(40, 0),
        vague: ts(50, 0),
        plain: ts(60, 0),
    };
    thread(
        &mut state,
        CH,
        &mut list[3],
        &[
            (stamps.edited_reply.clone(), "first reply, fixed".to_owned()),
            (stamps.plain_reply.clone(), "second reply".to_owned()),
        ],
    );
    list[3]["edited"] = json!({"user": "UADA00001", "ts": ts(35, 0)});
    list[4]["text"] = json!("message 4, with the typo fixed");
    list[4]["edited"] = json!({"user": "UADA00001", "ts": ts(45, 0)});
    list[5]["edited"] = json!({"user": "UADA00001"});
    state
        .replies
        .get_mut(&(CH.to_owned(), stamps.root.clone()))
        .expect("the thread's replies")[0]["edited"] =
        json!({"user": "UREPLIER1", "ts": ts(33, 0)});
    state.history.insert(CH.to_owned(), list);
    (state, stamps)
}

/// The edit time of the message at `stamp` among `messages`.
fn edited_at<'m>(messages: &'m [Message], stamp: &str) -> Option<&'m str> {
    messages
        .iter()
        .find(|message| ts_from_message_id(&message.id).as_deref() == Some(stamp))
        .unwrap_or_else(|| panic!("no message at {stamp}"))
        .edited_timestamp
        .as_deref()
}

fn assert_own(messages: &[Message], stamps: &Stamps, read: &str) {
    assert_eq!(
        edited_at(messages, &stamps.root),
        Some(at(35).as_str()),
        "{read}: the root"
    );
    assert_eq!(
        edited_at(messages, &stamps.edited),
        Some(at(45).as_str()),
        "{read}: the edited message"
    );
    assert_eq!(
        edited_at(messages, &stamps.vague),
        None,
        "{read}: an edit with no time"
    );
    assert_eq!(
        edited_at(messages, &stamps.plain),
        None,
        "{read}: a message never edited"
    );
}

fn assert_replies(messages: &[Message], stamps: &Stamps, read: &str) {
    assert_eq!(
        edited_at(messages, &stamps.edited_reply),
        Some(at(33).as_str()),
        "{read}: the edited reply"
    );
    assert_eq!(
        edited_at(messages, &stamps.plain_reply),
        None,
        "{read}: the reply never edited"
    );
}

fn view(view: TimelineView, thread_id: Option<&str>) -> TimelineRequest {
    TimelineRequest {
        view,
        thread_id: thread_id.map(str::to_owned),
        before: None,
        after: None,
        limit: 50,
    }
}

#[test]
fn the_fixture_says_its_instants_as_slack_ts_does() {
    assert_eq!(at(45), "2023-11-14T22:14:05.000000Z");
    assert_eq!(at(0), "2023-11-14T22:13:20.000000Z");
}

#[tokio::test]
async fn every_read_carries_the_time_slack_says_a_message_was_edited() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();

    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert_own(&page, &stamps, "history");

    let main = client
        .fetch_timeline(&channel(), &view(TimelineView::Main, None))
        .await
        .expect("Main");
    assert_own(&main.messages, &stamps, "Main");

    let flat = client
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert_own(&flat.messages, &stamps, "Flat");
    assert_replies(&flat.messages, &stamps, "Flat");

    let thread = client
        .fetch_timeline(
            &channel(),
            &view(TimelineView::Thread, Some(stamps.root.as_str())),
        )
        .await
        .expect("the thread");
    assert_eq!(
        edited_at(&thread.messages, &stamps.root),
        Some(at(35).as_str())
    );
    assert_replies(&thread.messages, &stamps, "the thread");

    let recent = client
        .fetch_recent(&channel(), 50)
        .await
        .expect("the newest messages");
    assert_own(&recent, &stamps, "the newest messages");
    assert_replies(&recent, &stamps, "the newest messages");
}

#[tokio::test]
async fn the_live_poller_delivers_a_message_with_its_edit() {
    let (state, _stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let hub = LiveHub::new();
    let mut receiver = hub.subscribe(&channel(), None).receiver;
    let mut cursor = None;
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("the seeding poll");
    // Written and corrected between two polls.
    fake.with(|state| {
        let mut message = profiled(&ts(70, 0), "UGRACE001", "Grace", "deploying now, really");
        message["edited"] = json!({"user": "UGRACE001", "ts": ts(72, 0)});
        state.history.get_mut(CH).expect("channel").push(message);
    });
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("a poll");
    let live = receiver.try_recv().expect("the new message");
    assert_eq!(
        live.message.edited_timestamp.as_deref(),
        Some(at(72).as_str())
    );
}

#[tokio::test]
async fn an_edit_brings_the_message_back_in_a_forward_read() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let main = view(TimelineView::Main, None);
    let newest = client
        .fetch_timeline(&channel(), &main)
        .await
        .expect("Main");
    let position = position_of(&newest, TimelineView::Main);
    let quiet = generic_since(&client, &channel(), &main, &position)
        .await
        .expect("a forward read");
    assert!(quiet.page.messages.is_empty(), "{:?}", quiet.page.messages);

    // The newest message is corrected: an edit inside the overlap the forward read looks back over.
    fake.with(|state| {
        for message in state.history.get_mut(CH).expect("channel").iter_mut() {
            if message["ts"] == json!(stamps.plain) {
                message["text"] = json!("message 6, corrected");
                message["edited"] = json!({"user": "UADA00001", "ts": ts(65, 0)});
            }
        }
    });
    let delta = generic_since(&client, &channel(), &main, &quiet.position)
        .await
        .expect("a forward read");
    let changed: Vec<(String, Option<String>, String)> = delta
        .page
        .messages
        .iter()
        .map(|m| {
            (
                ts_from_message_id(&m.id).expect("a Slack id"),
                m.edited_timestamp.clone(),
                m.content.clone(),
            )
        })
        .collect();
    assert_eq!(
        changed,
        [(
            stamps.plain.clone(),
            Some(at(65)),
            "message 6, corrected".to_owned()
        )]
    );
}

#[tokio::test]
async fn a_bridge_that_cannot_supply_edits_reads_as_before() {
    let (mut state, stamps) = fixture();
    for message in state
        .history
        .values_mut()
        .chain(state.replies.values_mut())
        .flatten()
    {
        message.as_object_mut().expect("an object").remove("edited");
    }
    let fake = Fake::start(state, PAGE_CAP).await;
    let flat = fake
        .client()
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert!(
        flat.messages.iter().all(|m| m.edited_timestamp.is_none()),
        "{:?}",
        flat.messages
    );
    assert_eq!(
        edited_at(&flat.messages, &stamps.edited),
        None,
        "the words changed and nobody said so: nothing is marked"
    );
}

fn deployment(fake: &Fake) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"

[[channels]]
id = "{CH}"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
        base = fake.base,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

async fn get(state: &AppState, uri: &str) -> Value {
    let request = Request::builder()
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    serde_json::from_slice(&bytes).expect("JSON")
}

/// The page's own reads, through a deployment assembled from configuration as `main` assembles one.
#[tokio::test]
async fn the_page_is_sent_the_edit_time_and_nothing_for_an_unedited_message() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let app = deployment(&fake);
    for uri in [
        format!("/api/v1/channels/{CH}/timeline?view=flat"),
        format!("/api/v1/channels/{CH}/page?render=html"),
    ] {
        let body = get(&app, &uri).await;
        let wire = |stamp: &str| -> Value {
            let id = vibe_talk::slack::message_id_from_ts(stamp).expect("encodable");
            body["messages"]
                .as_array()
                .expect("messages")
                .iter()
                .find(|message| message["id"] == json!(id.0))
                .unwrap_or_else(|| panic!("no message at {stamp} in {body}"))
                .get("edited_timestamp")
                .cloned()
                .unwrap_or(Value::Null)
        };
        assert_eq!(wire(&stamps.edited), json!(at(45)), "{uri}");
        assert_eq!(wire(&stamps.root), json!(at(35)), "{uri}");
        assert_eq!(
            wire(&stamps.plain),
            Value::Null,
            "{uri}: absent on the wire"
        );
        assert_eq!(wire(&stamps.vague), Value::Null, "{uri}");
        if uri.contains("flat") {
            assert_eq!(wire(&stamps.edited_reply), json!(at(33)), "{uri}");
        }
    }
}
