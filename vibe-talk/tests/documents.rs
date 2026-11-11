//! `documents-index-server` (`#628`): the fold index, fetch-on-demand and list
//! APIs, with a fake provider and a fake GitHub (an in-process fetcher).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::documents::{
    Entry, FetchError, FetchedRaw, Fetcher, GithubFetcher, Index, Service, TokenSource,
};
use vibe_talk::elevenlabs::fake::FakeElevenLabs;
use vibe_talk::http::router;
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::speech_tickets::SpeechTickets;
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
            moved_to: None,
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
    let (app, github, _tickets, _elevenlabs) = speech_harness();
    (app, github)
}

/// [`harness`], plus the speech tickets it mints into and the fake vendor it
/// speaks through: what a document speech ticket holds, and what the vendor
/// was actually told to say (`#1039 documents-read-parity`).
fn speech_harness() -> (
    axum::Router,
    Arc<FakeGithub>,
    Arc<SpeechTickets>,
    Arc<FakeElevenLabs>,
) {
    let (mut state, _discord, elevenlabs) = testing::state_parts();
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
    let tickets = Arc::clone(&state.speech_tickets);
    (router(state), github, tickets, elevenlabs)
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

/// [`call`] for an answer that is audio, not JSON: what a ticket plays.
async fn call_bytes(
    app: &axum::Router,
    method: &str,
    uri: &str,
    token: Option<&str>,
) -> (StatusCode, Vec<u8>) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    (status, bytes.to_vec())
}

