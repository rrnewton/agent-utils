//! `#2279 vibe-talk-post-facto-labels` (owner requests 380, 382): a label added to a message
//! from its menu, after the fact, through the REAL router with the page's exact requests. Only
//! the external upstream (a loopback Slack fake) is stubbed, never the route under test.
//!
//! One label set: a menu-added `#kvm` is the typed `#kvm`'s label in the served message, the
//! `label:` search, the labels browse and its counts; a plain name rides beside; thread replies
//! are labelled by their own ids; every change goes out as a `labels` stream event; nothing is
//! ever sent to the chat provider.

use crate::slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, ManualClock, TempDir, READ_TOKEN, WRITE_TOKEN};

const SLACK: &str = "slack";
const CH: &str = "C0000MAIN1";
/// A failure guard only: every wait below is for a condition, and passes as soon as it holds.
const GUARD: Duration = Duration::from_secs(120);

async fn send(
    state: &AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
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

async fn ok(state: &AppState, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (status, answer) = send(state, method, uri, WRITE_TOKEN, body).await;
    assert_eq!(status, StatusCode::OK, "{method} {uri}: {answer}");
    answer
}

async fn settled(state: &AppState) {
    assert!(state.message_cache.shadow_settled(GUARD).await);
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(GUARD), "the writer caught up");
    }
}

/// The page's exact label write: `PUT`/`DELETE` with `{"name": ...}` (`web/src/view/channel/labelpicker.ts`).
fn label_uri(message: &str) -> String {
    format!("/api/v1/channels/{CH}/message-labels/{message}")
}

