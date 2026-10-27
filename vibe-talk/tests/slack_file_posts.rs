//! `#313 slack-file-posts`: a Slack post that is only a file reaches the page as a line naming the
//! file, not as an empty row, on every way a Slack message arrives — history, Flat, a thread, and
//! the live poller — and nothing ever fetches the file.
//!
//! Against the loopback Slack fake, with an image-only post, a text-plus-file post, a file shared
//! into a thread, and a file from a bridge that sends only an id and a name. Every file object
//! points its `url_private` and thumbnail at the fake itself, so a fetch of one would be a request
//! the fake records.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{
    add_reply_message, file, file_post, profiled, thread, ts, Fake, FakeState, METHODS, TOKEN,
};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ThreadAsk};
use vibe_talk::live::{poll_once, LiveHub, LiveMessage};
use vibe_talk::model::{ChannelId, Message};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::{TimelineRequest, TimelineView};

const CH: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;

const IMAGE_LINK: &str = "https://example.slack.com/files/UADA00001/F0IMAGE01/screenshot.png";
const LOG_LINK: &str = "https://example.slack.com/files/UADA00001/F0BUILD01/build.log";
const SHARED_LINK: &str = "https://example.slack.com/files/UADA00001/F0SHARE01/graph.png";

/// What the image-only post's body is.
fn image_line() -> String {
    format!("📎 [screenshot.png](<{IMAGE_LINK}>) · PNG")
}

/// What the text-plus-file post's body is.
fn worded_line() -> String {
    format!("the build log, as promised\n\n📎 [build.log](<{LOG_LINK}>) · Plain Text")
}

/// What the file shared into the thread is.
fn shared_line() -> String {
    format!("📎 [graph.png](<{SHARED_LINK}>) · PNG")
}

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// The roots' `ts`: the image-only post, which roots a thread of one text reply, and the text post
/// a file was shared into.
struct Roots {
    image: String,
    deploy: String,
}

/// Six posts ten seconds apart on a fake started from this state; the file objects point at
/// `file_host`, which a test sets to the fake's own base once it runs.
fn fixture(file_host: &str) -> (FakeState, Roots) {
    let mut state = FakeState::default();
    let image = ts(20, 0);
    let deploy = ts(40, 0);
    let mut list = vec![
        profiled(&ts(10, 0), "UADA00001", "Ada", "good morning"),
        file_post(
            &image,
            "UADA00001",
            "Ada",
            "",
            vec![file("F0IMAGE01", "screenshot.png", "PNG", file_host)],
        ),
        // An older client's upload: the `file_share` subtype, with words beside the file.
        {
            let mut post = file_post(
                &ts(30, 0),
                "UGRACE001",
                "Grace",
                "the build log, as promised",
                vec![file("F0BUILD01", "build.log", "Plain Text", file_host)],
            );
            post["subtype"] = json!("file_share");
            post.as_object_mut().expect("an object").remove("upload");
            post
        },
        profiled(&deploy, "UADA00001", "Ada", "deploy started"),
        // A compatible bridge that sends only a file's id and name.
        json!({"type": "message", "ts": ts(50, 0), "user": "UGRACE001", "text": "",
            "files": [{"id": "F0BRIDGE1", "name": "diagram.svg"}],
            "user_profile": {"display_name": "Grace"}}),
    ];
    thread(
        &mut state,
        CH,
        &mut list[1],
        &[(ts(21, 0), "nice".to_owned())],
    );
    thread(
        &mut state,
        CH,
        &mut list[3],
        &[(ts(41, 0), "on it".to_owned())],
    );
    state.history.insert(CH.to_owned(), list);
    add_reply_message(
        &mut state,
        CH,
        &deploy,
        file_post(
            &ts(42, 0),
            "UGRACE001",
            "Grace",
            "",
            vec![file("F0SHARE01", "graph.png", "PNG", file_host)],
        ),
    );
    (state, Roots { image, deploy })
}

/// A fake serving [`fixture`], its files pointed at itself.
async fn start() -> (Fake, Roots) {
    let (state, roots) = fixture("http://unset.invalid/");
    let fake = Fake::start(state, PAGE_CAP).await;
    let host = fake.base.clone();
    fake.with(|state| {
        for message in state
            .history
            .values_mut()
            .flatten()
            .chain(state.replies.values_mut().flatten())
        {
            if let Some(files) = message.get_mut("files").and_then(Value::as_array_mut) {
                for one in files {
                    for key in ["url_private", "url_private_download", "thumb_360"] {
                        if let Some(url) = one.get(key).and_then(Value::as_str) {
                            let moved = url.replace("http://unset.invalid/", &host);
                            one[key] = json!(moved);
                        }
                    }
                }
            }
        }
    });
    (fake, roots)
}

/// Nothing outside the Web API subset was called: no file, no thumbnail, no `files.info`.
fn assert_no_file_was_fetched(fake: &Fake) {
    fake.with(|state| {
        for (method, _) in &state.requests {
            assert!(
                METHODS.contains(&method.as_str()),
                "the client asked for {method}: a file's content is never fetched"
            );
        }
    });
}

