//! `#1550 thread-list-pins`: the owner's pinned threads, over HTTP.
//!
//! Thread pins are SEPARATE from message pins (`tests/pins.rs`, #206): no
//! snapshot is kept, only the owner's choice per thread. A bang-bound thread
//! (root `!@NAME`, #917) is pinned by default with no record; unpinning one
//! records `pinned: false`, and that record is what keeps it unpinned.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::http::router;
use vibe_talk::testing::{self, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN};

struct Harness {
    router: axum::Router,
}

impl Harness {
    fn new() -> Self {
        let (state, _discord, _store) = testing::state_with_store();
        Self {
            router: router(state),
        }
    }

    async fn call(&self, method: &str, uri: &str, token: Option<&str>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let request = builder.body(Body::empty()).expect("request");
        let response = self
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

    async fn list(&self, channel: &str) -> Value {
        let (status, body) = self
            .call(
                "GET",
                &format!("/api/v1/channels/{channel}/thread-pins"),
                Some(READ_TOKEN),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body
    }
}

fn records(list: &Value) -> Vec<(String, bool)> {
    list["records"]
        .as_array()
        .expect("a list")
        .iter()
        .map(|record| {
            (
                record["thread_id"].as_str().expect("an id").to_owned(),
                record["pinned"].as_bool().expect("a flag"),
            )
        })
        .collect()
}

#[tokio::test]
async fn pin_unpin_are_idempotent_listed_and_scoped() {
    let harness = Harness::new();
    let empty = harness.list(READ_CHANNEL).await;
    assert_eq!(records(&empty), Vec::new());
    assert_eq!(empty["revision"], json!(0));

    let uri = format!("/api/v1/channels/{READ_CHANNEL}/thread-pins/thread-a");
    let (status, body) = harness.call("PUT", &uri, Some(WRITE_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pinned"], json!(true));
    assert_eq!(body["changed"], json!(true));

    // A repeat changes nothing and does not move the revision.
    let (status, again) = harness.call("PUT", &uri, Some(WRITE_TOKEN)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["changed"], json!(false));
    assert_eq!(again["revision"], body["revision"]);

    let listed = harness.list(READ_CHANNEL).await;
    assert_eq!(records(&listed), vec![("thread-a".to_owned(), true)]);

    // Unpinning records the choice (it does not erase the row): this is what
    // keeps a bang-bound thread, pinned by default, unpinned.
    let (status, body) = harness.call("DELETE", &uri, Some(WRITE_TOKEN)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["pinned"], json!(false));
    assert_eq!(body["changed"], json!(true));
    let listed = harness.list(READ_CHANNEL).await;
    assert_eq!(
        records(&listed),
        vec![("thread-a".to_owned(), false)],
        "an unpin is a kept record, not an absence"
    );

    // Message pins are a different list: nothing above touched them.
    let (status, pins) = harness
        .call(
            "GET",
            &format!("/api/v1/channels/{READ_CHANNEL}/pins"),
            Some(READ_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pins["pins"].as_array().expect("a list").len(), 0);
}

#[tokio::test]
async fn listing_is_a_read_and_writing_needs_the_write_scope() {
    let harness = Harness::new();
    let uri = format!("/api/v1/channels/{READ_CHANNEL}/thread-pins/thread-a");
    let (status, _) = harness.call("PUT", &uri, Some(READ_TOKEN)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = harness
        .call(
            "GET",
            &format!("/api/v1/channels/{READ_CHANNEL}/thread-pins"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = harness
        .call(
            "GET",
            "/api/v1/channels/no-such-channel/thread-pins",
            Some(READ_TOKEN),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn thread_pins_survive_the_store_being_closed_and_reopened() {
    let dir = std::env::temp_dir().join(format!("vibe-talk-thread-pins-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("state.sqlite3");
    let channel = vibe_talk::model::ChannelId(READ_CHANNEL.to_owned());
    {
        let store = vibe_talk::store::sqlite::SqliteStore::open(
            &path,
            vibe_talk::store::Retention::default(),
        )
        .expect("open");
        use vibe_talk::store::StateStore as _;
        store
            .set_thread_pin(&channel, "thread-a", true)
            .await
            .expect("pin");
        store
            .set_thread_pin(&channel, "thread-b", false)
            .await
            .expect("unpin a default");
    }
    let store =
        vibe_talk::store::sqlite::SqliteStore::open(&path, vibe_talk::store::Retention::default())
            .expect("reopen");
    use vibe_talk::store::StateStore as _;
    let pins = store.thread_pins(&channel).await.expect("read");
    assert_eq!(
        pins.records
            .iter()
            .map(|r| (r.thread_id.as_str(), r.pinned))
            .collect::<Vec<_>>(),
        vec![("thread-a", true), ("thread-b", false)]
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = Arc::new(());
}

#[test]
fn a_bang_root_names_its_agent_and_an_ordinary_root_does_not() {
    use vibe_talk::threads::bang_agent_of;
    assert_eq!(bang_agent_of("!@liteinst please look"), Some("liteinst"));
    assert_eq!(bang_agent_of("!@liteinst"), Some("liteinst"));
    assert_eq!(bang_agent_of("hello !@liteinst"), None);
    assert_eq!(bang_agent_of("!@Bad Name hi"), None);
    assert_eq!(bang_agent_of("!help"), None);
    assert_eq!(bang_agent_of(""), None);
}