/// Fetch the report into the cache, as the page's fetch does before a read.
async fn cache_report(app: &axum::Router) {
    let (status, body) = call(
        app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": KEY })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
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
async fn issue_and_gist_rows_carry_their_kinds_and_issue_fields() {
    // `#1176 doc-kind-buckets`: an issue link and a gist link fold beside a
    // markdown link, each with its own kind; the issue row names its number
    // before fetching and its status after.
    let (mut state, _discord, _elevenlabs) = testing::state_parts();
    let github = Arc::new(FakeGithub::default());
    let issue_key = "github:octo/widgets@main:.minibeads/issues/agent-utils-1176.md";
    github.put(
        issue_key,
        "---\ntitle: \"Buckets\"\nstatus: open\n---\n\n# Buckets\n\nbody\n",
        "v9",
    );
    github.put(
        "gist:0123456789abcdef0123456789abcdef",
        "# Gist Note\n",
        "v3",
    );
    let index = Arc::new(Index::new(Vec::new()));
    for (id, at, text) in [
        (
            "1000000000000000011",
            1_000_i64,
            "https://github.com/octo/widgets/blob/main/.minibeads/issues/agent-utils-1176.md",
        ),
        (
            "1000000000000000012",
            2_000,
            "https://gist.github.com/octo/0123456789abcdef0123456789abcdef",
        ),
        (
            "1000000000000000013",
            3_000,
            "https://github.com/octo/widgets/blob/main/docs/plain.md",
        ),
    ] {
        index.apply_message(READ_CHANNEL, id, Some(at), "agent", None, text);
    }
    state.documents = Arc::new(Service::new(index, github, 100, 1 << 20));
    let app = router(state);

    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let docs = body["documents"].as_array().expect("documents");
    assert_eq!(docs.len(), 3, "{body}");
    let by_kind: std::collections::HashMap<&str, &Value> = docs
        .iter()
        .map(|d| (d["kind"].as_str().expect("kind"), d))
        .collect();
    assert_eq!(by_kind["github_issue"]["issue_number"], 1176);
    assert_eq!(by_kind["github_issue"]["issue_status"], Value::Null);
    assert_eq!(
        by_kind["github_gist"]["key"],
        "gist:0123456789abcdef0123456789abcdef"
    );
    assert_eq!(by_kind["github_markdown"]["issue_number"], Value::Null);

    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": issue_key })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    let docs = body["documents"].as_array().expect("documents");
    let issue = docs
        .iter()
        .find(|d| d["kind"] == "github_issue")
        .expect("issue row");
    assert_eq!(issue["issue_status"], "open", "{body}");
    assert_eq!(issue["title"], "Buckets");
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
    // Checking spends credentials, so a read token may not ask.
    let (status, _) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = call(&app, "GET", &uri, Some(WRITE_TOKEN), None).await;
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
async fn paste_documents_fetch_through_the_command_source() {
    let index = Arc::new(Index::new(vec!["https://paste.example/".to_owned()]));
    index.apply_message(
        READ_CHANNEL,
        "1000000000000000009",
        Some(9),
        "agent",
        None,
        "<https://paste.example/Ab12|notes>",
    );
    let service = Service::new(
        index,
        Arc::new(vibe_talk::documents::PasteFetcher::new(vec![
            "/bin/echo".to_owned(),
            "--".to_owned(),
            "{}".to_owned(),
        ])),
        10,
        1 << 20,
    );
    let fetched = service.fetch_one("paste:Ab12").await.expect("paste fetch");
    assert!(fetched.content.contains("Ab12"), "{fetched:?}");
    // Removing its last mention removes the document, and its content with it.
    service
        .index
        .remove_message(READ_CHANNEL, "1000000000000000009");
    assert!(service.index.get("paste:Ab12").is_none());
    assert!(
        service.fetch_one("paste:Ab12").await.is_err(),
        "content never outlives its last mention"
    );
    service.index.remove_channel(READ_CHANNEL);
    assert!(service.index.is_empty());
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

/// `#1081 documents-relative-links`: a canonical key no message mentioned is
/// admitted by the write-scope fetch (registered mention-less, then fetched);
/// the read path stays read-only, and what does not canonicalise under this
/// deployment's kinds is refused without ever being indexed.
#[tokio::test]
async fn fetch_admits_an_unmentioned_document_by_key() {
    const LINKED: &str = "github:octo/widgets@main:docs/never-mentioned.md";
    let (app, github) = harness();
    github.put(
        LINKED,
        "# Never Mentioned\n\nlinked only from a document\n",
        "v1",
    );
    let uri = format!("/api/v1/documents/{}", encoded(LINKED));

    // GET stays read-only: an unindexed key is unknown until admitted.
    let (status, body) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "unknown_document", "{body}");

    // A read token still may not fetch — admission included.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(READ_TOKEN),
        Some(serde_json::json!({ "key": LINKED })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a read-token fetch admitted it"
    );

    // The write-scope fetch registers and fetches it, exactly once.
    let before = *github.fetches.lock().expect("lock");
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": LINKED })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["fetched"][0]["title"], "Never Mentioned");
    assert_eq!(*github.fetches.lock().expect("lock"), before + 1);
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": LINKED })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        *github.fetches.lock().expect("lock"),
        before + 1,
        "the second fetch was not served from the cache"
    );

    // It reads back mention-less, and lists without disturbing mention order.
    let (status, body) = call(&app, "GET", &uri, Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["content"]
        .as_str()
        .expect("content")
        .contains("Never Mentioned"));
    assert_eq!(body["document"]["mention_count"], 0);
    assert!(body["document"]["mentions"]
        .as_array()
        .expect("mentions")
        .is_empty());
    assert_eq!(body["document"]["fetched"], true);

    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 3, "{body}");
    let docs = body["documents"].as_array().expect("documents");
    let mentioned: Vec<&str> = docs
        .iter()
        .filter(|d| d["mention_count"].as_u64().expect("count") > 0)
        .map(|d| d["key"].as_str().expect("key"))
        .collect();
    assert_eq!(mentioned.len(), 2, "{body}");
    assert!(
        mentioned[0].ends_with("other.md"),
        "mention order moved: {body}"
    );
    assert_eq!(mentioned[1], KEY, "mention order moved: {body}");

    // Keys that do not canonicalise are refused and never indexed: malformed,
    // an address rather than a key, and a paste with no paste source
    // configured in this harness (an unconfigured kind).
    for bad in [
        "not a key",
        "github:octo/widgets@main:",
        "github:octo/widgets@main:../escape.md",
        "https://github.com/octo/widgets/blob/main/docs/a.md",
        "https://example.com/anything",
        "paste:Ab12",
        "google_doc:DocId123",
    ] {
        let (status, body) = call(
            &app,
            "POST",
            "/api/v1/documents/fetch",
            Some(WRITE_TOKEN),
            Some(serde_json::json!({ "key": bad })),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "admitted {bad}: {body}");
    }
    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 3, "a refused key was indexed: {body}");
}