fn search_uri(query: &str) -> String {
    let encoded: String = query
        .bytes()
        .map(|b| match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect();
    format!("/api/v1/channels/{CH}/search?q={encoded}&tz=UTC")
}

fn hit_ids(body: &Value) -> Vec<String> {
    body["hits"]
        .as_array()
        .expect("hits")
        .iter()
        .map(|m| m["id"].as_str().expect("id").to_owned())
        .collect()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|list| {
            list.iter()
                .map(|one| one.as_str().expect("a string").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The served message `id` from a timeline answer.
fn served<'a>(body: &'a Value, id: &str) -> &'a Value {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("{id} not served: {body}"))
}

/// Read SSE frames until one carries `needle`, or fail at the guard.
async fn read_until(body: &mut Body, needle: &str) -> String {
    let mut text = String::new();
    while !text.contains(needle) {
        let frame = tokio::time::timeout(GUARD, body.frame())
            .await
            .unwrap_or_else(|_| panic!("no {needle:?} on the stream; so far: {text:?}"))
            .expect("the stream ended")
            .expect("the frame is readable");
        if let Some(data) = frame.data_ref() {
            text.push_str(&String::from_utf8_lossy(data));
        }
    }
    text
}

async fn open_stream(state: &AppState, uri: &str) -> Body {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {READ_TOKEN}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    response.into_body()
}

/// Any Slack Web API method that writes, by the methods the client could call.
fn writes_upstream(method: &str) -> bool {
    [
        "chat.postMessage",
        "chat.update",
        "chat.delete",
        "pins.",
        "reactions.add",
        "reactions.remove",
        "stars.",
        "bookmarks.",
        "conversations.mark",
    ]
    .iter()
    .any(|prefix| method.starts_with(prefix))
}

struct Deployment {
    state: AppState,
    fake: slack_fake::Fake,
    /// The thread's id as a thread read names it (its root's ts).
    thread: String,
    /// Message ids as the server serves them, by text.
    ids: std::collections::HashMap<String, String>,
    _dir: TempDir,
}

impl Deployment {
    fn id(&self, text: &str) -> String {
        self.ids
            .get(text)
            .unwrap_or_else(|| panic!("no message {text:?}: {:?}", self.ids))
            .clone()
    }
}

/// A Slack channel of four messages (one typed `#kvm`, one a thread root with a reply), read
/// into a serving cache through the server's own routes.
async fn deployment(tag: &str) -> Deployment {
    let mut fixture = slack_fake::FakeState::default();
    let texts = [
        "plain words",
        "fixed it #kvm",
        "thread root here",
        "last one",
    ];
    let mut history: Vec<Value> = texts
        .iter()
        .enumerate()
        .map(|(n, text)| {
            slack_fake::profiled(
                &slack_fake::ts(i64::try_from(n).expect("small") * 60, 0),
                "UADA00001",
                "Ada",
                text,
            )
        })
        .collect();
    let root = slack_fake::ts(120, 0);
    let reply = slack_fake::ts(130, 0);
    slack_fake::thread(
        &mut fixture,
        CH,
        &mut history[2],
        &[(reply.clone(), "a reply in the thread".to_owned())],
    );
    fixture.history.insert(CH.to_owned(), history);
    let fake = slack_fake::Fake::start(fixture, 10).await;
    let dir = TempDir::new(tag);
    let clock = ManualClock::at((slack_fake::BASE_SECS + 3_600) * 1_000);
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{SLACK}"
kind = "slack"
api_base = "{base}"
token = "{token}"

[[channels]]
id = "{CH}"
label = "team"
writable = true
"#,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let main = ok(
        &state,
        "GET",
        &format!("/api/v1/channels/{CH}/timeline?view=main&limit=50"),
        None,
    )
    .await;
    let thread = ok(
        &state,
        "GET",
        &format!("/api/v1/channels/{CH}/timeline?view=thread&thread_id={root}&limit=50"),
        None,
    )
    .await;
    let mut ids = std::collections::HashMap::new();
    for body in [&main, &thread] {
        for message in body["messages"].as_array().expect("messages") {
            ids.insert(
                message["content"].as_str().expect("text").to_owned(),
                message["id"].as_str().expect("id").to_owned(),
            );
        }
    }
    let _ = reply;
    settled(&state).await;
    state.message_cache.set_serve(true);
    Deployment {
        state,
        fake,
        thread: root,
        ids,
        _dir: dir,
    }
}

#[tokio::test]
async fn a_menu_label_is_the_typed_label_everywhere_and_comes_off_again() {
    let d = deployment("labels-post-facto").await;
    let state = &d.state;
    let plain_id = d.id("plain words");
    let typed_id = d.id("fixed it #kvm");
    let mut stream = open_stream(state, &format!("/api/v1/channels/{CH}/stream")).await;
    let mut all = open_stream(state, "/api/v1/live/stream").await;
    let requests_before = d.fake.with(|s| s.requests.len());

    // Add `#KVM` to a message whose text has no hashtag: the answer carries the merged state,
    // derived from the server's copy, and the name as normalized.
    let added = ok(
        state,
        "PUT",
        &label_uri(&plain_id),
        Some(json!({"name": "#KVM"})),
    )
    .await;
    assert_eq!(added["name"], "#kvm", "{added}");
    assert_eq!(added["changed"], true);
    assert_eq!(strings(&added["event"]["labels"]), ["kvm"], "{added}");
    assert_eq!(strings(&added["event"]["manual_labels"]), ["#kvm"]);
    assert_eq!(added["event"]["derived"], true);

    // Both streams carry the change as a `labels` event naming the message, with no cursor.
    for body in [&mut stream, &mut all] {
        let text = read_until(body, "event: labels").await;
        let frame = text
            .split("\n\n")
            .find(|frame| frame.contains("event: labels"))
            .expect("the labels frame");
        assert!(frame.contains(&plain_id), "{frame}");
        assert!(
            !frame.contains("\nid:") && !frame.starts_with("id:"),
            "{frame}"
        );
    }

    // A plain label beside it.
    let plain = ok(
        state,
        "PUT",
        &label_uri(&plain_id),
        Some(json!({"name": "  Merge   Queue "})),
    )
    .await;
    assert_eq!(plain["name"], "merge queue");
    assert_eq!(strings(&plain["event"]["plain_labels"]), ["merge queue"]);

    // The served message: one label set, typed and menu labels merged at serve time.
    let timeline = ok(
        state,
        "GET",
        &format!("/api/v1/channels/{CH}/timeline?view=main&limit=50"),
        None,
    )
    .await;
    // Every read says where the menu labels are, so a page that missed the event catches up.
    assert!(
        timeline["labels_revision"].as_i64().is_some_and(|r| r > 0),
        "{timeline}"
    );
    let page = ok(
        state,
        "GET",
        &format!("/api/v1/channels/{CH}/page?limit=50"),
        None,
    )
    .await;
    assert_eq!(
        page["labels_revision"], timeline["labels_revision"],
        "{page}"
    );
    // ...and the channel's complete list names the message, with the labels as served.
    let list = ok(
        state,
        "GET",
        &format!("/api/v1/channels/{CH}/message-labels"),
        None,
    )
    .await;
    assert_eq!(
        list["messages"][0]["message_id"],
        plain_id.as_str(),
        "{list}"
    );
    assert_eq!(
        strings(&list["messages"][0]["plain_labels"]),
        ["merge queue"]
    );
    let message = served(&timeline, &plain_id);
    assert_eq!(strings(&message["labels"]), ["kvm"], "{message}");
    assert_eq!(strings(&message["plain_labels"]), ["merge queue"]);
    assert_eq!(strings(&message["manual_labels"]), ["#kvm", "merge queue"]);
    let typed = served(&timeline, &typed_id);
    assert_eq!(strings(&typed["labels"]), ["kvm"]);
    assert!(typed.get("manual_labels").is_none(), "{typed}");

    // Search: the explicit form finds the typed and the menu-added `#kvm` alike, the bare
    // form too (older links), and the phrase form finds the plain label.
    let both = ok(state, "GET", &search_uri("label:#kvm"), None).await;
    let mut found = hit_ids(&both);
    found.sort();
    assert_eq!(found, [plain_id.clone(), typed_id.clone()], "{both}");
    let bare = ok(state, "GET", &search_uri("label:kvm"), None).await;
    assert_eq!(hit_ids(&bare).len(), 2, "{bare}");
    let phrase = ok(state, "GET", &search_uri("label:\"merge queue\""), None).await;
    assert_eq!(
        hit_ids(&phrase),
        std::slice::from_ref(&plain_id),
        "{phrase}"
    );

    // The browse: `kvm` counted once per message (2), the plain label in its own list.
    let browse = ok(state, "GET", "/api/v1/labels", None).await;
    let kvm = browse["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .find(|entry| entry["name"] == "kvm")
        .unwrap_or_else(|| panic!("kvm listed: {browse}"));
    assert_eq!(kvm["count"], 2, "{browse}");
    assert_eq!(browse["plain"][0]["name"], "merge queue", "{browse}");
    assert_eq!(browse["plain"][0]["count"], 1);

    // Adding the typed message's own `#kvm` from the menu changes nothing a reader sees, and
    // removing it says the text carries it.
    ok(
        state,
        "PUT",
        &label_uri(&typed_id),
        Some(json!({"name": "#kvm"})),
    )
    .await;
    let kept = ok(
        state,
        "DELETE",
        &label_uri(&typed_id),
        Some(json!({"name": "#kvm"})),
    )
    .await;
    assert_eq!(kept["text_derived"], true, "{kept}");
    assert_eq!(strings(&kept["event"]["labels"]), ["kvm"]);

    // Remove the menu `#kvm`: gone from that message, the typed one stands.
    let removed = ok(
        state,
        "DELETE",
        &label_uri(&plain_id),
        Some(json!({"name": "#kvm"})),
    )
    .await;
    assert_eq!(removed["changed"], true);
    assert!(strings(&removed["event"]["labels"]).is_empty(), "{removed}");
    let after = ok(state, "GET", &search_uri("label:#kvm"), None).await;
    assert_eq!(hit_ids(&after), std::slice::from_ref(&typed_id), "{after}");

    // Server-local: not one write reached the chat provider at any point.
    let requests = d.fake.with(|s| s.requests.clone());
    assert!(requests.len() >= requests_before);
    let wrote: Vec<&String> = requests
        .iter()
        .map(|(method, _)| method)
        .filter(|method| writes_upstream(method))
        .collect();
    assert!(wrote.is_empty(), "a label reached the provider: {wrote:?}");
}

#[tokio::test]
async fn a_thread_reply_is_labelled_by_its_own_id_and_names_are_checked() {
    let d = deployment("labels-thread-reply").await;
    let state = &d.state;
    let reply = d.id("a reply in the thread");
    let root = d.id("thread root here");
    let added = ok(
        state,
        "PUT",
        &label_uri(&reply),
        Some(json!({"name": "#followup"})),
    )
    .await;
    assert_eq!(strings(&added["event"]["labels"]), ["followup"], "{added}");
    assert_eq!(
        added["event"]["derived"], true,
        "the reply's text is held: {added}"
    );

    // The thread read serves the label on the reply, not on its root.
    let thread = ok(
        state,
        "GET",
        &format!(
            "/api/v1/channels/{CH}/timeline?view=thread&thread_id={}&limit=50",
            d.thread
        ),
        None,
    )
    .await;
    assert_eq!(strings(&served(&thread, &reply)["labels"]), ["followup"]);
    assert!(served(&thread, &root).get("labels").is_none(), "{thread}");
    let found = ok(state, "GET", &search_uri("label:#followup"), None).await;
    assert_eq!(hit_ids(&found), std::slice::from_ref(&reply), "{found}");

    // The channel scopes the id: another channel is refused, not a second label.
    let (status, refused) = send(
        state,
        "PUT",
        &format!("/api/v1/channels/C0000OTHER/message-labels/{reply}"),
        WRITE_TOKEN,
        Some(json!({"name": "#followup"})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{refused}");

    // Names the normalizer refuses, with the reason; the read token may not write.
    for name in ["#1abc", "#pinned", "pinned", "auto:x", ""] {
        let (status, refused) = send(
            state,
            "PUT",
            &label_uri(&reply),
            WRITE_TOKEN,
            Some(json!({"name": name})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name:?}: {refused}");
    }
    let (status, _) = send(
        state,
        "PUT",
        &label_uri(&reply),
        READ_TOKEN,
        Some(json!({"name": "#x"})),
    )
    .await;
    assert!(
        status.is_client_error(),
        "the read token must not label: {status}"
    );
}
