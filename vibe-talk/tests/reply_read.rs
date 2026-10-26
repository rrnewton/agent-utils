//! `#300 reply-marks-read`, the server's half, through the real router.
//!
//! A reply marks read what it answers: Done, held here, and the provider's read marker moved
//! through the message where the provider lets this server move it — forward only, and once. The
//! page decides which message its own sends mark and asks for the provider's half with
//! `after_reply: true`; a voice agent's reply has no page in the loop, so the server marks both
//! halves for it once the post is confirmed. The in-memory chat service records every mark asked of
//! it, so "asked once" and "never asked for an older message" are counted, not inferred.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatError, UpstreamReadScope};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::http::router;
use vibe_talk::model::ChannelId;
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
    discord: Arc<FakeDiscord>,
    store: Arc<FakeStore>,
}

/// A server whose provider lets it move the read marker, with `count` messages in the writable
/// channel, oldest first.
fn harness(count: usize) -> (Harness, Vec<String>) {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_read_marks();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let ids = (0..count)
        .map(|n| {
            discord
                .seed(&channel, "codex-eng", &format!("update {n}"))
                .0
        })
        .collect();
    (
        Harness {
            router: router(state),
            discord,
            store,
        },
        ids,
    )
}

async fn send(
    harness: &Harness,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("accept", "application/json, text/event-stream")
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = harness
        .router
        .clone()
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

/// The page's ask after a reply it sent answered `id` in `channel`.
async fn reply_mark_in(harness: &Harness, channel: &str, id: &str) -> (StatusCode, Value) {
    send(
        harness,
        "POST",
        &format!("/api/v1/channels/{channel}/upstream-read"),
        Some(json!({ "message_id": id, "after_reply": true })),
    )
    .await
}

async fn reply_mark(harness: &Harness, id: &str) -> (StatusCode, Value) {
    reply_mark_in(harness, WRITE_CHANNEL, id).await
}

/// The owner's own **Mark read in**.
async fn owners_mark(harness: &Harness, id: &str) -> (StatusCode, Value) {
    send(
        harness,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/upstream-read"),
        Some(json!({ "message_id": id })),
    )
    .await
}

/// Every mark the provider was asked for, in order, as `channel:id`.
fn asked(harness: &Harness) -> Vec<String> {
    harness
        .discord
        .upstream_read_marks()
        .into_iter()
        .map(|(channel, id)| format!("{}:{}", channel.0, id.0))
        .collect()
}

fn here(id: &str) -> String {
    format!("{WRITE_CHANNEL}:{id}")
}

/// The provider marks asked for, once at least `count` have been: a voice reply's mark runs after
/// the tool's answer.
async fn asked_at_least(harness: &Harness, count: usize) -> Vec<String> {
    for _ in 0..400 {
        let now = asked(harness);
        if now.len() >= count {
            return now;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    asked(harness)
}

/// Long enough for a voice reply's mark, which needs no I/O here, to have asked if it was going to.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(150)).await;
}

/// What a reply or the reader marked Done: the store's marks, less the owner's own messages, which
/// start Done when this server first sees them (`#549 own-read-stored`) — a voice reply this server
/// posted among them.
async fn done_here(harness: &Harness) -> Vec<String> {
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let marks = harness
        .store
        .dismissals(&channel)
        .await
        .expect("the store reads");
    let own = harness
        .store
        .own_decided(&channel, &marks)
        .await
        .expect("the store reads");
    marks
        .into_iter()
        .filter(|id| !own.contains(id))
        .map(|id| id.0)
        .collect()
}

#[tokio::test]
async fn a_reply_mark_asks_the_provider_once_per_message_and_never_for_an_older_one() {
    let (harness, ids) = harness(9);
    let (status, body) = reply_mark(&harness, &ids[4]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(body["through"], ids[4].as_str());
    assert_eq!(asked(&harness), vec![here(&ids[4])]);

    // The same message again: a second reply to it is not a second mark.
    let (status, body) = reply_mark(&harness, &ids[4]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "already_marked", "{body}");
    // An older message: the marker is past it, and moving it there would move it back.
    let (status, body) = reply_mark(&harness, &ids[2]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "behind", "{body}");
    assert_eq!(
        asked(&harness),
        vec![here(&ids[4])],
        "a reply-mark asked for nothing new"
    );

    // A newer one moves it on.
    let (_, body) = reply_mark(&harness, &ids[6]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[4]), here(&ids[6])]);

    // THE CONTROL: the owner's own mark of the older message is asked, as it always was — it is the
    // owner's word, and the provider's own check keeps it forward — and says no outcome.
    let (status, body) = owners_mark(&harness, &ids[2]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.get("outcome").is_none(), "{body}");
    assert_eq!(
        asked(&harness),
        vec![here(&ids[4]), here(&ids[6]), here(&ids[2])],
        "the floor stopped the owner's own mark, so this test cannot tell it from a stopped reply-mark"
    );

    // Neither marks anything Done: the page marks Done itself, by the Done button's own act.
    assert_eq!(done_here(&harness).await, Vec::<String>::new());
}

#[tokio::test]
async fn the_owners_own_mark_raises_the_floor_that_reply_marks_keep_to() {
    let (harness, ids) = harness(9);
    let (status, body) = owners_mark(&harness, &ids[7]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (_, body) = reply_mark(&harness, &ids[5]).await;
    assert_eq!(body["outcome"], "behind", "{body}");
    let (_, body) = reply_mark(&harness, &ids[7]).await;
    assert_eq!(body["outcome"], "already_marked", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[7])]);
    let (_, body) = reply_mark(&harness, &ids[8]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[7]), here(&ids[8])]);
}

