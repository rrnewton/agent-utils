//! The send log beyond one process and one answer, related to `#468 send-dedupe`.
//!
//! - `#465 replication-c6-op-key-marker`, part 2. The owner, 2026-10-23: after an uncertain send,
//!   look upstream before offering Retry. A part whose answer was lost after the provider took it
//!   is looked for in its destination: found by its key, it landed, and the send is answered as
//!   posted, with nothing posted twice.
//! - `#471 replication-c5-post-keys`. What a keyed post landed as is filed in the store, so a
//!   repeat after a restart is answered with it and posts nothing.
//!
//! Against the loopback Slack fake (which keeps a post's metadata, as a transport passing it
//! through would) and the in-memory provider. Matching by words is the pure
//! `ops::match_uncertain_part`, tested at the end.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::ops::{match_uncertain_part, UNCERTAIN_PART_SKEW_MS};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, WRITE_TOKEN};

const TEAM: &str = "C0000MAIN1";
const KEY: &str = "outgoing-key_465";

async fn call(state: &AppState, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
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

async fn slack(fake: &Fake) -> AppState {
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

[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
        read = testing::READ_TOKEN,
        base = fake.base,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

fn fixture(lost: Vec<usize>) -> FakeState {
    let mut state = FakeState {
        lose_post_answers: lost,
        ..FakeState::default()
    };
    state.history.insert(
        TEAM.to_owned(),
        vec![profiled(
            &ts(10, 0),
            "UADA00001",
            "Ada",
            "is the build green?",
        )],
    );
    state
}

fn reply() -> String {
    format!("/api/v1/channels/{TEAM}/reply")
}

async fn get(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
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

/// `#934 voice-page-reply-parent-arrow`. The owner: "When I reply or quote reply especially inside
/// a thread. I'm not getting a little blue arrows to blink it back to the parent." Slack keeps no
/// reply pointer, so this server keeps its own: a reply and a quote-reply in the channel (which
/// Slack posts in their parents' threads) and a reply and a quote-reply inside a thread each come
/// back from every read saying what they answer, and still do after a restart: by our metadata in
/// Slack itself, where the transport passes it both ways, and by this server's own record where it
/// does not.
#[tokio::test]
async fn slack_replies_and_quote_replies_keep_their_parents_on_every_read_and_after_a_restart() {
    for drops_metadata in [false, true] {
        replies_keep_their_parents(drops_metadata).await;
    }
}

async fn replies_keep_their_parents(drops_metadata: bool) {
    let transport = if drops_metadata {
        "a transport dropping metadata"
    } else {
        "Slack"
    };
    let mut fixture = FakeState {
        drops_metadata,
        ..fixture(vec![])
    };
    let mut root = profiled(&ts(20, 0), "UADA00001", "Ada", "shall we ship?");
    let answer = ts(21, 0);
    slack_fake::thread(
        &mut fixture,
        TEAM,
        &mut root,
        &[(answer.clone(), "after lunch".to_owned())],
    );
    let history = fixture.history.get_mut(TEAM).expect("the channel");
    history.push(profiled(&ts(15, 0), "UADA00001", "Ada", "deploy tonight?"));
    history.push(root);
    let fake = Fake::start(fixture, 50).await;
    let state = slack(&fake).await;
    let id = |ts: &str| vibe_talk::slack::message_id_from_ts(ts).expect("an id").0;
    let (status, main) = get(
        &state,
        &format!("/api/v1/channels/{TEAM}/timeline?view=main"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{main}");
    let in_thread = main["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["id"] == json!(id(&ts(20, 0))))
        .and_then(|m| m["thread"]["id"].as_str())
        .expect("the root heads its thread")
        .to_owned();
    // (what, text, the parent, posted inside the thread)
    let cases = [
        (
            "a reply in the channel",
            "yes, it is green",
            id(&ts(10, 0)),
            false,
        ),
        (
            "a quote-reply in the channel",
            "> deploy tonight?\nno, tomorrow",
            id(&ts(15, 0)),
            false,
        ),
        ("a reply inside a thread", "suits me", id(&answer), true),
        (
            "a quote-reply inside a thread",
            "> after lunch\nfine by me",
            id(&answer),
            true,
        ),
    ];
    let mut posted = Vec::new();
    for (n, (what, text, parent, inside)) in cases.iter().enumerate() {
        let mut body =
            json!({ "text": text, "reply_to": parent, "idempotency_key": format!("k-934-{n}") });
        if *inside {
            body["thread_id"] = json!(in_thread);
        }
        let (status, answered) = call(&state, &reply(), body).await;
        assert_eq!(status, StatusCode::OK, "{what}: {answered}");
        assert_eq!(
            answered["posted"]["reply_to"],
            json!(parent),
            "{what}: the posted reply does not say what it answers"
        );
        let thread = answered["posted"]["thread"]["id"]
            .as_str()
            .expect("posted in a thread")
            .to_owned();
        posted.push((
            *what,
            thread,
            answered["posted"]["id"].clone(),
            parent.clone(),
        ));
    }
    assert_eq!(fake.calls("chat.postMessage"), cases.len());
    // Each post names its parent in our metadata, for every reader of Slack that returns it.
    let filed: Vec<Value> = fake
        .requests("chat.postMessage")
        .iter()
        .map(|params| serde_json::from_str(&params["metadata"]).expect("metadata as JSON"))
        .collect();
    for ((what, _, _, parent), filed) in posted.iter().zip(&filed) {
        // Exactly `event_type` (text) and `event_payload` (an object), within 8 KB: a bridge in
        // front of Slack refuses any other shape, and the post with it.
        let fields = filed.as_object().expect("metadata is an object");
        assert_eq!(fields.len(), 2, "{what}: {filed}");
        assert!(
            fields["event_type"].is_string() && fields["event_payload"].is_object(),
            "{what}: {filed}"
        );
        assert!(filed.to_string().len() < 8 * 1024, "{what}: {filed}");
        assert_eq!(
            filed["event_payload"]["reply_to"],
            json!(parent),
            "{transport}: {what}: {filed}"
        );
    }
    // Every read of each thread, and after a restart: the same store, the pointers read back.
    let mut restarted = state.clone();
    restarted.reply_pointers = Arc::new(vibe_talk::reply_pointers::ReplyPointers::new());
    assert_eq!(
        restarted.restore_reply_pointers().await.expect("restored"),
        cases.len()
    );
    for (server, when) in [(&state, "a read"), (&restarted, "a read after a restart")] {
        for (what, thread, reply_id, parent) in &posted {
            let encoded = thread.replace(':', "%3A");
            let (status, page) = get(
                server,
                &format!("/api/v1/channels/{TEAM}/timeline?view=thread&thread_id={encoded}"),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{page}");
            let found = page["messages"]
                .as_array()
                .expect("messages")
                .iter()
                .find(|m| m["id"] == *reply_id)
                .unwrap_or_else(|| {
                    panic!("{transport}: {when} of the thread does not hold {what}: {page}")
                });
            assert_eq!(
                found["reply_to"],
                json!(parent),
                "{transport}: {when} lost what {what} answers"
            );
        }
    }
}

#[tokio::test]
async fn a_slack_post_whose_answer_was_lost_is_found_by_its_key_and_answered_as_posted() {
    let fake = Fake::start(fixture(vec![1]), 50).await;
    let state = slack(&fake).await;
    let (status, answer) = call(
        &state,
        &reply(),
        json!({ "text": "yes, it is green", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(fake.calls("chat.postMessage"), 1);
    // The answer names the message Slack holds.
    let held = fake.with(|state| {
        state.history[TEAM]
            .last()
            .and_then(|m| m["ts"].as_str())
            .map(str::to_owned)
            .expect("posted")
    });
    let id = answer["parts"][0]["id"].as_str().expect("an id").to_owned();
    assert_eq!(
        vibe_talk::slack::ts_from_message_id(&MessageId(id)).as_deref(),
        Some(held.as_str())
    );
    // Retry asks again and is answered from the send log: nothing is posted twice.
    let (status, again) = call(
        &state,
        &reply(),
        json!({ "text": "yes, it is green", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(fake.calls("chat.postMessage"), 1, "Retry posted it twice");
}

#[tokio::test]
async fn a_repeat_after_a_restart_is_answered_from_the_store_and_posts_nothing() {
    let fake = Fake::start(fixture(vec![]), 50).await;
    let state = slack(&fake).await;
    let words = json!({ "text": "deploying now", "idempotency_key": KEY });
    let (status, first) = call(&state, &reply(), words.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(fake.calls("chat.postMessage"), 1);
    // A restart: the same store, an empty send log, read back as the server does at start.
    let mut restarted = state.clone();
    restarted.post_keys = Arc::new(vibe_talk::post_keys::PostKeys::new());
    assert_eq!(restarted.restore_post_keys().await.expect("restored"), 1);
    let (status, again) = call(&restarted, &reply(), words).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        again["parts"], first["parts"],
        "answered with the first post"
    );
    assert_eq!(
        fake.calls("chat.postMessage"),
        1,
        "a repeat after a restart posted it again"
    );
    // Other words under the same key are still refused, after the restart as before it.
    let (status, reused) = call(
        &restarted,
        &reply(),
        json!({ "text": "something else", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{reused}");
    assert_eq!(fake.calls("chat.postMessage"), 1);
}

#[tokio::test]
async fn the_purge_forgets_the_send_log_in_memory_and_in_the_store() {
    let fake = Fake::start(fixture(vec![]), 50).await;
    let state = slack(&fake).await;
    let words = json!({ "text": "deploying now", "idempotency_key": KEY });
    let (status, _) = call(&state, &reply(), words.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let request = Request::builder()
        .method("DELETE")
        .uri("/api/v1/storage")
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        state
            .store
            .kept_post_keys(0)
            .await
            .expect("read")
            .is_empty(),
        "the purge left the owner's words in the store"
    );
    // Erased means forgotten: the same send is a new post now, as it would be with a new key.
    let (status, _) = call(&state, &reply(), words).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fake.calls("chat.postMessage"), 2);
}

#[tokio::test]
async fn a_lost_answer_whose_post_cannot_be_seen_stays_uncertain() {
    // The transport drops metadata, and the fake's dates are older than the request, so neither
    // the key nor the words can find it: the answer stays what it was.
    let mut state = fixture(vec![1]);
    state.drops_metadata = true;
    let fake = Fake::start(state, 50).await;
    let state = slack(&fake).await;
    let (status, answer) = call(
        &state,
        &reply(),
        json!({ "text": "yes, it is green", "idempotency_key": KEY }),
    )
    .await;
    assert_ne!(status, StatusCode::OK, "{answer}");
    assert_eq!(fake.calls("chat.postMessage"), 1);
}

#[tokio::test]
async fn the_in_memory_provider_with_a_readable_carrier_is_found_by_key() {
    use vibe_talk::chat::{OpKeyCarrier, OpKeyDedupe};
    let (state, discord, _store) = testing::state_with_store();
    discord.carry_op_keys(OpKeyCarrier {
        carried: true,
        dedupes: OpKeyDedupe::Never,
        readable: true,
    });
    discord.lose_answer_after(0, "timed out");
    let (status, answer) = call(
        &state,
        &format!("/api/v1/channels/{}/reply", testing::WRITE_CHANNEL),
        json!({ "text": "on it", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(discord.posted().len(), 1);
}

fn message(id: &str, author: &str, text: &str, at_ms: i64, op_key: Option<&str>) -> Message {
    let mut message: Message = serde_json::from_value(json!({
        "id": id, "channel_id": "C1", "author": author, "author_id": author,
        "author_is_bot": false,
        "timestamp": vibe_talk::clock::iso_from_ms(at_ms),
        "content": text,
    }))
    .expect("a message");
    message.op_key = op_key.map(str::to_owned);
    message.author_id = UserId(author.to_owned());
    message
}

#[test]
fn the_words_match_only_one_new_message_by_this_account_with_no_other_key() {
    let began = 1_790_000_000_000;
    let mine = |id: &str, text: &str, at: i64| message(id, "me", text, at, None);
    let none_ours = |_: &MessageId| false;
    let find = |read: &[Message], part: &str, key: Option<&str>, readable: bool| {
        match_uncertain_part(read, part, key, readable, Some("me"), began, none_ours)
    };
    // Found: the one new message by this account with the part's words, whitespace aside.
    let read = [
        mine("a", "yes,  it is\ngreen", began + 900),
        message("b", "ada", "is the build green?", began - 60_000, None),
    ];
    let found = find(&read, "yes, it is green", Some(KEY), false).expect("found");
    assert_eq!(found.id.0, "a");
    // Not: an older post with the same words.
    let older = [mine(
        "a",
        "yes, it is green",
        began - UNCERTAIN_PART_SKEW_MS - 1,
    )];
    assert!(find(&older, "yes, it is green", None, false).is_none());
    // Not: the same words this server already saw land as its own post a moment before, whose
    // answer came back: the owner sending "ok" twice, the second never posted.
    let earlier = [mine("a", "ok", began + 1)];
    assert!(
        match_uncertain_part(&earlier, "ok", None, false, Some("me"), began, |id| id.0
            == "a")
        .is_none(),
        "an earlier send of the same words was taken for this one"
    );
    // Not: someone else's words.
    let theirs = [message("a", "ada", "yes, it is green", began + 10, None)];
    assert!(find(&theirs, "yes, it is green", None, false).is_none());
    // Not: two that match; which is ours cannot be told.
    let two = [mine("a", "ok", began + 1), mine("b", "ok", began + 2)];
    assert!(find(&two, "ok", None, false).is_none());
    // Not: a message carrying another key.
    let other = [message("a", "me", "ok", began + 1, Some("someone-else"))];
    assert!(find(&other, "ok", Some(KEY), false).is_none());
    // Where reads return every key and this part carried one, no key means not posted.
    let unkeyed = [mine("a", "ok", began + 1)];
    assert!(find(&unkeyed, "ok", Some(KEY), true).is_none());
    // Not by words at all where the provider does not say which account is this server's.
    assert!(match_uncertain_part(&unkeyed, "ok", None, false, None, began, none_ours).is_none());
    // By key, exactly, whatever its words, time or account.
    let keyed = [message(
        "z",
        "ada",
        "edited since",
        began - 99_999,
        Some(KEY),
    )];
    assert_eq!(
        match_uncertain_part(&keyed, "ok", Some(KEY), true, None, began, none_ours)
            .expect("found")
            .id
            .0,
        "z"
    );
    let _ = ChannelId("C1".to_owned());
}
