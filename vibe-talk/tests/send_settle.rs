//! `#283 send-settle-in-task`: what a send posted is filed in the send memory whether or not its
//! caller is still waiting for the answer.
//!
//! A confirmed proposal and a direct send are each entered in the post gate's send memory when they
//! start, and settled with what they posted (`#270 voice-transcript-markers`): the voice page draws
//! a card from that record, freezes it, and replays it to the next call. Since `#277 send-order` the
//! post itself runs in a task of its own, which outlives a caller that goes away, but the record was
//! still settled by the caller. So a caller gone mid-post filed a post that then landed as "not
//! confirmed", and one gone while it waited for its destination's turn filed words that were never
//! sent. The post's own task settles it now, exactly once.
//!
//! Every test drives the real routes — the commit route and the MCP `post_reply` — against a
//! provider that holds each post until the test lets it go. A caller going away is its request
//! dropped, as axum drops a handler whose connection closed.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::state::AppState;
use vibe_talk::store::SendOutcome;
use vibe_talk::testing::{self, WRITE_CHANNEL, WRITE_TOKEN};

/// A provider that lets each post through only when the test gives the gate a permit, and counts
/// the posts that reached it before they wait.
struct HeldPosts {
    inner: Arc<FakeDiscord>,
    gate: Arc<Semaphore>,
    reached: Mutex<usize>,
}

impl HeldPosts {
    async fn hold(&self) {
        *self.reached.lock().expect("reached") += 1;
        self.gate
            .acquire()
            .await
            .expect("the test keeps the gate open")
            .forget();
    }
}

#[async_trait]
impl ChatClient for HeldPosts {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.inner.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.hold().await;
        self.inner.post_message(channel, content, reply_to).await
    }
}

struct Harness {
    state: AppState,
    held: Arc<HeldPosts>,
    discord: Arc<FakeDiscord>,
}

/// The server, with a store (so confirmation can be turned off) and every post held at the
/// provider until `held.gate` has a permit for it.
fn harness() -> Harness {
    let (mut state, discord, _store) = testing::state_with_store();
    let held = Arc::new(HeldPosts {
        inner: Arc::clone(&discord),
        gate: Arc::new(Semaphore::new(0)),
        reached: Mutex::new(0),
    });
    state.replace_chat(Arc::clone(&held) as Arc<dyn ChatClient>);
    Harness {
        state,
        held,
        discord,
    }
}

/// `method uri` with the write token, as a caller of its own: a task the test can drop.
fn call(
    harness: &Harness,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    let app = vibe_talk::http::router(harness.state.clone());
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .header("accept", "application/json, text/event-stream");
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
    .expect("request");
    tokio::spawn(async move {
        let response = app.oneshot(request).await.expect("router responds");
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
    })
}

/// The same, waited for.
async fn answered(harness: &Harness, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (status, body) = call(harness, method, uri, body)
        .await
        .expect("the caller stayed");
    assert_eq!(status, StatusCode::OK, "{method} {uri}: {body}");
    body
}

/// The voice agent's `post_reply`, as a caller of its own.
fn post_reply(harness: &Harness, text: &str) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    call(
        harness,
        "POST",
        "/mcp",
        Some(json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "post_reply",
                "arguments": { "channel_id": WRITE_CHANNEL, "text": text },
            },
        })),
    )
}

fn tool_text(response: &Value) -> String {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// A proposal pending for the owner, and the commit body that sends it as proposed.
async fn proposed(harness: &Harness, text: &str) -> Value {
    let (status, response) = post_reply(harness, text).await.expect("the caller stayed");
    assert_eq!(status, StatusCode::OK, "{response}");
    assert!(tool_text(&response).starts_with("NOT SENT."), "{response}");
    let pending =
        answered(harness, "GET", "/api/v1/post-proposals", None).await["proposal"].clone();
    json!({
        "handle": pending["handle"],
        "channel_id": pending["channel_id"],
        "text": pending["text"],
        "reply_to": pending["reply_to"],
        "confirmed_by": "ui",
    })
}

/// Confirmation off: `post_reply` posts at once.
async fn sending_at_once(harness: &Harness) {
    answered(
        harness,
        "PUT",
        "/api/v1/voice-settings",
        Some(json!({ "ask_before_send": false })),
    )
    .await;
}

/// Let every task that can run, run, and give anything waiting on a clock a moment.
async fn quiet() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
}