fn contents(messages: &[Message]) -> Vec<&str> {
    messages
        .iter()
        .map(|message| message.content.as_str())
        .collect()
}

fn request(view: TimelineView, thread_id: Option<&str>) -> TimelineRequest {
    TimelineRequest {
        view,
        thread_id: thread_id.map(str::to_owned),
        before: None,
        after: None,
        limit: 50,
    }
}

#[tokio::test]
async fn history_draws_a_line_for_each_file_and_keeps_words_beside_them() {
    let (fake, _) = start().await;
    let client = fake.client();
    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert_eq!(
        contents(&page),
        [
            "good morning".to_owned(),
            image_line(),
            worded_line(),
            "deploy started".to_owned(),
            "📎 diagram.svg".to_owned(),
        ]
    );
    assert_eq!(page[1].author, "Ada", "a file post keeps its author");

    // The main timeline reads the same history.
    let main = client
        .fetch_timeline(&channel(), &request(TimelineView::Main, None))
        .await
        .expect("main");
    assert_eq!(contents(&main.messages), contents(&page));
    assert_no_file_was_fetched(&fake);
}

#[tokio::test]
async fn flat_and_the_thread_views_carry_a_file_shared_into_a_thread() {
    let (fake, roots) = start().await;
    let client = fake.client();

    let flat = client
        .fetch_timeline(&channel(), &request(TimelineView::Flat, None))
        .await
        .expect("flat");
    assert_eq!(
        contents(&flat.messages),
        [
            "good morning".to_owned(),
            image_line(),
            "nice".to_owned(),
            worded_line(),
            "deploy started".to_owned(),
            "on it".to_owned(),
            shared_line(),
            "📎 diagram.svg".to_owned(),
        ]
    );
    let shared = flat
        .messages
        .iter()
        .find(|message| message.content == shared_line())
        .expect("the shared file");
    let membership = shared.thread.as_ref().expect("a thread");
    assert_eq!(membership.id, roots.deploy);
    assert!(!membership.is_root);

    let thread = client
        .fetch_timeline(
            &channel(),
            &request(TimelineView::Thread, Some(&roots.deploy)),
        )
        .await
        .expect("thread");
    assert_eq!(
        contents(&thread.messages),
        [
            "deploy started".to_owned(),
            "on it".to_owned(),
            shared_line()
        ]
    );

    // A read of several threads at once, as a catch-up reads them.
    let answers = client
        .fetch_threads(
            &channel(),
            &[
                ThreadAsk::newest(roots.deploy.clone(), 20),
                ThreadAsk::newest(roots.image.clone(), 20),
            ],
        )
        .await;
    for (id, answer) in answers {
        let page = answer.expect("a thread read");
        let expected = if id == roots.deploy {
            vec![
                "deploy started".to_owned(),
                "on it".to_owned(),
                shared_line(),
            ]
        } else {
            vec![image_line(), "nice".to_owned()]
        };
        assert_eq!(contents(&page.messages), expected, "thread {id}");
    }

    // The thread list names a thread rooted on an image by the image, as text: a title is drawn
    // as it is, and the line's Markdown would show the file's address.
    let threads = client
        .fetch_timeline(&channel(), &request(TimelineView::Threads, None))
        .await
        .expect("threads");
    let image = threads
        .threads
        .iter()
        .find(|summary| summary.id == roots.image)
        .expect("the image's thread");
    assert_eq!(image.title, "📎 screenshot.png · PNG");
    assert_eq!(
        image.root.as_ref().map(|root| root.content.as_str()),
        Some(image_line().as_str())
    );
    let deploy = threads
        .threads
        .iter()
        .find(|summary| summary.id == roots.deploy)
        .expect("the deploy thread");
    assert_eq!(deploy.title, "deploy started");
    assert_no_file_was_fetched(&fake);
}

/// One poller over the channel, with its subscription.
struct Poller {
    hub: LiveHub,
    receiver: tokio::sync::broadcast::Receiver<LiveMessage>,
    cursor: Option<u64>,
}

impl Poller {
    fn new() -> Self {
        let hub = LiveHub::new();
        let receiver = hub.subscribe(&channel(), None).receiver;
        Self {
            hub,
            receiver,
            cursor: None,
        }
    }

    async fn poll(&mut self, client: &dyn ChatClient) -> Vec<String> {
        poll_once(client, &self.hub, &channel(), 50, &mut self.cursor)
            .await
            .expect("a poll");
        let mut published = Vec::new();
        while let Ok(live) = self.receiver.try_recv() {
            published.push(live.message.content);
        }
        published
    }
}