#[tokio::test]
async fn a_failed_reply_mark_raises_no_floor_and_the_next_reply_asks_again() {
    let (harness, ids) = harness(4);
    harness
        .discord
        .fail_upstream_read_mark_next(ChatError::Transport("bridge timed out".to_owned()));
    let (status, body) = reply_mark(&harness, &ids[2]).await;
    assert!(
        status.is_server_error(),
        "a failed mark answered {status}: {body}"
    );
    assert!(body.get("outcome").is_none(), "{body}");
    // Nothing was marked, so nothing behind it is taken as marked either.
    let (_, body) = reply_mark(&harness, &ids[1]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    let (_, body) = reply_mark(&harness, &ids[2]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(
        asked(&harness),
        vec![here(&ids[2]), here(&ids[1]), here(&ids[2])]
    );
}

#[tokio::test]
async fn two_reply_marks_of_one_message_at_once_ask_the_provider_once() {
    let (harness, ids) = harness(3);
    let harness = Arc::new(harness);
    let gate = harness.discord.hold_upstream_read_marks();
    let target = ids[1].clone();
    let first = tokio::spawn({
        let (harness, target) = (Arc::clone(&harness), target.clone());
        async move { reply_mark(&harness, &target).await }
    });
    // The first is at the provider, held there, before the second arrives.
    assert_eq!(asked_at_least(&harness, 1).await, vec![here(&target)]);
    let second = tokio::spawn({
        let (harness, target) = (Arc::clone(&harness), target.clone());
        async move { reply_mark(&harness, &target).await }
    });
    settle().await;
    assert_eq!(
        asked(&harness),
        vec![here(&target)],
        "the second asked while the first was held"
    );
    gate.add_permits(2);
    let (one, two) = (first.await.expect("first"), second.await.expect("second"));
    assert_eq!(
        (one.0, two.0),
        (StatusCode::OK, StatusCode::OK),
        "{one:?} {two:?}"
    );
    assert_eq!(one.1["outcome"], "marked", "{}", one.1);
    assert_eq!(two.1["outcome"], "already_marked", "{}", two.1);
    assert_eq!(asked(&harness), vec![here(&target)]);
}

#[tokio::test]
async fn a_mark_held_at_the_provider_in_one_channel_does_not_hold_up_another_channel() {
    let (harness, ids) = harness(2);
    let other = harness.discord.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "codex-eng",
        "elsewhere",
    );
    let harness = Arc::new(harness);
    let gate = harness.discord.hold_upstream_read_marks();
    let first = tokio::spawn({
        let (harness, target) = (Arc::clone(&harness), ids[1].clone());
        async move { reply_mark(&harness, &target).await }
    });
    assert_eq!(asked_at_least(&harness, 1).await, vec![here(&ids[1])]);
    let second = tokio::spawn({
        let (harness, target) = (Arc::clone(&harness), other.0.clone());
        async move { reply_mark_in(&harness, READ_CHANNEL, &target).await }
    });
    // The other channel's mark reaches the provider while the first is still held there.
    assert_eq!(
        asked_at_least(&harness, 2).await,
        vec![here(&ids[1]), format!("{READ_CHANNEL}:{}", other.0)],
        "a mark in one channel waited on a mark held in another"
    );
    gate.add_permits(2);
    let (one, two) = (first.await.expect("first"), second.await.expect("second"));
    assert_eq!(one.1["outcome"], "marked", "{}", one.1);
    assert_eq!(two.1["outcome"], "marked", "{}", two.1);
}

#[tokio::test]
async fn floors_are_per_channel() {
    let (harness, ids) = harness(3);
    let other = harness.discord.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "codex-eng",
        "older, elsewhere",
    );
    let (_, body) = reply_mark(&harness, &ids[2]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    // An older id in ANOTHER channel is that channel's first mark, not one behind this channel's.
    let (_, body) = reply_mark_in(&harness, READ_CHANNEL, &ids[0]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    let (_, body) = reply_mark_in(&harness, READ_CHANNEL, &other.0).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(
        asked(&harness),
        vec![
            here(&ids[2]),
            format!("{READ_CHANNEL}:{}", ids[0]),
            format!("{READ_CHANNEL}:{}", other.0),
        ]
    );
}

#[tokio::test]
async fn a_reply_mark_where_the_provider_cannot_mark_is_refused_and_asks_nothing() {
    let (state, discord, _store) = vibe_talk::testing::state_with_store();
    let id = discord.seed(&ChannelId(WRITE_CHANNEL.to_owned()), "codex-eng", "hello");
    let harness = Harness {
        router: router(state),
        discord,
        store: Arc::new(FakeStore::new()),
    };
    let (status, body) = reply_mark(&harness, &id.0).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "refused", "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("does not support upstream read marks")),
        "{body}"
    );
    assert!(harness.discord.upstream_read_marks().is_empty());
}

// --- never wider than the channel ------------------------------------------------------------

/// The bridge's refusal of a channel whose marker would clear messages vibe-talk never received.
fn thread_channel_refusal() -> ChatError {
    ChatError::Status {
        status: 409,
        body: r#"{"code":"thread_channel"}"#.to_owned(),
    }
}

/// Every channel the provider was asked what its marker covers in, in order.
fn scope_asks(harness: &Harness) -> Vec<String> {
    harness
        .discord
        .upstream_read_scope_asks()
        .into_iter()
        .map(|channel| channel.0)
        .collect()
}

#[tokio::test]
async fn a_reply_in_a_channel_narrowed_to_one_thread_asks_the_provider_nothing() {
    let (harness, ids) = harness(4);
    harness.discord.set_upstream_read_scope(
        &ChannelId(WRITE_CHANNEL.to_owned()),
        UpstreamReadScope::Wider,
    );
    let (status, body) = reply_mark(&harness, &ids[2]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "thread_channel", "{body}");
    assert!(
        body["upstream_read_notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("was not asked")),
        "the answer says the cursor was moved: {body}"
    );
    // A newer message, and the first again: still nothing, and what the marker covers is not asked
    // twice.
    let (_, body) = reply_mark(&harness, &ids[3]).await;
    assert_eq!(body["outcome"], "thread_channel", "{body}");
    assert_eq!(
        asked(&harness),
        Vec::<String>::new(),
        "a reply in one thread of a larger space moved the whole space's marker"
    );
    assert_eq!(scope_asks(&harness), vec![WRITE_CHANNEL.to_owned()]);

    // THE CONTROL: a whole channel beside it, on the same provider, is marked.
    let other = harness.discord.seed(
        &ChannelId(READ_CHANNEL.to_owned()),
        "codex-eng",
        "a whole channel",
    );
    let (_, body) = reply_mark_in(&harness, READ_CHANNEL, &other.0).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(asked(&harness), vec![format!("{READ_CHANNEL}:{}", other.0)]);

    // The owner's own Mark read in is his choice, labelled with the whole space, and still asks.
    let (status, body) = owners_mark(&harness, &ids[1]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        asked(&harness),
        vec![format!("{READ_CHANNEL}:{}", other.0), here(&ids[1])]
    );
}

#[tokio::test]
async fn the_read_that_tells_a_markers_scope_is_counted_as_the_reply_marks() {
    // Attribution (`remote-ops-audit`): the read is the server's own, made for a reply's mark, and
    // the upstream profile says so rather than counting it untagged.
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    discord.enable_upstream_read_marks();
    let id = discord.seed(&ChannelId(WRITE_CHANNEL.to_owned()), "codex-eng", "hello");
    let harness = Harness {
        router: router(state.clone()),
        discord,
        store,
    };
    let (_, body) = reply_mark(&harness, &id.0).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    let counted: Vec<(&str, &str, u64)> = state
        .providers
        .profile()
        .report()
        .providers
        .iter()
        .flat_map(|provider| {
            provider
                .reasons
                .iter()
                .map(|row| (row.name, row.reason.as_str(), row.count))
        })
        .filter(|(name, _, _)| *name == "upstream_read_scope")
        .collect();
    assert_eq!(counted, vec![("upstream_read_scope", "reply-mark", 1)]);
}

#[tokio::test]
async fn a_reply_where_the_provider_cannot_say_what_its_marker_covers_asks_nothing() {
    let (harness, ids) = harness(3);
    harness.discord.set_upstream_read_scope(
        &ChannelId(WRITE_CHANNEL.to_owned()),
        UpstreamReadScope::Unknown,
    );
    let (status, body) = reply_mark(&harness, &ids[1]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "scope_unknown", "{body}");
    assert!(asked(&harness).is_empty(), "a blind reply-mark was asked");
    // Not an answer to keep: the next reply asks again, and once the provider says, it is marked.
    harness.discord.set_upstream_read_scope(
        &ChannelId(WRITE_CHANNEL.to_owned()),
        UpstreamReadScope::Channel,
    );
    let (_, body) = reply_mark(&harness, &ids[2]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[2])]);
    assert_eq!(
        scope_asks(&harness),
        vec![WRITE_CHANNEL.to_owned(), WRITE_CHANNEL.to_owned()]
    );
}

