//! Each chat provider's diagnostics checks carry ids named for it, and a Slack token's owner is
//! named by display name. `#325 provider-diagnostics-ids`.
//!
//! A Slack deployment reported its Slack checks under `discord.token` and `discord.channel` while
//! their titles said Slack, and named the token's account by a handle that, in a workspace whose
//! handles are numbers, named nobody. Against the loopback Slack fake and the in-memory Discord;
//! the both-providers case builds a real Discord client from the legacy `[discord]` section, which
//! names the public Discord API, and the test harness keeps that client on loopback
//! (`#359 provider-diagnostics-ids`, see `vibe_talk::egress`).

mod slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{profiled, ts, Fake, FakeState, SELF_USER, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;

const TEAM: &str = "C0000TEAM1";

/// A workspace whose handles are numbers: the token's account is `100009`, and its people know it
/// as "Release Bot".
fn fixture(with_name: bool) -> FakeState {
    let mut state = FakeState {
        auth_user: Some("100009".to_owned()),
        ..FakeState::default()
    };
    state.history.insert(
        TEAM.to_owned(),
        vec![profiled(&ts(10, 0), "U0ADA0001", "Ada", "hello")],
    );
    if with_name {
        state.users.insert(
            SELF_USER.to_owned(),
            json!({"id": SELF_USER, "name": "100009", "is_bot": true,
                "profile": {"display_name": "Release Bot", "real_name": "Release Bot"}}),
        );
    }
    state
}

async fn report(state: &AppState) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/diagnostics")
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
    serde_json::from_slice(&bytes).expect("json")
}

fn ids(body: &Value) -> Vec<String> {
    body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .filter_map(|check| check["id"].as_str().map(str::to_owned))
        .collect()
}

fn check<'b>(body: &'b Value, id: &str) -> &'b Value {
    body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|check| check["id"] == id)
        .unwrap_or_else(|| panic!("no {id} check: {body:#}"))
}

async fn slack_deployment(fake: &Fake) -> AppState {
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{api_base}"
token = "{TOKEN}"

[[channels]]
id = "{TEAM}"
label = "team"
"#,
        read = vibe_talk::testing::READ_TOKEN,
        api_base = fake.base,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

#[tokio::test]
async fn a_slack_deployments_checks_are_slack_checks() {
    let fake = Fake::start(fixture(true), 20).await;
    let body = report(&slack_deployment(&fake).await).await;
    let ids = ids(&body);
    assert!(
        !ids.iter().any(|id| id.starts_with("discord.")),
        "a Slack check under a Discord id: {ids:?}"
    );
    let token = check(&body, "slack.token");
    assert_eq!(token["status"], "pass", "{token:#}");
    assert_eq!(token["title"], "Slack token");
    let detail = token["detail"].as_str().expect("detail");
    assert!(
        detail.contains("it belongs to Release Bot (UBOTSELF1)"),
        "the token's owner by name: {detail}"
    );
    assert!(!detail.contains("100009"), "the numeric handle: {detail}");
    let channel = check(&body, "slack.channel");
    assert_eq!(channel["subject"], TEAM);
    assert!(
        channel["title"]
            .as_str()
            .is_some_and(|title| title.starts_with("Slack channel: team")),
        "{channel:#}"
    );
}

#[tokio::test]
async fn without_a_display_name_the_token_is_named_by_its_handle() {
    let fake = Fake::start(fixture(false), 20).await;
    let body = report(&slack_deployment(&fake).await).await;
    let detail = check(&body, "slack.token")["detail"]
        .as_str()
        .expect("detail")
        .to_owned();
    assert!(
        detail.contains("it belongs to 100009 (UBOTSELF1)"),
        "{detail}"
    );
}

#[tokio::test]
async fn a_discord_deployment_keeps_its_discord_ids() {
    // The negative control: a Discord-protocol provider's ids are the ones they always were.
    let (state, _discord) = vibe_talk::testing::state();
    let body = report(&state).await;
    let ids = ids(&body);
    assert!(ids.iter().any(|id| id == "discord.token"), "{ids:?}");
    assert_eq!(ids.iter().filter(|id| *id == "discord.channel").count(), 2);
    assert!(!ids.iter().any(|id| id.starts_with("slack.")), "{ids:?}");
}

#[tokio::test]
async fn with_both_providers_each_check_is_named_for_its_own() {
    let fake = Fake::start(fixture(true), 20).await;
    let text =
        vibe_talk::testing::config_toml().replace(
            "[discord]\n",
            &format!(
                "[[providers]]\nkey = \"slack\"\nkind = \"slack\"\napi_base = \"{}\"\ntoken = \
             \"{TOKEN}\"\n\n[discord]\n",
                fake.base
            ),
        ) + &format!("\n[[channels]]\nid = \"{TEAM}\"\nlabel = \"team\"\nprovider = \"slack\"\n");
    let (mut state, _discord, _vendor) = vibe_talk::testing::state_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let body = report(&state).await;
    assert_eq!(check(&body, "slack.token")["subject"], "slack");
    assert_eq!(check(&body, "discord.token")["subject"], "discord");
    // The Discord client names the public API and is held on loopback: unreachable on every
    // host, never the public API's answer to a test token, which ended the channel probe early.
    let discord_token = check(&body, "discord.token")["summary"]
        .as_str()
        .unwrap_or_default();
    assert!(discord_token.contains("never reached"), "{body:#}");
    assert_eq!(check(&body, "slack.channel")["subject"], TEAM);
    let discord_channels = body["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .filter(|check| check["id"] == "discord.channel")
        .count();
    assert_eq!(discord_channels, 2, "{body:#}");
}
