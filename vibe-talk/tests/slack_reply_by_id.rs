//! `#319 slack-reply-by-id`: a Slack thread reply that a listing surfaced reads by its id.
//!
//! Slack's `conversations.history` holds no thread reply, so a lookup by id there proves a reply
//! absent, and `GET /messages/{id}` and the voice agent's `read_message` answered `unknown_message`
//! for a reply the digest, the to-do list and the catch-up had just listed. A by-id read that the
//! channel's own messages do not answer is now looked for once more in the thread this server
//! already knows the id to be a reply in (its held recent window, speech read-ahead, a pin, the
//! bookmark), and every listing names a reply's thread, so a caller can pass it back. Counted here
//! at the loopback Slack fake: one pinned read of history, then one of the thread, and nothing at all
//! read on a guess.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, thread, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::auth::Scope;
use vibe_talk::mcp::protocol::{dispatch, Outcome};
use vibe_talk::providers::ChatRouter;
use vibe_talk::slack::message_id_from_ts;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;

const TEAM: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;

/// A canonical `ts` this many seconds before now, so a catch-up of the last day sees it.
fn ago(seconds: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after the epoch")
        .as_secs();
    format!("{}.000000", i64::try_from(now).expect("now") - seconds)
}

/// The ids and thread of the fixture's messages.
struct Ids {
    /// The thread's root `ts`, which is the thread id.
    root: String,
    /// The thread's two replies.
    reply: String,
    second: String,
    /// One of the channel's own messages.
    plain: String,
}

fn id_of(ts: &str) -> String {
    message_id_from_ts(ts).expect("encodable").0
}

/// Three messages of the channel's own, minutes old; the middle one roots a thread of two replies.
fn fixture() -> (FakeState, Ids) {
    let mut state = FakeState::default();
    let root = ago(600);
    let (reply, second) = (ago(590), ago(580));
    let mut list = vec![
        profiled(&ago(900), "UADA00001", "Ada", "good morning"),
        profiled(&root, "UADA00001", "Ada", "deploy started"),
        profiled(&ago(300), "UGRACE001", "Grace", "lunch at noon"),
    ];
    thread(
        &mut state,
        TEAM,
        &mut list[1],
        &[
            (reply.clone(), "build is green".to_owned()),
            (second.clone(), "tests pass too".to_owned()),
        ],
    );
    state.history.insert(TEAM.to_owned(), list);
    let ids = Ids {
        root,
        reply: id_of(&reply),
        second: id_of(&second),
        plain: id_of(&ago(900)),
    };
    (state, ids)
}

/// A Slack deployment of one channel, assembled as `main` assembles one, with nothing read yet.
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
id = "{TEAM}"
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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
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

async fn get(state: &AppState, uri: &str) -> (StatusCode, Value) {
    call(state, "GET", uri, None).await
}

/// One MCP tool call's text, and whether it was an error result.
async fn tool(state: &AppState, name: &str, arguments: Value) -> (String, bool) {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments },
    });
    let Outcome::Reply(response) = dispatch(state, Scope::Read, &request).await else {
        panic!("expected a reply");
    };
    let result = response.result.as_ref().expect("a result");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned();
    (text, result["isError"] == true)
}

/// The line of a tool's answer that carries `text`.
fn line_of<'a>(answer: &'a str, text: &str) -> &'a str {
    answer
        .lines()
        .find(|line| line.contains(text))
        .unwrap_or_else(|| panic!("no line holds {text:?}: {answer}"))
}

/// The entry of a digest or catch-up whose summary is `text`.
fn entry<'a>(entries: &'a Value, text: &str) -> &'a Value {
    entries
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["summary"] == text)
        .unwrap_or_else(|| panic!("no entry says {text:?}: {entries}"))
}

/// What the fake has been asked so far: (history reads, replies reads).
fn reads(fake: &Fake) -> (usize, usize) {
    (
        fake.calls("conversations.history"),
        fake.calls("conversations.replies"),
    )
}