#[tokio::test]
async fn a_failed_scope_read_marks_nothing_and_the_next_reply_asks_again() {
    let (harness, ids) = harness(3);
    harness
        .discord
        .fail_upstream_read_scope_next(ChatError::Transport("bridge timed out".to_owned()));
    let (status, body) = reply_mark(&harness, &ids[1]).await;
    assert!(
        status.is_server_error(),
        "a failed scope read answered {status}: {body}"
    );
    assert!(asked(&harness).is_empty(), "a mark was asked blind");
    let (_, body) = reply_mark(&harness, &ids[1]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[1])]);
    assert_eq!(scope_asks(&harness).len(), 2);
}

#[tokio::test]
async fn a_thread_channel_refusal_is_remembered_whoever_asked_the_mark() {
    // The owner's own mark, refused: no reply asks the provider after it, nor asks its scope.
    let (harness, ids) = harness(4);
    harness
        .discord
        .fail_upstream_read_mark_next(thread_channel_refusal());
    let (status, body) = owners_mark(&harness, &ids[1]).await;
    assert!(
        !status.is_success(),
        "a refused mark answered {status}: {body}"
    );
    let (_, body) = reply_mark(&harness, &ids[3]).await;
    assert_eq!(body["outcome"], "thread_channel", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[1])]);
    assert!(
        scope_asks(&harness).is_empty(),
        "{:?}",
        scope_asks(&harness)
    );

    // A reply's own mark, refused although the scope read said the marker is the channel's: the
    // provider's refusal wins, and the next reply asks nothing.
    let (harness, ids) = crate::harness(4);
    harness
        .discord
        .fail_upstream_read_mark_next(thread_channel_refusal());
    let (status, body) = reply_mark(&harness, &ids[1]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["outcome"], "thread_channel", "{body}");
    let (_, body) = reply_mark(&harness, &ids[3]).await;
    assert_eq!(body["outcome"], "thread_channel", "{body}");
    assert_eq!(asked(&harness), vec![here(&ids[1])]);

    // THE CONTROL: any other failure is not remembered, and the next reply asks.
    let (harness, ids) = crate::harness(4);
    harness
        .discord
        .fail_upstream_read_mark_next(ChatError::Status {
            status: 409,
            body: r#"{"code":"conflict"}"#.to_owned(),
        });
    let (status, _) = owners_mark(&harness, &ids[1]).await;
    assert!(!status.is_success(), "{status}");
    let (_, body) = reply_mark(&harness, &ids[3]).await;
    assert_eq!(body["outcome"], "marked", "{body}");
}

