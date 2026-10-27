//! `#321 slack-emoji-shortcodes`: a standard emoji Slack writes as `:name:` reaches the page and
//! the voice as the emoji, on every way a Slack message arrives — history, Flat, a thread, the
//! live poller — while a custom emoji keeps its `:name:` and a post is sent exactly as written.
//!
//! Against the loopback Slack fake, with standard shortcodes, skin tones, a workspace's custom
//! emoji, a shortcode inside code, and a time and an address whose colons only look like one.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use slack_fake::{add_reply, profiled, thread, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::live::{poll_once, LiveHub};
use vibe_talk::model::{ChannelId, Message};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::{TimelineRequest, TimelineView};

const CH: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;

/// Standard shortcodes, as the read-only exercise found them.
const STANDARD: &str = "Found it :crab: thanks :slightly_smiling_face:";
/// Skin tones, and two emoji with no space between them.
const TONED: &str = ":wave::skin-tone-3: morning, :+1::skin-tone-5::+1:";
/// A workspace's custom emoji beside a standard one.
const CUSTOM: &str = ":party-parrot: shipped :tada:";
/// A shortcode inside code, and colons that only look like one.
const LITERAL: &str = "run `echo :crab:` at 10:100:20 on fe80::a:b";
/// A thread reply.
const REPLY: &str = ":white_check_mark: done";

fn standard() -> &'static str {
    "Found it 🦀 thanks \u{1F642}"
}
fn toned() -> &'static str {
    "👋🏼 morning, 👍🏾👍"
}
fn custom() -> &'static str {
    ":party-parrot: shipped 🎉"
}
fn reply() -> &'static str {
    "✅ done"
}

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// Four posts ten seconds apart, the first rooting a thread of one reply.
fn fixture() -> (FakeState, String) {
    let mut state = FakeState::default();
    let root = ts(10, 0);
    let mut list = vec![
        profiled(&root, "UADA00001", "Ada", STANDARD),
        profiled(&ts(20, 0), "UGRACE001", "Grace", TONED),
        profiled(&ts(30, 0), "UADA00001", "Ada", CUSTOM),
        profiled(&ts(40, 0), "UGRACE001", "Grace", LITERAL),
    ];
    thread(
        &mut state,
        CH,
        &mut list[0],
        &[(ts(11, 0), REPLY.to_owned())],
    );
    state.history.insert(CH.to_owned(), list);
    (state, root)
}

fn contents(messages: &[Message]) -> Vec<&str> {
    messages.iter().map(|m| m.content.as_str()).collect()
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

#[tokio::test]
async fn every_read_shows_standard_shortcodes_as_emoji() {
    let (state, root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();

    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert_eq!(contents(&page), [standard(), toned(), custom(), LITERAL]);

    let flat = client
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert_eq!(
        contents(&flat.messages),
        [standard(), reply(), toned(), custom(), LITERAL]
    );

    let thread = client
        .fetch_timeline(&channel(), &view(TimelineView::Thread, Some(root.as_str())))
        .await
        .expect("the thread");
    assert_eq!(contents(&thread.messages), [standard(), reply()]);

    // The thread list names the thread by its root's first line, emoji and all.
    let threads = client
        .fetch_timeline(&channel(), &view(TimelineView::Threads, None))
        .await
        .expect("the thread list");
    assert_eq!(
        threads
            .threads
            .iter()
            .map(|summary| summary.title.as_str())
            .collect::<Vec<_>>(),
        [standard()]
    );

    let recent = client
        .fetch_recent(&channel(), 50)
        .await
        .expect("the newest messages");
    assert!(contents(&recent).contains(&reply()), "{recent:?}");
}

#[tokio::test]
async fn the_voice_says_the_emoji_and_not_its_name_between_colons() {
    let (state, _root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let page = fake
        .client()
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    let zone = vibe_talk::clock::zone("UTC").expect("a bundled zone");
    let said = vibe_talk::speakable::for_speech(&page[0].content, 0, &zone);
    assert!(
        !said.contains(':') && !said.contains("slightly smiling face"),
        "the voice reads the shortcode's name: {said}"
    );
    assert!(said.contains('🦀') && said.contains('\u{1F642}'), "{said}");
}

#[tokio::test]
async fn the_live_poller_shows_standard_shortcodes_as_emoji() {
    let (state, root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let hub = LiveHub::new();
    let mut receiver = hub.subscribe(&channel(), None).receiver;
    let mut cursor = None;
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("the seeding poll");
    fake.with(|state| {
        state.history.get_mut(CH).expect("channel").push(profiled(
            &ts(50, 0),
            "UADA00001",
            "Ada",
            ":rocket: deploying :party-parrot:",
        ));
        add_reply(state, CH, &root, &ts(51, 0), ":eyes::skin-tone-2: looking");
    });
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("a poll");
    let mut published = Vec::new();
    while let Ok(live) = receiver.try_recv() {
        published.push(live.message.content);
    }
    published.sort();
    assert_eq!(published, ["👀 looking", "🚀 deploying :party-parrot:"]);
}

#[tokio::test]
async fn a_post_is_sent_exactly_as_written() {
    let (state, _root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let posted = client
        .post_message(&channel(), ":crab: on it :party-parrot:", None)
        .await
        .expect("posted");
    assert_eq!(
        fake.requests("chat.postMessage")[0]
            .get("text")
            .map(String::as_str),
        Some(":crab: on it :party-parrot:"),
        "the shortcode was changed in what Slack was sent"
    );
    // What Slack answers with is read as every read is.
    assert_eq!(posted.content, "🦀 on it :party-parrot:");
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

#[tokio::test]
async fn the_page_is_sent_the_emoji_and_the_code_as_written() {
    let (state, _root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let app = deployment(&fake);
    let flat = get(&app, &format!("/api/v1/channels/{CH}/timeline?view=flat")).await;
    let messages = flat["messages"].as_array().expect("messages");
    let wire: Vec<&str> = messages
        .iter()
        .map(|message| message["content"].as_str().expect("content"))
        .collect();
    assert_eq!(wire, [standard(), reply(), toned(), custom(), LITERAL]);
    // The code span is drawn as code, its shortcode as written.
    let literal = messages
        .iter()
        .find(|message| message["content"] == LITERAL)
        .expect("the literal message");
    let html = literal["content_html"].as_str().expect("rendered");
    assert!(html.contains("<code>echo :crab:</code>"), "{html}");
    assert!(!html.contains('🦀'), "{html}");
}