#[tokio::test]
async fn a_reply_the_digest_listed_reads_by_id_without_its_thread() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);
    let base = format!("/api/v1/channels/{TEAM}");

    let (status, digest) = get(&server, &format!("{base}/digest")).await;
    assert_eq!(status, StatusCode::OK, "{digest}");
    let entries = &digest["entries"];
    let reply = entry(entries, "build is green");
    assert_eq!(reply["id"], ids.reply);
    assert_eq!(
        reply["thread_id"], ids.root,
        "a reply's entry names its thread"
    );
    for own in ["good morning", "deploy started", "lunch at noon"] {
        assert!(
            entry(entries, own).get("thread_id").is_none(),
            "{own:?} is one of the channel's own messages, a root included: {entries}"
        );
    }

    // By its id alone, as the digest gave it: one pinned read of history, which proves it is not
    // among the channel's own messages, and one of the thread the digest's read said it is in.
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.reply)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "build is green");
    assert_eq!(read["message"]["thread"]["id"], ids.root);
    assert_eq!(read["message"]["thread"]["is_root"], false);
    assert_eq!(reads(&fake), (before.0 + 1, before.1 + 1));
    let asked = fake.requests("conversations.replies");
    let last = asked.last().expect("a replies read");
    assert_eq!(last["ts"], ids.root, "the thread it is in, and no other");

    // The same read with the thread given goes straight to it.
    let before = reads(&fake);
    let (status, read) = get(
        &server,
        &format!("{base}/messages/{}?thread_id={}", ids.second, ids.root),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "tests pass too");
    assert_eq!(reads(&fake), (before.0, before.1 + 1));

    // One of the channel's own messages is still one read.
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.plain)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "good morning");
    assert_eq!(reads(&fake), (before.0 + 1, before.1));
}

/// Negative controls: nothing is read on a guess, and a reply deleted since it was listed is a miss.
#[tokio::test]
async fn an_id_no_listing_placed_in_a_thread_is_a_miss_after_one_read() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);
    let base = format!("/api/v1/channels/{TEAM}");
    let (status, _) = get(&server, &format!("{base}/digest")).await;
    assert_eq!(status, StatusCode::OK);

    // An instant between two messages: nothing there, in the channel or in any thread.
    let nothing = id_of(&ago(450));
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{nothing}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{read}");
    assert_eq!(read["error"], "unknown_message");
    assert_eq!(
        reads(&fake),
        (before.0 + 1, before.1),
        "no thread was read for an id no listing placed in one"
    );

    // The reply deleted in Slack after the digest listed it: its thread is read, and has it no more.
    fake.with(|state| {
        let key = (TEAM.to_owned(), ids.root.clone());
        state
            .replies
            .get_mut(&key)
            .expect("the thread")
            .retain(|reply| reply["text"] != "build is green");
    });
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.reply)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{read}");
    assert_eq!(read["error"], "unknown_message");
    assert_eq!(reads(&fake), (before.0 + 1, before.1 + 1));
}

#[tokio::test]
async fn the_todo_list_and_the_catch_up_name_a_replys_thread() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);

    let (status, todo) = get(&server, &format!("/api/v1/channels/{TEAM}/todo")).await;
    assert_eq!(status, StatusCode::OK, "{todo}");
    let messages = todo["messages"].as_array().expect("messages");
    let reply = messages
        .iter()
        .find(|message| message["id"] == ids.reply)
        .unwrap_or_else(|| panic!("the reply is not on the list: {todo}"));
    assert_eq!(reply["thread"]["id"], ids.root);
    assert_eq!(reply["thread"]["is_root"], false);

    let (status, activity) = get(&server, "/api/v1/activity").await;
    assert_eq!(status, StatusCode::OK, "{activity}");
    let entries = &activity["channels"][0]["entries"];
    assert_eq!(entry(entries, "build is green")["thread_id"], ids.root);
    assert_eq!(entry(entries, "tests pass too")["thread_id"], ids.root);
    assert!(entry(entries, "deploy started").get("thread_id").is_none());
    assert!(
        activity["newest"]["entry"].get("thread_id").is_none(),
        "the newest is one of the channel's own: {activity}"
    );

    // And each reads by the id the list gave it.
    for (id, text) in [
        (&ids.reply, "build is green"),
        (&ids.second, "tests pass too"),
    ] {
        let (status, read) = get(&server, &format!("/api/v1/channels/{TEAM}/messages/{id}")).await;
        assert_eq!(status, StatusCode::OK, "{read}");
        assert_eq!(read["message"]["content"], text);
    }
}

#[tokio::test]
async fn the_voice_agent_reads_a_reply_it_was_told_about() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);

    let (catch_up, failed) = tool(&server, "recent_activity", json!({})).await;
    assert!(!failed, "{catch_up}");
    let thread = format!(" | thread {} | ", ids.root);
    assert!(
        line_of(&catch_up, "build is green").contains(&thread),
        "{catch_up}"
    );
    for own in ["good morning", "deploy started", "lunch at noon"] {
        assert!(
            !line_of(&catch_up, own).contains("| thread"),
            "{own:?}: {catch_up}"
        );
    }

    // What the agent was told is what it may read, by the id alone.
    let (read, failed) = tool(
        &server,
        "read_message",
        json!({ "channel_id": TEAM, "message_id": ids.reply }),
    )
    .await;
    assert!(!failed, "{read}");
    assert!(read.contains("build is green"), "{read}");
    assert!(
        read.contains(&thread),
        "the reply read says its thread too: {read}"
    );

    let (digest, failed) = tool(&server, "digest_channel", json!({ "channel_id": TEAM })).await;
    assert!(!failed, "{digest}");
    assert!(
        line_of(&digest, "tests pass too").contains(&thread),
        "{digest}"
    );
    assert!(
        !line_of(&digest, "deploy started").contains("| thread"),
        "{digest}"
    );
}