/// The same admission for a paste, under its configured prefix.
#[tokio::test]
async fn fetch_admits_an_unmentioned_paste_under_its_prefix() {
    let (mut state, _discord) = testing::state();
    let index = Arc::new(Index::new(vec!["https://paste.example/".to_owned()]));
    state.documents = Arc::new(Service::new(
        index,
        Arc::new(vibe_talk::documents::PasteFetcher::new(vec![
            "/bin/echo".to_owned(),
            "--".to_owned(),
            "{}".to_owned(),
        ])),
        10,
        1 << 20,
    ));
    let app = router(state);
    let (status, body) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": "paste:Ab12" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, body) = call(
        &app,
        "GET",
        &format!("/api/v1/documents/{}", encoded("paste:Ab12")),
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["content"].as_str().expect("content").contains("Ab12"));
    assert_eq!(body["document"]["mention_count"], 0);
    // A malformed paste key is refused, not indexed.
    let (status, _) = call(
        &app,
        "POST",
        "/api/v1/documents/fetch",
        Some(WRITE_TOKEN),
        Some(serde_json::json!({ "key": "paste:bad/id" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = call(&app, "GET", "/api/v1/documents", Some(READ_TOKEN), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 1, "{body}");
}

// --- moved-document fallback, against a fake GitHub over HTTP ----------------

/// A one-thread HTTP GitHub: raw paths and one tree answer, both canned.
struct FakeHttpGithub {
    port: u16,
    tree_hits: Arc<std::sync::atomic::AtomicUsize>,
}

fn fake_http_github(tree_paths: Vec<&str>) -> FakeHttpGithub {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let tree_hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = tree_hits.clone();
    let tree = serde_json::json!({
        "tree": tree_paths.iter().map(|p| serde_json::json!({"path": p, "type": "blob"})).collect::<Vec<_>>()
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            use std::io::{Read as _, Write as _};
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let request = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = request
                .split_whitespace()
                .nth(1)
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .to_owned();
            let (status, body) = if path.contains("/git/trees/") {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                (200, tree.clone())
            } else if path.ends_with("/reports/old.md") {
                (200, "# Moved report\n\nbody\n".to_owned())
            } else {
                (404, "not found".to_owned())
            };
            let reason = if status == 200 { "OK" } else { "Not Found" };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                    .as_bytes(),
            );
        }
    });
    FakeHttpGithub { port, tree_hits }
}

fn http_fetcher(fake: &FakeHttpGithub) -> GithubFetcher {
    let base = format!("http://127.0.0.1:{}", fake.port);
    GithubFetcher::new(TokenSource::None, Some(base.clone()), None).with_api_base(base)
}

fn old_entry(r#ref: &str) -> Entry {
    let text = format!("https://github.com/octo/widgets/blob/{ref}/docs/old.md");
    let index = Index::new(Vec::new());
    index.apply_message("c", "m1", Some(1_000), "agent", None, &text);
    index
        .get(&format!("github:octo/widgets@{ref}:docs/old.md"))
        .expect("entry")
}

#[tokio::test]
async fn a_moved_document_fetches_from_its_new_home() {
    let fake = fake_http_github(vec!["docs/other.md", "reports/old.md"]);
    let fetched = http_fetcher(&fake)
        .fetch(&old_entry("main"))
        .await
        .expect("moved fetch");
    assert!(fetched.content.contains("Moved report"));
    assert_eq!(fetched.moved_to.as_deref(), Some("reports/old.md"));
}

#[tokio::test]
async fn a_missing_or_ambiguous_document_says_not_found() {
    for paths in [vec!["docs/other.md"], vec!["a/old.md", "b/old.md"]] {
        let fake = fake_http_github(paths);
        let error = http_fetcher(&fake)
            .fetch(&old_entry("main"))
            .await
            .expect_err("must not fetch");
        let FetchError::Failed(detail) = error else {
            panic!("wrong failure: {error}");
        };
        assert_eq!(detail, "not found at docs/old.md (moved or deleted)");
    }
}

#[tokio::test]
async fn a_commit_ref_never_looks_in_the_tree() {
    let fake = fake_http_github(vec!["reports/old.md"]);
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let error = http_fetcher(&fake)
        .fetch(&old_entry(sha))
        .await
        .expect_err("must not fetch");
    assert!(matches!(error, FetchError::Failed(_)));
    assert_eq!(
        fake.tree_hits.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "a commit ref looked in the tree"
    );
}
/// `#1039 documents-read-parity`: a cached document prepares like a channel
/// message — one ticket naming the server's cached words, its blocks listed,
/// each block playable through the same ticket URL. Text a caller puts in
/// the request is never what is spoken.
#[tokio::test]
async fn document_speech_prepare_mints_a_ticket_from_cached_content() {
    let (app, github, _tickets, elevenlabs) = speech_harness();
    github.put(
        KEY,
        "First paragraph of the report says hello to the reader.\n\n\
         Second paragraph of the report says goodbye to the reader.\n",
        "v1",
    );
    cache_report(&app).await;

    // The percent-encoded key, as the page sends it, at read scope. The
    // decoy text rides along to prove it is not what gets spoken.
    let uri = format!("/api/v1/documents/{}/speech/prepare", encoded(KEY));
    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(READ_TOKEN),
        Some(serde_json::json!({
            "text": "caller supplied words that must never be spoken"
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["expires_in_seconds"], 600, "{body}");
    let prepared = body["prepared"].as_array().expect("prepared");
    assert_eq!(prepared.len(), 1, "{body}");
    assert_eq!(prepared[0]["message_id"], KEY, "{body}");
    let url = prepared[0]["url"].as_str().expect("url").to_owned();
    assert!(url.starts_with("/api/v1/speech/"), "{url}");
    let blocks = prepared[0]["blocks"].as_array().expect("blocks listed");
    assert_eq!(blocks.len(), 2, "{body}");
    assert_eq!(blocks[0]["kind"], "paragraph", "{body}");
    assert_eq!(blocks[0]["spoken"], true, "{body}");
    assert!(blocks[0]["chars"].as_u64().expect("chars") > 0, "{body}");
    assert_eq!(blocks[0]["element"], 0, "{body}");
    assert_eq!(blocks[0]["piece"], 0, "{body}");
    assert_eq!(blocks[0]["pieces"], 1, "{body}");
    assert_eq!(blocks[1]["element"], 1, "{body}");

    // Block 0 plays from the ticket, with no bearer token, as an
    // `<audio src>` asks; the vendor is told the cached paragraph.
    let (status, audio) = call_bytes(
        &app,
        "GET",
        &format!("{url}?selection=doc-read&block=0"),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!audio.is_empty(), "no audio came back for block 0");
    let spoken = elevenlabs.spoken().join("\n");
    assert!(spoken.contains("First paragraph"), "{spoken}");
    assert!(
        !spoken.contains("caller supplied"),
        "caller text was spoken: {spoken}"
    );

    // A block the preparation never listed is not there to play.
    let (status, body) = call(
        &app,
        "GET",
        &format!("{url}?selection=doc-read&block=99"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "unknown_speech_block", "{body}");
}

/// `#1039 documents-read-parity`: an unknown key is the documents 404; a
/// known document nobody has fetched prepares nothing, and asking does not
/// fetch it.
#[tokio::test]
async fn document_speech_prepare_unknown_is_404_and_uncached_is_empty() {
    let (app, github) = harness();

    let uri = format!(
        "/api/v1/documents/{}/speech/prepare",
        encoded("github:octo/widgets@main:docs/nope.md")
    );
    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(READ_TOKEN),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "unknown_document", "{body}");

    // Indexed by the harness's third message, never fetched.
    let uri = format!(
        "/api/v1/documents/{}/speech/prepare",
        encoded("github:octo/widgets@main:docs/other.md")
    );
    let fetches_before = *github.fetches.lock().expect("lock");
    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(READ_TOKEN),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["prepared"], serde_json::json!([]), "{body}");
    assert_eq!(body["expires_in_seconds"], 600, "{body}");
    assert_eq!(
        *github.fetches.lock().expect("lock"),
        fetches_before,
        "preparing speech fetched the document"
    );
}

/// `#1039 documents-read-parity`: pace and voice are kept on the ticket the
/// way channel preparation keeps them — the pace clamped to the spoken
/// range, the voice trimmed, a voice of only blank space none at all.
#[tokio::test]
async fn document_speech_prepare_keeps_clamped_speed_and_trimmed_voice() {
    let (app, _github, tickets, _elevenlabs) = speech_harness();
    cache_report(&app).await;
    let uri = format!("/api/v1/documents/{}/speech/prepare", encoded(KEY));
    let claim = |body: &Value| {
        let url = body["prepared"][0]["url"].as_str().expect("url");
        let ticket = url.strip_prefix("/api/v1/speech/").expect("ticket url");
        tickets.claim(ticket).expect("the minted ticket is held")
    };

    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(READ_TOKEN),
        Some(serde_json::json!({ "speed": 99.0, "voice": "  some-voice  " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let prepared = claim(&body);
    assert_eq!(prepared.channel, "", "a document has no channel");
    assert_eq!(prepared.message, KEY);
    assert_eq!(prepared.speed, Some(2.0), "the pace was not clamped");
    assert_eq!(
        prepared.voice.as_deref(),
        Some("some-voice"),
        "the voice was not trimmed"
    );

    let (status, body) = call(
        &app,
        "POST",
        &uri,
        Some(READ_TOKEN),
        Some(serde_json::json!({ "speed": 0.01, "voice": "   " })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let prepared = claim(&body);
    assert_eq!(prepared.speed, Some(0.5), "the pace was not clamped");
    assert_eq!(prepared.voice, None, "a blank voice names no voice");

    // And a ticket minted this way still plays whole.
    let url = body["prepared"][0]["url"].as_str().expect("url");
    let (status, audio) = call_bytes(&app, "GET", url, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!audio.is_empty(), "no audio came back");
}

#[tokio::test]
async fn search_finds_only_fetched_documents_by_title_and_body() {
    let (app, github) = harness();
    const OTHER_KEY: &str = "github:octo/widgets@main:docs/other.md";
    github.put(OTHER_KEY, "# Other Notes\n\nzebraword lives here\n", "v1");
    // Before any fetch, nothing is searched.
    let (status, body) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=body",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["searched"], 0);
    assert_eq!(body["hits"].as_array().map(Vec::len), Some(0));

    cache_report(&app).await;
    // Body match.
    let (status, body) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=body",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["searched"], 1, "one fetched document searched: {body}");
    assert_eq!(body["hits"][0]["key"], KEY);
    assert_eq!(body["hits"][0]["title_match"], false);
    assert!(body["hits"][0]["snippet"]
        .as_str()
        .is_some_and(|s| s.contains("body")));
    // Title match, case-insensitively, in the shared language.
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=%22real%20title%22",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(body["hits"][0]["key"], KEY);
    assert_eq!(body["hits"][0]["title_match"], true);
    // No message leakage: "report" is the mentioning messages' link label and
    // the row's file name, but neither the fetched title nor its body.
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=report",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(body["hits"].as_array().map(Vec::len), Some(0), "{body}");
    // No unfetched leakage: the other document holds "zebraword" but is not fetched.
    let (_, body) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=zebraword",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(body["hits"].as_array().map(Vec::len), Some(0), "{body}");
    assert_eq!(body["searched"], 1);
    // An empty query is refused, and a hit needs the read token like the list.
    let (status, _) = call(
        &app,
        "GET",
        "/api/v1/documents/search?q=",
        Some(READ_TOKEN),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "GET", "/api/v1/documents/search?q=body", None, None).await;
    assert_ne!(status, StatusCode::OK);
}
