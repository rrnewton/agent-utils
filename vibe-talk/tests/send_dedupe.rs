//! `#448 send-dedupe`: a keyed post is posted once, whatever its provider.
//!
//! The owner sent a long message to a Slack channel. It was cut in three parts and took twelve
//! seconds; the page showed it as failed, the owner pressed Retry, and the message reached the
//! channel twice, then a third time. Every attempt carried the same idempotency key, and Slack has
//! nothing upstream that reads one. Now this server remembers keyed posts itself
//! (`vibe_talk::post_keys`).
//!
//! Every test drives the real HTTP route through the real Slack client against a loopback fake of
//! the Slack Web API, so "posted once" is counted where Slack would count it.

mod slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, WRITE_TOKEN};

const TEAM: &str = "C0000MAIN1";
const KEY: &str = "outgoing-key_448";

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

/// A Slack deployment against the fake, taking `max_post_chars` in one message.
async fn slack(fake: &Fake, max_post_chars: usize) -> AppState {
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
max_post_chars = {max_post_chars}

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

fn fixture() -> FakeState {
    let mut state = FakeState::default();
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

fn outcome(wait: u64) -> String {
    format!("/api/v1/channels/{TEAM}/reply/outcome?wait={wait}")
}

/// Numbered paragraphs, `bytes` long or a little over: real places to cut, no two parts alike.
fn prose(bytes: usize) -> String {
    let mut text = String::new();
    let mut run = 0;
    while text.len() < bytes {
        run += 1;
        text.push_str(&format!(
            "Paragraph {run} says what the owner wrote about the deploy, in plain words. "
        ));
        if run % 4 == 0 {
            text.push_str("\n\n");
        }
    }
    text
}

fn ids(answer: &Value) -> Vec<String> {
    answer["parts"]
        .as_array()
        .expect("parts")
        .iter()
        .map(|part| part["id"].as_str().expect("id").to_owned())
        .collect()
}

#[tokio::test]
async fn a_repeated_key_is_answered_with_the_first_post_and_posts_nothing() {
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let text = prose(1_500);
    let body = json!({ "text": text, "idempotency_key": KEY });
    let (status, first) = call(&state, &reply(), body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let posted = fake.requests("chat.postMessage").len();
    assert!(posted >= 3, "the fixture did not cut the post in three");

    let (status, again) = call(&state, &reply(), body).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(
        ids(&again),
        ids(&first),
        "the repeat was not the first post"
    );
    assert_eq!(
        fake.requests("chat.postMessage").len(),
        posted,
        "the repeat posted again"
    );

    // A new message with the same words is a new key, and posts.
    let (status, new) = call(
        &state,
        &reply(),
        json!({ "text": text, "idempotency_key": "outgoing-key_449" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{new}");
    assert_eq!(fake.requests("chat.postMessage").len(), posted * 2);
    assert_ne!(ids(&new), ids(&first));

    // So does the same words with no key at all, as before.
    let (status, _) = call(&state, &reply(), json!({ "text": "short" })).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = call(&state, &reply(), json!({ "text": "short" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(fake.requests("chat.postMessage").len(), posted * 2 + 2);
}

#[tokio::test]
async fn a_repeat_that_arrives_while_the_first_is_posting_is_coalesced_into_it() {
    let mut fixture = fixture();
    fixture.post_delay = Some(Duration::from_millis(150));
    let fake = Fake::start(fixture, 20).await;
    let state = slack(&fake, 600).await;
    let body = json!({ "text": prose(1_500), "idempotency_key": KEY });
    let uri = reply();
    let first = call(&state, &uri, body.clone());
    let repeat = async {
        // Well inside the first post's three parts.
        tokio::time::sleep(Duration::from_millis(100)).await;
        call(&state, &reply(), body.clone()).await
    };
    let ((status_a, a), (status_b, b)) = tokio::join!(first, repeat);
    assert_eq!(status_a, StatusCode::OK, "{a}");
    assert_eq!(status_b, StatusCode::OK, "{b}");
    assert_eq!(
        ids(&a),
        ids(&b),
        "the repeat was answered with another post"
    );
    assert_eq!(
        fake.requests("chat.postMessage").len(),
        ids(&a).len(),
        "the two requests posted more than one message's parts"
    );
}

#[tokio::test]
async fn a_caller_that_gave_up_and_retried_gets_the_post_it_gave_up_on() {
    // The incident, on the server's side: the page stopped waiting — something in front of this
    // server answered for it, or its connection dropped — while the post went on landing. The
    // retry comes under the same key, and is not a second post.
    let mut fixture = fixture();
    fixture.post_delay = Some(Duration::from_millis(150));
    let fake = Fake::start(fixture, 20).await;
    let state = slack(&fake, 600).await;
    let body = json!({ "text": prose(1_500), "idempotency_key": KEY });
    let gave_up = tokio::time::timeout(
        Duration::from_millis(200),
        call(&state, &reply(), body.clone()),
    )
    .await;
    assert!(
        gave_up.is_err(),
        "the first post answered before the caller gave up"
    );

    // Asked what became of it, posting nothing: still posting, and how far.
    let (status, posting) = call(&state, &outcome(0), body.clone()).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{posting}");
    assert_eq!(posting["state"], "posting");
    assert!(posting["parts"].as_u64().expect("parts") >= 3, "{posting}");
    assert!(posting["posted"].as_u64().expect("posted") < posting["parts"].as_u64().unwrap());

    // Waiting for it gives its answer.
    let (status, settled) = call(&state, &outcome(5), body.clone()).await;
    assert_eq!(status, StatusCode::OK, "{settled}");
    let parts = ids(&settled);

    let (status, retried) = call(&state, &reply(), body).await;
    assert_eq!(status, StatusCode::OK, "{retried}");
    assert_eq!(ids(&retried), parts);
    assert_eq!(
        fake.requests("chat.postMessage").len(),
        parts.len(),
        "the retry posted the message again"
    );
}

#[tokio::test]
async fn a_post_this_server_never_saw_is_unknown_and_the_outcome_posts_nothing() {
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let (status, answer) = call(
        &state,
        &outcome(0),
        json!({ "text": "never sent", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["error"], "post_unknown");
    let (status, answer) = call(&state, &outcome(0), json!({ "text": "no key" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(fake.requests("chat.postMessage").len(), 0);
}

#[tokio::test]
async fn a_post_that_landed_in_part_is_answered_with_its_207_and_its_rest_still_goes() {
    let mut fixture = fixture();
    fixture.fail_posts = vec![2];
    let fake = Fake::start(fixture, 20).await;
    let state = slack(&fake, 600).await;
    let body = json!({ "text": prose(1_500), "idempotency_key": KEY });
    let (status, partial) = call(&state, &reply(), body.clone()).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{partial}");
    assert_eq!(partial["posted"], 1);
    let after_first = fake.requests("chat.postMessage").len();

    // The whole text again, as a page that never saw the 207 sends it: the 207 again, so it learns
    // the rest, and the first part is not posted twice.
    let (status, again) = call(&state, &reply(), body).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{again}");
    assert_eq!(again["unsent"], partial["unsent"]);
    assert_eq!(fake.requests("chat.postMessage").len(), after_first);

    // The rest, under the same key, is other words: it goes.
    let (status, done) = call(
        &state,
        &reply(),
        json!({ "text": partial["unsent"], "thread_id": partial["thread_id"], "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{done}");
    assert!(fake.requests("chat.postMessage").len() > after_first);
}

#[tokio::test]
async fn a_post_that_posted_nothing_is_forgotten_so_retry_still_sends_it() {
    let mut fixture = fixture();
    fixture.fail_posts = vec![1];
    let fake = Fake::start(fixture, 20).await;
    let state = slack(&fake, 600).await;
    let body = json!({ "text": "one short part", "idempotency_key": KEY });
    let (status, failed) = call(&state, &reply(), body.clone()).await;
    assert_eq!(status, StatusCode::MULTI_STATUS, "{failed}");
    assert_eq!(failed["posted"], 0);
    let (status, sent) = call(&state, &reply(), body).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(fake.requests("chat.postMessage").len(), 2);
}

#[tokio::test]
async fn the_later_parts_of_a_long_slack_post_go_in_without_reading_the_thread_first() {
    // Each part after the first used to read the thread its own first part had just started
    // (`conversations.replies`) before it was posted: a round trip each, in series.
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let (status, answer) = call(
        &state,
        &reply(),
        json!({ "text": prose(1_500), "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert!(fake.requests("chat.postMessage").len() >= 3);
    assert_eq!(
        fake.calls("conversations.replies"),
        0,
        "a later part read the thread its first part had just started"
    );

    // A thread this client has not posted in is still read before it is posted into.
    let (status, answer) = call(
        &state,
        &reply(),
        json!({ "text": "into the old one", "thread_id": ts(10, 0) }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(fake.calls("conversations.replies"), 1);
}

#[tokio::test]
async fn a_repeat_whose_placement_changed_between_attempts_is_still_the_one_post() {
    // A page may draw a reply into the thread it starts before the server's copy arrives, and a
    // retry from such a page could say "thread X" where the first attempt said "reply to X". The key
    // names the post, so the repeat is answered with it, and nothing is posted where it now points.
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let root = vibe_talk::slack::message_id_from_ts(&ts(10, 0)).expect("id");
    let (status, first) = call(
        &state,
        &reply(),
        json!({ "text": "yes, it is green", "reply_to": root.as_str(), "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let posted = fake.requests("chat.postMessage").len();
    let (status, again) = call(
        &state,
        &reply(),
        json!({ "text": "yes, it is green", "thread_id": ts(10, 0), "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(ids(&again), ids(&first));
    assert_eq!(
        fake.requests("chat.postMessage").len(),
        posted,
        "the moved repeat was posted again"
    );
    let (status, asked) = call(
        &state,
        &outcome(0),
        json!({ "text": "yes, it is green", "thread_id": ts(10, 0), "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{asked}");
    assert_eq!(ids(&asked), ids(&first));
}

#[tokio::test]
async fn a_key_reused_for_other_words_is_refused_and_posts_nothing() {
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let (status, _) = call(
        &state,
        &reply(),
        json!({ "text": "the first message", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, refused) = call(
        &state,
        &reply(),
        json!({ "text": "something else entirely", "idempotency_key": KEY }),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(refused["error"], "key_reused");
    assert_eq!(fake.requests("chat.postMessage").len(), 1);
}

#[tokio::test]
async fn a_short_post_does_not_excuse_a_later_reply_into_its_thread_from_the_read() {
    // Only the rest of a long post skips the read: a reply into the thread of a message this client
    // posted on its own is checked like any other, so a root removed meanwhile is still refused.
    let fake = Fake::start(fixture(), 20).await;
    let state = slack(&fake, 600).await;
    let (status, posted) = call(&state, &reply(), json!({ "text": "a root" })).await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    let root = vibe_talk::slack::ts_from_message_id(&vibe_talk::model::MessageId(
        posted["posted"]["id"].as_str().expect("id").to_owned(),
    ))
    .expect("a Slack id");
    let _ = call(
        &state,
        &reply(),
        json!({ "text": "under it", "thread_id": root }),
    )
    .await;
    assert_eq!(fake.calls("conversations.replies"), 1);
}