// --- a voice agent's reply -------------------------------------------------------------------

async fn post_reply(harness: &Harness, arguments: Value) -> String {
    let (status, response) = send(
        harness,
        "POST",
        "/mcp",
        Some(json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "post_reply", "arguments": arguments },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{response}");
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

async fn ask_before_send(harness: &Harness, ask: bool) {
    let (status, body) = send(
        harness,
        "PUT",
        "/api/v1/voice-settings",
        Some(json!({ "ask_before_send": ask })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn a_voice_reply_sent_at_once_marks_what_it_answers_done_and_read_and_nothing_else() {
    let (harness, ids) = harness(4);
    ask_before_send(&harness, false).await;

    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it", "reply_to": ids[1] }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    // Done is written before the answer; the provider's half runs after it.
    assert_eq!(done_here(&harness).await, vec![ids[1].clone()]);
    assert_eq!(asked_at_least(&harness, 1).await, vec![here(&ids[1])]);

    // A post to the channel itself answers nothing, and marks nothing.
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "status?" }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    // A second reply to the same message: Done already, and the marker already through it.
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "and done", "reply_to": ids[1] }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    // A reply to an older message: Done, and the marker is left where it is.
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "that too", "reply_to": ids[0] }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    settle().await;
    let mut done = done_here(&harness).await;
    done.sort();
    assert_eq!(done, vec![ids[0].clone(), ids[1].clone()]);
    assert_eq!(
        asked(&harness),
        vec![here(&ids[1])],
        "a voice reply moved the marker back or twice"
    );

    // THE CONTROL: a reply the chat service refused answered nothing yet, and marks nothing.
    harness
        .discord
        .refuse_post_after(0, 403, "Missing Permissions");
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "this one fails", "reply_to": ids[3] }),
    )
    .await;
    assert!(said.starts_with("NOT SENT"), "{said}");
    settle().await;
    assert!(
        !done_here(&harness).await.contains(&ids[3]),
        "a reply that was not sent marked Done"
    );
    assert_eq!(
        asked(&harness),
        vec![here(&ids[1])],
        "a reply that was not sent marked read upstream"
    );
}