#[tokio::test]
async fn a_file_posted_or_shared_into_a_thread_reaches_the_live_stream() {
    let (fake, roots) = start().await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&client).await;

    let host = fake.base.clone();
    fake.with(|state| {
        state
            .history
            .get_mut(CH)
            .expect("the channel")
            .push(file_post(
                &ts(60, 0),
                "UADA00001",
                "Ada",
                "",
                vec![file("F0LIVE001", "photo.jpg", "JPEG", &host)],
            ));
        add_reply_message(
            state,
            CH,
            &roots.deploy,
            file_post(
                &ts(61, 0),
                "UGRACE001",
                "Grace",
                "and the trace",
                vec![file("F0TRACE01", "trace.json", "JSON", &host)],
            ),
        );
    });
    let mut published = poller.poll(&client).await;
    published.sort();
    assert_eq!(
        published,
        [
            "and the trace\n\n📎 [trace.json](<https://example.slack.com/files/UADA00001/F0TRACE01/trace.json>) · JSON",
            "📎 [photo.jpg](<https://example.slack.com/files/UADA00001/F0LIVE001/photo.jpg>) · JPEG",
        ]
    );
    assert_no_file_was_fetched(&fake);
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

/// The message of `messages` whose `content` is `content`.
fn by_content<'v>(messages: &'v Value, content: &str) -> &'v Value {
    messages
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["content"] == content)
        .unwrap_or_else(|| panic!("no message says {content:?} in {messages}"))
}

#[tokio::test]
async fn the_page_draws_each_file_as_a_link_to_its_slack_page() {
    let (fake, _) = start().await;
    let state = deployment(&fake);

    let page = get(&state, &format!("/api/v1/channels/{CH}/page?render=html")).await;
    let image = by_content(&page["messages"], &image_line());
    let html = image["content_html"].as_str().expect("rendered");
    assert!(
        html.contains(&format!("href=\"{IMAGE_LINK}\""))
            && html.contains(">screenshot.png</a> · PNG"),
        "{html}"
    );
    assert!(
        !html.contains("files-pri") && !html.contains("files-tmb"),
        "never the file itself: {html}"
    );
    let worded = by_content(&page["messages"], &worded_line());
    let html = worded["content_html"].as_str().expect("rendered");
    assert!(
        html.starts_with("<p>the build log, as promised</p>")
            && html.contains(&format!("href=\"{LOG_LINK}\""))
            && html.contains(">build.log</a> · Plain Text"),
        "the words and the file both show: {html}"
    );
    // What the bridge gave, a name and no page, is drawn as it is: plain text, so no HTML at all.
    let bridged = by_content(&page["messages"], "📎 diagram.svg");
    assert!(bridged.get("content_html").is_none(), "{bridged}");

    // Flat, as the page reads it, renders the file shared into a thread.
    let flat = get(&state, &format!("/api/v1/channels/{CH}/timeline?view=flat")).await;
    let shared = by_content(&flat["messages"], &shared_line());
    let html = shared["content_html"].as_str().expect("rendered");
    assert!(html.contains(&format!("href=\"{SHARED_LINK}\"")), "{html}");
    assert_no_file_was_fetched(&fake);
}

/// Read the stream until `want` complete events have arrived; an SSE event ends at a blank line.
async fn read_events(body: &mut Body, want: usize) -> String {
    let mut text = String::new();
    while text.matches("\n\n").count() < want {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), body.frame())
            .await
            .unwrap_or_else(|_| panic!("nothing streamed within five seconds; so far: {text:?}"))
            .expect("the stream ended early")
            .expect("a readable frame");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    text
}

#[tokio::test]
async fn the_live_stream_sends_the_page_a_new_file_post_drawn_with_its_link() {
    let (fake, _) = start().await;
    let state = deployment(&fake);
    let client = Arc::clone(&state.providers.entry("slack").expect("slack").client);
    let mut cursor = None;
    poll_once(client.as_ref(), &state.live, &channel(), 50, &mut cursor)
        .await
        .expect("the seeding poll");

    let request = Request::builder()
        .uri(format!("/api/v1/channels/{CH}/stream"))
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();

    let host = fake.base.clone();
    fake.with(|state| {
        state
            .history
            .get_mut(CH)
            .expect("the channel")
            .push(file_post(
                &ts(60, 0),
                "UADA00001",
                "Ada",
                "",
                vec![file("F0LIVE001", "photo.jpg", "JPEG", &host)],
            ));
    });
    let tick = poll_once(client.as_ref(), &state.live, &channel(), 50, &mut cursor)
        .await
        .expect("a poll");
    assert_eq!(tick.published, 1);

    let text = read_events(&mut body, 1).await;
    let payload: Value = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .find_map(|json| serde_json::from_str(json.trim()).ok())
        .expect("one event");
    let message = &payload["message"];
    let link = "https://example.slack.com/files/UADA00001/F0LIVE001/photo.jpg";
    assert_eq!(
        message["content"],
        format!("📎 [photo.jpg](<{link}>) · JPEG")
    );
    let html = message["content_html"]
        .as_str()
        .expect("rendered on the way out");
    assert!(
        html.contains(&format!("href=\"{link}\"")) && html.contains(">photo.jpg</a> · JPEG"),
        "{html}"
    );
    assert_no_file_was_fetched(&fake);
}
