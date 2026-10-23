//! `#473 replication-c7-server-versions`, the server's half: every copy a page is served carries
//! its version on this server's clock (`observed_ms`), and every newest page says when its read
//! began and where the stretch it proves absent ends (`settled_below_ms`), so the page can order
//! copies as the server does instead of by when it sent its reads.
//!
//! Against the loopback Slack fake.

mod slack_fake;

use std::sync::Arc;

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

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"));
    let body = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            Body::from(body.to_string())
        }
        None => Body::empty(),
    };
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request.body(body).expect("request"))
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

fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(
        TEAM.to_owned(),
        vec![
            profiled(&ts(10, 0), "UADA00001", "Ada", "is the build green?"),
            profiled(&ts(20, 0), "UADA00001", "Ada", "and the deploy?"),
        ],
    );
    state
}

#[tokio::test]
async fn a_newest_page_and_every_copy_on_it_carry_the_servers_version() {
    let fake = Fake::start(fixture(), 50).await;
    let state = slack(&fake).await;
    let before = vibe_talk::store::now_ms();
    let (status, page) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{TEAM}/timeline"),
        None,
    )
    .await;
    let after = vibe_talk::store::now_ms();
    assert_eq!(status, StatusCode::OK, "{page}");
    let observed = page["observed_ms"].as_i64().expect("the page's version");
    assert!(
        before <= observed && observed <= after,
        "{observed} not in [{before}, {after}]"
    );
    assert_eq!(
        page["settled_below_ms"].as_i64(),
        Some(observed - vibe_talk::cache::coverage::SETTLE_MS),
        "a newest page says where its proven stretch ends"
    );
    let messages = page["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    for message in messages {
        assert_eq!(message["observed_ms"].as_i64(), Some(observed), "{message}");
    }
    // A step back proves no absence.
    let before_cursor = page["next_before"].as_str().map(str::to_owned);
    if let Some(cursor) = before_cursor {
        let (status, older) = call(
            &state,
            "GET",
            &format!("/api/v1/channels/{TEAM}/timeline?before={cursor}"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{older}");
        assert!(older.get("settled_below_ms").is_none(), "{older}");
    }
}

#[tokio::test]
async fn this_servers_own_post_carries_its_version() {
    let fake = Fake::start(fixture(), 50).await;
    let state = slack(&fake).await;
    let before = vibe_talk::store::now_ms();
    let (status, answer) = call(
        &state,
        "POST",
        &format!("/api/v1/channels/{TEAM}/reply"),
        Some(json!({ "text": "green, deploying" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let observed = answer["parts"][0]["observed_ms"]
        .as_i64()
        .expect("a version");
    assert!(observed >= before, "{answer}");
}