#[tokio::test]
async fn a_proposed_voice_reply_marks_nothing_until_it_is_confirmed_and_posted() {
    let (harness, ids) = harness(3);
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "looks good", "reply_to": ids[2] }),
    )
    .await;
    assert!(said.starts_with("NOT SENT."), "{said}");
    settle().await;
    // THE CONTROL: proposing is not sending.
    assert_eq!(
        done_here(&harness).await,
        Vec::<String>::new(),
        "a proposal marked Done"
    );
    assert!(
        asked(&harness).is_empty(),
        "a proposal marked read upstream"
    );

    let (status, pending) = send(&harness, "GET", "/api/v1/post-proposals", None).await;
    assert_eq!(status, StatusCode::OK, "{pending}");
    let proposal = &pending["proposal"];
    let (status, body) = send(
        &harness,
        "POST",
        "/api/v1/post-proposals/commit",
        Some(json!({
            "handle": proposal["handle"],
            "channel_id": proposal["channel_id"],
            "text": proposal["text"],
            "reply_to": proposal["reply_to"],
            "confirmed_by": "ui",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(harness.discord.posted().len(), 1);
    assert_eq!(done_here(&harness).await, vec![ids[2].clone()]);
    assert_eq!(asked_at_least(&harness, 1).await, vec![here(&ids[2])]);
}

#[tokio::test]
async fn a_voice_reply_where_the_provider_cannot_mark_is_still_done() {
    let (state, discord, store) = vibe_talk::testing::state_with_store();
    let id = discord.seed(&ChannelId(WRITE_CHANNEL.to_owned()), "codex-eng", "hello");
    let harness = Harness {
        router: router(state),
        discord,
        store,
    };
    ask_before_send(&harness, false).await;
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "hi", "reply_to": id.0 }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    settle().await;
    assert_eq!(done_here(&harness).await, vec![id.0.clone()]);
    assert!(harness.discord.upstream_read_marks().is_empty());
}

#[tokio::test]
async fn a_voice_reply_in_a_channel_narrowed_to_one_thread_is_done_and_asks_the_provider_nothing() {
    let (harness, ids) = harness(3);
    harness.discord.set_upstream_read_scope(
        &ChannelId(WRITE_CHANNEL.to_owned()),
        UpstreamReadScope::Wider,
    );
    ask_before_send(&harness, false).await;
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it", "reply_to": ids[1] }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    // The Done is this server's own, and stands.
    assert_eq!(done_here(&harness).await, vec![ids[1].clone()]);
    // The provider was asked what its marker covers, and then nothing more.
    for _ in 0..400 {
        if !scope_asks(&harness).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    settle().await;
    assert_eq!(scope_asks(&harness), vec![WRITE_CHANNEL.to_owned()]);
    assert_eq!(
        asked(&harness),
        Vec::<String>::new(),
        "a voice agent's reply in one thread of a larger space moved the whole space's marker"
    );

    // THE CONTROL: the same reply in a whole channel moves it.
    let (harness, ids) = crate::harness(3);
    ask_before_send(&harness, false).await;
    let said = post_reply(
        &harness,
        json!({ "channel_id": WRITE_CHANNEL, "text": "on it", "reply_to": ids[1] }),
    )
    .await;
    assert!(said.starts_with("SENT."), "{said}");
    assert_eq!(asked_at_least(&harness, 1).await, vec![here(&ids[1])]);
}