/// Wait for a send started by `door` to be waiting for its destination's turn: a commit has spent
/// its handle by then. A direct send says nothing until it settles, so it is given a moment.
async fn waiting_for_its_turn(harness: &Harness, door: Door) {
    quiet().await;
    if let Door::Commit = door {
        for _ in 0..300 {
            let pending = answered(harness, "GET", "/api/v1/post-proposals", None).await;
            if pending["proposal"].is_null() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the commit never spent its handle");
    }
}

/// Wait for `posts` posts to have reached the provider, where they are held.
async fn reached(harness: &Harness, posts: usize) {
    for _ in 0..300 {
        if *harness.held.reached.lock().expect("reached") >= posts {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{posts} post(s) never reached the provider");
}

/// What the send memory has filed, and the newest `send_seq`.
fn filed(harness: &Harness) -> (Vec<vibe_talk::post_gate::VoiceSendRecord>, u64) {
    harness.state.post_gate.sends_after(0)
}

/// Wait for the send memory to file `count` sends.
async fn filed_count(
    harness: &Harness,
    count: usize,
) -> Vec<vibe_talk::post_gate::VoiceSendRecord> {
    for _ in 0..300 {
        let (sends, _) = filed(harness);
        if sends.len() >= count {
            return sends;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "the send memory never filed {count} send(s): {:?}",
        filed(harness)
    );
}

/// The two doors a send the send memory keeps comes in by.
#[derive(Clone, Copy, Debug)]
enum Door {
    /// The owner confirmed a proposal: the commit route.
    Commit,
    /// Confirmation is off: the voice agent's `post_reply` posted at once.
    Direct,
}

const DOORS: [Door; 2] = [Door::Commit, Door::Direct];

/// Start a send of `text` by `door`, as a caller that may go away.
async fn start(
    harness: &Harness,
    door: Door,
    text: &str,
) -> tokio::task::JoinHandle<(StatusCode, Value)> {
    match door {
        Door::Commit => {
            let body = proposed(harness, text).await;
            call(harness, "POST", "/api/v1/post-proposals/commit", Some(body))
        }
        Door::Direct => {
            sending_at_once(harness).await;
            post_reply(harness, text)
        }
    }
}

#[tokio::test]
async fn a_send_whose_caller_goes_away_while_it_is_posted_is_filed_with_what_it_posted() {
    for door in DOORS {
        let harness = harness();
        let caller = start(&harness, door, "on my way").await;
        reached(&harness, 1).await;
        // The caller goes away while the provider is still working on the post.
        caller.abort();
        quiet().await;
        assert_eq!(
            filed(&harness).1,
            0,
            "{door:?}: the send was filed before the provider answered"
        );
        // The provider answers: the post lands, and that is what is filed.
        harness.held.gate.add_permits(1);
        let sends = filed_count(&harness, 1).await;
        let posted = harness.discord.posted();
        assert_eq!(posted.len(), 1, "{door:?}: the post did not land");
        assert_eq!(sends.len(), 1, "{door:?}: {sends:?}");
        let record = &sends[0].record;
        assert_eq!(
            record.outcome,
            SendOutcome::Sent,
            "{door:?}: a post that landed after its caller went away was filed as {:?}",
            record.outcome
        );
        assert_eq!(record.parts_posted, 1, "{door:?}");
        assert!(
            record.message_id.is_some(),
            "{door:?}: the landed post cannot be opened from its card"
        );
        assert!(
            sends[0].text.ends_with("on my way"),
            "{door:?}: {}",
            sends[0].text
        );
        quiet().await;
        assert_eq!(filed(&harness).1, 1, "{door:?}: the send was filed twice");
    }
}

#[tokio::test]
async fn a_send_whose_caller_goes_away_while_it_waits_for_its_turn_files_nothing() {
    for door in DOORS {
        let harness = harness();
        // The destination held, as a post ahead of this one would hold it.
        let turn = harness.state.post_lanes.turn(WRITE_CHANNEL, None).await;
        let caller = start(&harness, door, "on my way").await;
        waiting_for_its_turn(&harness, door).await;
        assert_eq!(
            *harness.held.reached.lock().expect("reached"),
            0,
            "{door:?}: the post went while its destination was held"
        );
        caller.abort();
        quiet().await;
        drop(turn);
        quiet().await;
        assert!(
            harness.discord.posted().is_empty(),
            "{door:?}: a send given up while it waited was posted"
        );
        assert_eq!(
            filed(&harness),
            (Vec::new(), 0),
            "{door:?}: words that were never sent were filed"
        );
    }
    // A direct send given up while it waited is forgotten, not remembered as on its way or
    // unconfirmed: the same words go at once when the voice agent asks again.
    let harness = harness();
    let turn = harness.state.post_lanes.turn(WRITE_CHANNEL, None).await;
    let caller = start(&harness, Door::Direct, "on my way").await;
    quiet().await;
    caller.abort();
    quiet().await;
    drop(turn);
    harness.held.gate.add_permits(1);
    let (status, again) = post_reply(&harness, "on my way")
        .await
        .expect("the caller stayed");
    assert_eq!(status, StatusCode::OK, "{again}");
    assert!(
        tool_text(&again).starts_with("SENT."),
        "the same words were refused as a repeat of a send that posted nothing: {again}"
    );
    assert_eq!(harness.discord.posted().len(), 1);
    assert_eq!(filed(&harness).1, 1);
}

#[tokio::test]
async fn a_send_whose_caller_goes_away_after_the_post_is_filed_once() {
    for door in DOORS {
        let harness = harness();
        let caller = start(&harness, door, "on my way").await;
        reached(&harness, 1).await;
        harness.held.gate.add_permits(1);
        let sends = filed_count(&harness, 1).await;
        // The provider has answered and the send is filed; the caller goes away before it has read
        // the answer, or after.
        caller.abort();
        quiet().await;
        let (after, through) = filed(&harness);
        assert_eq!(
            through, 1,
            "{door:?}: the send was filed again when its caller went"
        );
        assert_eq!(
            after, sends,
            "{door:?}: what was filed changed when its caller went"
        );
        assert_eq!(after[0].record.outcome, SendOutcome::Sent, "{door:?}");
    }
    // And a caller that stays is answered with the number its send was filed under.
    let harness = harness();
    let caller = start(&harness, Door::Commit, "on my way").await;
    harness.held.gate.add_permits(1);
    let (status, body) = caller.await.expect("the caller stayed");
    assert_eq!(status, StatusCode::OK, "{body}");
    let (sends, through) = filed(&harness);
    assert_eq!(through, 1);
    assert_eq!(body["send_seq"], sends[0].record.send_seq, "{body}");
}