/// What a listing names is enough on its own: a server holding nothing reads the reply in the
/// thread the agent passes back, and, given none, reads nothing on a guess.
#[tokio::test]
async fn read_message_takes_the_thread_a_line_gave() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);

    let (read, failed) = tool(
        &server,
        "read_message",
        json!({ "channel_id": TEAM, "message_id": ids.reply, "thread_id": ids.root }),
    )
    .await;
    assert!(!failed, "{read}");
    assert!(read.contains("build is green"), "{read}");
    assert_eq!(reads(&fake), (0, 1), "one read, of the thread it was given");

    let cold = deployment(&fake);
    let before = reads(&fake);
    let (read, failed) = tool(
        &cold,
        "read_message",
        json!({ "channel_id": TEAM, "message_id": ids.second }),
    )
    .await;
    assert!(failed, "{read}");
    assert!(read.starts_with("unknown_message"), "{read}");
    assert_eq!(
        reads(&fake),
        (before.0 + 1, before.1),
        "nothing listed it on this server, so no thread was read"
    );

    // The manifest offers the argument.
    let request = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let Outcome::Reply(response) = dispatch(&cold, Scope::Read, &request).await else {
        panic!("expected a reply");
    };
    let tools = response.result.as_ref().expect("a result")["tools"]
        .as_array()
        .expect("tools")
        .clone();
    let read_message = tools
        .iter()
        .find(|tool| tool["name"] == "read_message")
        .expect("read_message");
    assert_eq!(
        read_message["inputSchema"]["properties"]["thread_id"]["type"],
        "string"
    );
    assert_eq!(
        read_message["inputSchema"]["required"],
        json!(["channel_id", "message_id"])
    );
}

/// The store knows a pinned reply's thread, and the bookmark's: a server that has read nothing
/// finds either by its id alone. A thread the store names that Slack does not have is a miss.
#[tokio::test]
async fn a_pinned_or_bookmarked_reply_reads_by_id_on_a_server_that_read_nothing() {
    let (state, ids) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = deployment(&fake);
    let base = format!("/api/v1/channels/{TEAM}");

    let (status, pinned) = call(
        &server,
        "PUT",
        &format!("{base}/pins/{}", ids.reply),
        Some(json!({
            "author": "Replier", "author_id": "UREPLIER1", "content": "build is green",
            "timestamp": "2026-10-14T09:00:00Z", "thread_id": ids.root, "thread_root": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pinned}");
    assert_eq!(reads(&fake), (0, 0), "pinning reads nothing");
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.reply)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "build is green");
    assert_eq!(reads(&fake), (1, 1));

    let (status, marked) = call(
        &server,
        "PUT",
        &format!("{base}/bookmark"),
        Some(json!({
            "message_id": ids.second, "timestamp": "2026-10-14T09:00:10Z", "thread_id": ids.root,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{marked}");
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.second)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "tests pass too");

    // A pin whose thread is not one Slack has (the root of none): Slack answers thread_not_found,
    // which is the same miss the channel gave, not a failure of the chat service.
    let (status, _) = call(
        &server,
        "PUT",
        &format!("{base}/pins/{}", ids.plain),
        Some(json!({
            "author": "Ada", "author_id": "UADA00001", "content": "good morning",
            "timestamp": "2026-10-14T09:00:00Z", "thread_id": ago(10), "thread_root": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let stray = id_of(&ago(5));
    let (status, _) = call(
        &server,
        "PUT",
        &format!("{base}/pins/{stray}"),
        Some(json!({
            "author": "Ada", "author_id": "UADA00001", "content": "gone",
            "timestamp": "2026-10-14T09:00:00Z", "thread_id": ago(10), "thread_root": false,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{stray}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{read}");
    assert_eq!(read["error"], "unknown_message");
    assert_eq!(reads(&fake), (before.0 + 1, before.1 + 1));
    // One of the channel's own messages the store says is a reply is still read where it is, in
    // one read: what the store knows is asked only after the channel has not answered.
    let before = reads(&fake);
    let (status, read) = get(&server, &format!("{base}/messages/{}", ids.plain)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["message"]["content"], "good morning");
    assert_eq!(reads(&fake), (before.0 + 1, before.1));
}
