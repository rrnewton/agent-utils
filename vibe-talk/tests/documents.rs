//! `documents-index-server` (`#628`): the fold index, fetch-on-demand and list
//! APIs, with a fake provider and a fake GitHub (an in-process fetcher).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::documents::{Entry, FetchError, FetchedRaw, Fetcher, Index, Service};
use vibe_talk::http::router;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};

const KEY: &str = "github:octo/widgets@main:docs/report.md";
const URL: &str = "https://github.com/octo/widgets/blob/main/docs/report.md";

/// A fake GitHub: contents by key, with a version the test can advance.
#[derive(Default)]
struct FakeGithub {
    contents: Mutex<HashMap<String, String>>,
    versions: Mutex<HashMap<String, String>>,
    fetches: Mutex<usize>,
}

impl FakeGithub {
    fn put(&self, key: &str, content: &str, version: &str) {
        self.contents
            .lock()
            .expect("lock")
            .insert(key.to_owned(), content.to_owned());
        self.versions
            .lock()
            .expect("lock")
            .insert(key.to_owned(), version.to_owned());
    }
}

#[async_trait::async_trait]
impl Fetcher for FakeGithub {
    async fn fetch(&self, entry: &Entry) -> Result<FetchedRaw, FetchError> {
        *self.fetches.lock().expect("lock") += 1;
        let content = self
            .contents
            .lock()
            .expect("lock")
            .get(&entry.key)
            .cloned()
            .ok_or_else(|| FetchError::Failed("not found".to_owned()))?;
        Ok(FetchedRaw {
            content,
            version: self.versions.lock().expect("lock").get(&entry.key).cloned(),
        })
    }

    async fn current_version(&self, entry: &Entry) -> Result<Option<String>, FetchError> {
        Ok(self.versions.lock().expect("lock").get(&entry.key).cloned())
    }
}

fn message(id: &str, at_ms: i64, text: &str) -> Message {
    Message {
        thread: None,
        id: MessageId(id.to_owned()),
        channel_id: ChannelId(READ_CHANNEL.to_owned()),
        author: "agent".to_owned(),
        author_id: UserId("u1".to_owned()),
        author_is_bot: true,
        timestamp: String::new(),
        spoken_time: String::new(),
        edited_timestamp: None,
        reply_to: None,
        content: text.to_owned(),
        spoken_content: String::new(),
        content_html: String::new(),
        noise: false,
        reactions: None,
        heading: None,
        mentions: Vec::new(),
        upstream_url: None,
        upstream_app_url: None,
        op_key: None,
        observed_ms: Some(at_ms),
    }
}

/// A router whose documents service indexes two mentions of one report, plus a
/// second document, fetching from the fake GitHub.
fn harness() -> (axum::Router, Arc<FakeGithub>) {
    let (mut state, _discord) = testing::state();
    let github = Arc::new(FakeGithub::default());
    github.put(KEY, "# Real Title\n\nbody\n\n## Part\n", "v1");
    let index = Arc::new(Index::new(Vec::new()));
    // What the cache fold produces as the two messages are cached.
    let first = message("1000000000000000001", 1_000, &format!("[report]({URL})"));
    let second = message("1000000000000000002", 2_000, URL);
    for m in [&first, &second] {
        index.apply_message(
            m.channel_id.as_str(),
            m.id.as_str(),
            m.created_ms().or(m.observed_ms),
            &m.author,
            None,
            &m.content,
        );
    }
    index.apply_message(
        READ_CHANNEL,
        "1000000000000000003",
        Some(3_000),
        "agent",
        None,
        "https://github.com/octo/widgets/blob/main/docs/other.md",
    );
    state.documents = Arc::new(Service::new(index, github.clone(), 100, 1 << 20));
    (router(state), github)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let request = match body {
        Some(json) => builder
            .header("content-type", "application/json")
            .body(Body::from(json.to_string()))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    let response = app.clone().oneshot(request).await.expect("router responds");
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

fn encoded(key: &str) -> String {
    // The key's `/`, `:` and `@` percent-encoded, as the page sends it.
    key.replace('%', "%25")
        .replace(':', "%3A")
        .replace('@', "%40")
        .replace('/', "%2F")
}

#[tokio::test]
async fn list_is_mention_order_dimmed_then_fetched_with_real_title() {
    let (app, _github) = harness();
    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let docs = body["documents"].as_array().expect("documents");
    assert_eq!(docs.len(), 2);
    // Mention order: newest mention at the bottom.
    assert!(
        docs[0]["key"].as_str().expect("key").ends_with("other.md"),
        "{body}"
    );
    assert_eq!(docs[1]["key"], KEY);
    assert_eq!(
        docs[1]["fetched"], false,
        "unfetched rows are dimmed: {body}"
    );
    assert_eq!(
        docs[1]["title"], "report",
        "link label names the dimmed row"
    );
    assert_eq!(docs[1]["mention_count"], 2);
    assert!(body["revision"].as_u64().expect("revision") >= 1);

    // Reading the list at read scope must not fetch; fetching needs write scope.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(READ_TOKEN),
        Some(serde_json::json!({ "key": KEY })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": KEY })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fetched"][0]["title"], "Real Title");

    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let docs = body["documents"].as_array().expect("documents");
    assert_eq!(docs[1]["fetched"], true);
    assert_eq!(docs[1]["title"], "Real Title");

    // The detail is cacheable by version: content rides with its version.
    let uri = format!("/api/v1/documents/{}", encoded(KEY));
    let (status, body) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["document"]["version"], "v1");
    assert!(body["content"]
        .as_str()
        .expect("content")
        .contains("Real Title"));
}

