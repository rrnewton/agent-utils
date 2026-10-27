//! `#322 slack-quote-lines`: a Slack quote ends at its own line, on every way a Slack message
//! arrives — history, Flat, a thread, the live poller — and on the page.
//!
//! In Slack `>` quotes the line it starts and no other, so an answer written straight under a
//! quoted question is drawn under the quote. Read as CommonMark, the unquoted line ran into the
//! quote (a lazy continuation), and the owner's answer was drawn inside it. Slack sends `>` as
//! `&gt;`, as it sends every `>`, which is how the fixtures here write it.
//!
//! Against the loopback Slack fake, with one quoted line then an answer, several quoted lines then
//! an answer, a quote that ends the message, a `>>>` that quotes the rest of it, and a quote and
//! its answer in a thread reply.

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

/// One quoted line, then the owner's answer.
const ONE: &str = "&gt; Should we ship it today?\nYes, after the smoke test.";
/// Several quoted lines, a quoted blank line among them, then an answer of two lines.
const SEVERAL: &str = "&gt; The runner is back.\n&gt; Two shards were retried.\n&gt;\n&gt; Is the lane green?\nGreen now.\nShipping at noon.";
/// A quote that ends the message.
const AT_END: &str = "For the record:\n&gt; nothing ships on Fridays";
/// `>>>` quotes the rest of the message.
const REST: &str = "Forwarding:\n&gt;&gt;&gt; the whole\nof this\n\nis quoted";
/// A thread reply that quotes its root and answers it.
const REPLY: &str = "&gt; which branch?\nrelease-12, as before";

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// What each fixture becomes in `content`.
fn one() -> &'static str {
    "> Should we ship it today?\n\nYes, after the smoke test."
}
fn several() -> &'static str {
    "> The runner is back.\n> Two shards were retried.\n>\n> Is the lane green?\n\nGreen now.\nShipping at noon."
}
fn at_end() -> &'static str {
    "For the record:\n> nothing ships on Fridays"
}
fn rest() -> &'static str {
    "Forwarding:\n> the whole\n> of this\n>\n> is quoted"
}
fn reply() -> &'static str {
    "> which branch?\n\nrelease-12, as before"
}

/// Five posts ten seconds apart, the first rooting a thread whose one reply quotes it.
fn fixture() -> (FakeState, String) {
    let mut state = FakeState::default();
    let root = ts(10, 0);
    let mut list = vec![
        profiled(&root, "UADA00001", "Ada", ONE),
        profiled(&ts(20, 0), "UOWNER001", "Owner", SEVERAL),
        profiled(&ts(30, 0), "UADA00001", "Ada", AT_END),
        profiled(&ts(40, 0), "UGRACE001", "Grace", REST),
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
async fn every_read_ends_a_quote_at_its_own_line() {
    let (state, root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();

    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert_eq!(contents(&page), [one(), several(), at_end(), rest()]);

    let flat = client
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert_eq!(
        contents(&flat.messages),
        [one(), reply(), several(), at_end(), rest()]
    );

    let thread = client
        .fetch_timeline(&channel(), &view(TimelineView::Thread, Some(root.as_str())))
        .await
        .expect("the thread");
    assert_eq!(contents(&thread.messages), [one(), reply()]);

    let recent = client
        .fetch_recent(&channel(), 50)
        .await
        .expect("the newest messages");
    assert!(contents(&recent).contains(&reply()), "{recent:?}");
}

#[tokio::test]
async fn the_live_poller_ends_a_quote_at_its_own_line() {
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
            "&gt; did it land?\nIt did.",
        ));
        add_reply(
            state,
            CH,
            &root,
            &ts(51, 0),
            "&gt; and the reply?\nThat too.",
        );
    });
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("a poll");
    let mut published = Vec::new();
    while let Ok(live) = receiver.try_recv() {
        published.push(live.message.content);
    }
    published.sort();
    assert_eq!(
        published,
        ["> and the reply?\n\nThat too.", "> did it land?\n\nIt did."]
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

/// The rendered body of the message whose `content` is `content`.
fn html_of<'v>(messages: &'v Value, content: &str) -> &'v str {
    messages
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"] == content)
        .unwrap_or_else(|| panic!("no message says {content:?} in {messages}"))["content_html"]
        .as_str()
        .unwrap_or_else(|| panic!("{content:?} was not rendered"))
}

/// The text inside the quotes of `html`, and the text after the last one.
fn quoted_and_after(html: &str) -> (String, &str) {
    let mut quoted = String::new();
    let mut rest = html;
    while let Some(start) = rest.find("<blockquote>") {
        let end = rest[start..].find("</blockquote>").expect("a closed quote") + start;
        quoted.push_str(&rest[start..end]);
        rest = &rest[end + "</blockquote>".len()..];
    }
    (quoted, rest)
}

#[tokio::test]
async fn the_page_draws_the_answer_under_the_quote_not_in_it() {
    let (state, _root) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let app = deployment(&fake);
    let flat = get(&app, &format!("/api/v1/channels/{CH}/timeline?view=flat")).await;
    let messages = &flat["messages"];

    for (content, question, answer) in [
        (
            one(),
            "Should we ship it today?",
            "Yes, after the smoke test.",
        ),
        (several(), "Is the lane green?", "Shipping at noon."),
        (reply(), "which branch?", "release-12, as before"),
    ] {
        let html = html_of(messages, content);
        assert_eq!(html.matches("<blockquote>").count(), 1, "{html}");
        let (quoted, after) = quoted_and_after(html);
        assert!(quoted.contains(question), "{html}");
        assert!(
            !quoted.contains(answer) && after.contains(answer),
            "the answer was drawn inside the quote: {html}"
        );
    }
    // The several quoted lines are one quote, the quoted blank line inside it.
    let (quoted, _) = quoted_and_after(html_of(messages, several()));
    assert!(
        quoted.contains("The runner is back.") && quoted.contains("Two shards were retried."),
        "{quoted}"
    );

    // A quote that ends the message is the last thing drawn.
    let html = html_of(messages, at_end());
    let (quoted, after) = quoted_and_after(html);
    assert!(quoted.contains("nothing ships on Fridays"), "{html}");
    assert!(after.trim().is_empty(), "{html}");
    assert!(html.starts_with("<p>For the record:</p>"), "{html}");

    // `>>>`: one quote, to the end of the message, its blank line included.
    let html = html_of(messages, rest());
    assert_eq!(html.matches("<blockquote>").count(), 1, "{html}");
    let (quoted, after) = quoted_and_after(html);
    assert!(
        quoted.contains("the whole") && quoted.contains("is quoted"),
        "{html}"
    );
    assert!(after.trim().is_empty(), "{html}");
}
