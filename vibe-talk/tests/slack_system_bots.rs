//! A Slack system bot is drawn as a bot with its own name, never as the coding agent.
//! `#323 slack-system-bots`.
//!
//! The page used to take the one bot in a channel besides this server to be the coding agent, so a
//! Slack workspace's welcome bot, alone in a new channel, was drawn as the agent: no name, no
//! "(bot)", the agent's tile. Against the loopback Slack fake: each kind of system bot reaches the
//! page as a bot, by its own name; and client-config tells the page which accounts are the coding
//! agents (`coding_agent_user_ids`) and that in a Slack channel no other bot may be guessed to be
//! one, while a Discord-protocol provider with nothing configured keeps the guess. The page's half
//! is in `tests/js/voice_page.test.mjs` and `tests/system_bots_browser.py`.

mod slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::model::ChannelId;
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;

const TEAM: &str = "C0000TEAM1";

/// A new channel's first messages: Slackbot, a workspace's welcome bot posting as an integration,
/// an app bot with a bot user, and the coding agent, which posts as a bot too.
fn fixture() -> FakeState {
    let mut state = FakeState::default();
    state.history.insert(
        TEAM.to_owned(),
        vec![
            json!({"type": "message", "ts": ts(10, 0), "user": "USLACKBOT",
                "text": "This is the very beginning of the channel."}),
            json!({"type": "message", "subtype": "bot_message", "ts": ts(20, 0),
                "bot_id": "B0WELCOME1", "username": "Welcome bot",
                "text": "Welcome to your new workspace!"}),
            json!({"type": "message", "ts": ts(30, 0), "user": "U0APPBOT01",
                "bot_id": "B0APPBOT01", "app_id": "A0APPBOT01",
                "bot_profile": {"id": "B0APPBOT01", "name": "Calendar"},
                "text": "Standup in five minutes"}),
            json!({"type": "message", "ts": ts(40, 0), "user": "U0AGENT001",
                "bot_id": "B0AGENT001", "bot_profile": {"id": "B0AGENT001", "name": "Coder"},
                "text": "Build is green"}),
        ],
    );
    // As Slack answers for Slackbot: a user, not a bot.
    state.users.insert(
        "USLACKBOT".to_owned(),
        json!({"id": "USLACKBOT", "name": "slackbot", "is_bot": false,
            "profile": {"display_name": "", "real_name": "Slackbot"}}),
    );
    state
}

#[tokio::test]
async fn every_kind_of_system_bot_is_a_bot_with_its_own_name() {
    let fake = Fake::start(fixture(), 20).await;
    let messages = fake
        .client()
        .fetch_page(&ChannelId(TEAM.to_owned()), 20, None, None)
        .await
        .expect("a page");
    let authors: Vec<(&str, &str, bool)> = messages
        .iter()
        .map(|message| {
            (
                message.author.as_str(),
                message.author_id.as_str(),
                message.author_is_bot,
            )
        })
        .collect();
    assert_eq!(
        authors,
        [
            ("Slackbot", "USLACKBOT", true),
            ("Welcome bot", "B0WELCOME1", true),
            ("Calendar", "U0APPBOT01", true),
            ("Coder", "U0AGENT001", true),
        ]
    );
}

fn config_text(api_base: &str, provider_extra: &str) -> String {
    format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{api_base}"
token = "{TOKEN}"
owner_user_id = "UOWNER001"
{provider_extra}
[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
    )
}

async fn deployment(fake: &Fake, provider_extra: &str) -> AppState {
    let text = config_text(&fake.base, provider_extra);
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

async fn client_config(state: &AppState) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/client-config")
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

#[tokio::test]
async fn a_slack_deployment_tells_the_page_to_guess_no_coding_agent() {
    let fake = Fake::start(fixture(), 20).await;
    let config = client_config(&deployment(&fake, "").await).await;
    assert_eq!(config["coding_agents_named_only"], true, "{config}");
    assert!(config.get("coding_agent_ids").is_none(), "{config}");

    let config = client_config(
        &deployment(
            &fake,
            "coding_agent_user_ids = [\" U0AGENT001 \", \"U0AGENT001\"]\n",
        )
        .await,
    )
    .await;
    assert_eq!(config["coding_agents_named_only"], true, "{config}");
    assert_eq!(
        config["coding_agent_ids"],
        json!(["U0AGENT001"]),
        "trimmed, each once"
    );
}

#[tokio::test]
async fn a_discord_deployment_keeps_the_guess_until_it_names_its_agents() {
    // The negative control: nothing changes for a Discord-protocol provider that names nobody.
    let (state, _discord) = vibe_talk::testing::state();
    let config = client_config(&state).await;
    assert!(config.get("coding_agents_named_only").is_none(), "{config}");
    assert!(config.get("coding_agent_ids").is_none(), "{config}");

    let text = vibe_talk::testing::config_toml().replace(
        "[discord]\n",
        "[discord]\ncoding_agent_user_ids = [\"20\"]\n",
    );
    assert!(text.contains("coding_agent_user_ids"), "the fixture moved");
    let (state, _discord, _vendor) = vibe_talk::testing::state_from_toml(&text);
    let config = client_config(&state).await;
    assert_eq!(config["coding_agents_named_only"], true, "{config}");
    assert_eq!(config["coding_agent_ids"], json!(["20"]));
}

#[tokio::test]
async fn with_several_providers_each_describes_its_own_coding_agents() {
    let fake = Fake::start(fixture(), 20).await;
    let text = vibe_talk::testing::config_toml().replace(
        "[discord]\n",
        &format!(
            "[[providers]]\nkey = \"slack\"\nkind = \"slack\"\napi_base = \"{}\"\ntoken = \
             \"{TOKEN}\"\ncoding_agent_user_ids = [\"U0AGENT001\"]\n\n[discord]\n",
            fake.base
        ),
    );
    let (mut state, _discord, _vendor) = vibe_talk::testing::state_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let config = client_config(&state).await;
    let providers = config["providers"].as_array().expect("providers");
    let slack = providers
        .iter()
        .find(|provider| provider["key"] == "slack")
        .expect("slack");
    let discord = providers
        .iter()
        .find(|provider| provider["key"] == "discord")
        .expect("discord");
    assert_eq!(slack["coding_agent_ids"], json!(["U0AGENT001"]));
    assert_eq!(slack["coding_agents_named_only"], true);
    assert!(discord.get("coding_agent_ids").is_none(), "{discord}");
    assert!(
        discord.get("coding_agents_named_only").is_none(),
        "{discord}"
    );
}