#[tokio::test]
async fn fetch_recent_and_freshness_and_since_sync() {
    let (app, github) = harness();
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch-recent",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "limit": 20 })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Only the fake-backed document fetches; the other has no fake content.
    assert_eq!(body["fetched"].as_array().expect("fetched").len(), 1);

    // Fetching again is idempotent: no second fetch of the same document.
    let before = *github.fetches.lock().expect("lock");
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": KEY })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(*github.fetches.lock().expect("lock"), before);

    // A newer version at the source is reported, not swapped in.
    github.put(KEY, "# Real Title v2\n", "v2");
    let uri = format!("/api/v1/documents/{}/freshness", encoded(KEY));
    let (status, body) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["update_available"], true);
    assert_eq!(body["version"], "v1");
    assert_eq!(body["latest_version"], "v2");

    // The refresh tap replaces the cached copy.
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": KEY, "refresh": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fetched"][0]["version"], "v2");

    // since_revision sync: from 0 everything is a change; from current, nothing.
    let (status, body) = call(
        &app,
        "GET",
        "/api/v1/documents?since_revision=0",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["changed"].as_array().expect("changed").len(), 2);
    let revision = body["revision"].as_u64().expect("revision");
    let uri = format!("/api/v1/documents?since_revision={revision}");
    let (status, body) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["changed"].as_array().expect("changed").is_empty());
    assert!(body["deleted"].as_array().expect("deleted").is_empty());
}

#[tokio::test]
async fn cache_fills_fold_the_index() {
    // The real path: a write-token timeline read fills the cache, and the fill
    // folds its messages into the document index. An edit read again replaces
    // what the message mentioned.
    const T0: i64 = 1_787_054_400_000;
    let (mut state, fake, _store) = testing::state_with_store_from_toml(&testing::config_toml());
    let seeded = fake.seed_at(
        &ChannelId(READ_CHANNEL.to_owned()),
        "agent",
        &format!("[report]({URL})"),
        T0,
    );
    let chat = Arc::new(testing::TimelineChat::new(fake.clone()));
    state.replace_chat(chat);
    let dir = testing::TempDir::new("documents-fold");
    let clock = testing::ManualClock::at(T0 + 3_600_000);
    let state = testing::with_message_cache(state, dir.path(), clock).await;
    let app = router(state.clone());
    let uri = format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50");
    let (status, body) = call(&app, "GET", &uri, Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        state.documents.index.len(),
        1,
        "the fill folded the link in: {body}"
    );
    assert_eq!(state.documents.index.list()[0].key, KEY);

    // Edit upstream and read again: the mention is replaced, not added.
    fake.edit(
        &seeded,
        "https://github.com/octo/widgets/blob/main/docs/other.md",
    );
    let (status, body) = call(&app, "GET", &uri, Some(WRITE_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let keys: Vec<String> = state
        .documents
        .index
        .list()
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(keys.len(), 1, "{keys:?}");
    assert!(
        keys[0].ends_with("other.md"),
        "the edit replaced the mention: {keys:?}"
    );

    // A read-scope caller has no fill, so it can fold nothing in.
    assert!(
        state
            .message_cache
            .begin_for(
                vibe_talk::auth::Scope::Read,
                &ChannelId(READ_CHANNEL.to_owned())
            )
            .is_none(),
        "a read-scope caller has no fill, so it can fold nothing in"
    );
}

#[tokio::test]
async fn bounded_cache_evicts_oldest() {
    let github = Arc::new(FakeGithub::default());
    let index = Arc::new(Index::new(Vec::new()));
    for n in 0..3i64 {
        let key = format!("github:octo/widgets@main:docs/d{n}.md");
        github.put(&key, &format!("# Doc {n}\n"), "v1");
        index.apply_message(
            READ_CHANNEL,
            &format!("100000000000000000{n}"),
            Some(n + 1),
            "agent",
            None,
            &format!("https://github.com/octo/widgets/blob/main/docs/d{n}.md"),
        );
    }
    let service = Service::new(index, github, 2, 1 << 20);
    for n in 0..3 {
        service
            .fetch_one(&format!("github:octo/widgets@main:docs/d{n}.md"))
            .await
            .expect("fetch");
    }
    let (count, _bytes) = service.cache_stats();
    assert_eq!(count, 2, "the bounded cache holds at most its ceiling");
    assert!(
        service
            .cached("github:octo/widgets@main:docs/d0.md")
            .is_none(),
        "the oldest fetched document was evicted"
    );
}
