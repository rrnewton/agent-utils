//! One server, two chat providers: every channel-scoped route reaches the provider that owns the
//! channel, the page is told each provider's own capabilities, and a channel added from the app
//! stays with the provider that registered it across a restart.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity, RegisteredChannel, SourceClaim};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
use vibe_talk::testing::{READ_CHANNEL, WRITE_TOKEN};

const SLACK_CHANNEL: &str = "C0123ABCDE";
const ADDED_SLACK_CHANNEL: &str = "C0999ZZZZZ";
const INGEST_TOKEN: &str = "test-ingest-token-0000000000";

/// The in-memory provider standing in for Slack: its own name, its own account, and certainty
/// about Slack-shaped references only.
struct SlackLike(Arc<FakeDiscord>);

#[async_trait]
impl ChatClient for SlackLike {
    fn provider_name(&self) -> &str {
        "Slack"
    }
    fn claims_source(&self, source: &str) -> SourceClaim {
        if source.contains(".slack.com/") || source.starts_with('C') {
            SourceClaim::Certain
        } else {
            SourceClaim::Never
        }
    }
    fn self_author_id(&self) -> Option<String> {
        Some("U0SELF0001".to_owned())
    }
    fn supports_threading(&self) -> bool {
        true
    }
    fn supports_channel_registration(&self) -> bool {
        true
    }
    async fn register_channel(
        &self,
        source: &str,
        label: &str,
    ) -> Result<RegisteredChannel, ChatError> {
        ChatClient::register_channel(self.0.as_ref(), source, label).await
    }
    async fn unregister_channel(&self, channel: &ChannelId) -> Result<(), ChatError> {
        self.0.unregister_channel(channel).await
    }
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.0.identity().await
    }
    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.0.fetch_page(channel, limit, before, after).await
    }
    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.0.post_message(channel, content, reply_to).await
    }
}

fn config_text() -> String {
    format!(
        "[ingest]\ntoken = \"{INGEST_TOKEN}\"\n\n\
         [[providers]]\nkey = \"slack\"\nkind = \"slack\"\ntoken = \"xoxb-test\"\nlive_poll_seconds = 30\n\n\
         [[channels]]\nid = \"{SLACK_CHANNEL}\"\nlabel = \"slack team\"\nwritable = true\nprovider = \"slack\"\n\n{}",
        vibe_talk::testing::config_toml()
    )
}

struct Deployment {
    state: AppState,
    discord: Arc<FakeDiscord>,
    slack: Arc<FakeDiscord>,
    store: Arc<vibe_talk::store::fake::FakeStore>,
}

/// A server whose Discord is the usual fake and whose Slack is a second one.
fn deployment(store: Option<Arc<vibe_talk::store::fake::FakeStore>>) -> Deployment {
    let (mut state, discord, fresh_store) =
        vibe_talk::testing::state_with_store_from_toml(&config_text());
    let store = match store {
        Some(shared) => {
            state.store = shared.clone();
            shared
        }
        None => fresh_store,
    };
    let slack = Arc::new(FakeDiscord::new());
    slack.register_channel(&ChannelId(SLACK_CHANNEL.to_owned()));
    let entries = vec![
        ProviderEntry {
            key: "discord".to_owned(),
            namespace: state.config.providers[0].namespace(),
            client: discord.clone(),
            live_poll_seconds: 0,
        },
        ProviderEntry {
            key: "slack".to_owned(),
            namespace: state.config.providers[1].namespace(),
            client: Arc::new(SlackLike(slack.clone())),
            live_poll_seconds: 30,
        },
    ];
    let router = ChatRouter::new(entries, state.config.default_provider_key());
    state.replace_providers(Arc::new(router));
    Deployment {
        state,
        discord,
        slack,
        store,
    }
}

async fn call(
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
            .body(Body::from(json.to_string())),
        None => builder.body(Body::empty()),
    }
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

#[tokio::test]
async fn the_page_learns_every_provider_and_which_one_owns_each_channel() {
    let Deployment { state, .. } = deployment(None);
    let (status, config) = call(&state, "GET", "/api/v1/client-config", WRITE_TOKEN, None).await;
    assert_eq!(status, StatusCode::OK, "{config}");
    assert_eq!(config["chat_provider_name"], "Discord and Slack");
    let providers = config["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0]["key"], "discord");
    assert_eq!(providers[0]["threading_supported"], false);
    assert_eq!(providers[0]["live_delivery"], "push");
    assert_eq!(providers[1]["key"], "slack");
    assert_eq!(providers[1]["name"], "Slack");
    assert_eq!(providers[1]["threading_supported"], true);
    assert_eq!(providers[1]["channel_registration_supported"], true);
    assert_eq!(providers[1]["live_delivery"], "poll");
    assert_eq!(providers[1]["live_poll_seconds"], 30);
    assert_eq!(providers[1]["self_author_id"], "U0SELF0001");
    let owner = |id: &str| {
        config["channels"]
            .as_array()
            .expect("channels")
            .iter()
            .find(|channel| channel["id"] == id)
            .map(|channel| channel["provider"].clone())
    };
    assert_eq!(owner(SLACK_CHANNEL), Some(json!("slack")));
    assert_eq!(owner(READ_CHANNEL), Some(json!("discord")));
}

#[tokio::test]
async fn reads_and_posts_reach_only_the_owning_provider() {
    let Deployment {
        state,
        discord,
        slack,
        ..
    } = deployment(None);
    slack.seed(&ChannelId(SLACK_CHANNEL.to_owned()), "grace", "from slack");
    discord.seed(&ChannelId(READ_CHANNEL.to_owned()), "ada", "from discord");

    let (status, page) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{SLACK_CHANNEL}/messages"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.to_string().contains("from slack"));
    assert!(!page.to_string().contains("from discord"));

    let (status, posted) = call(
        &state,
        "POST",
        &format!("/api/v1/channels/{SLACK_CHANNEL}/reply"),
        WRITE_TOKEN,
        Some(json!({"text": "hello slack"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{posted}");
    assert_eq!(slack.posted().len(), 1);
    assert!(discord.posted().is_empty());
}

#[tokio::test]
async fn pushed_events_are_refused_for_a_channel_whose_provider_is_polled() {
    let Deployment { state, .. } = deployment(None);
    let event = |channel: &str| {
        json!({
            "event_id": format!("evt-{channel}"),
            "historical": false,
            "kind": "create",
            "message": {
                "id": "1500000000000000000",
                "channel_id": channel,
                "author": "adapter",
                "author_id": "42",
                "author_is_bot": false,
                "timestamp": "2026-10-02T10:00:00+00:00",
                "reply_to": null,
                "content": "pushed",
            },
        })
    };
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/live/events",
        INGEST_TOKEN,
        Some(event(SLACK_CHANNEL)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["error"], "channel_is_polled");
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/live/events",
        INGEST_TOKEN,
        Some(event(READ_CHANNEL)),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
}

#[tokio::test]
async fn a_channel_added_by_link_stays_with_its_provider_across_a_restart() {
    let first = deployment(None);
    let added = ChannelId(ADDED_SLACK_CHANNEL.to_owned());
    first.slack.enable_channel_registration(&added, false, true);
    let (status, body) = call(
        &first.state,
        "POST",
        "/api/v1/channels",
        WRITE_TOKEN,
        Some(json!({
            "source": "https://acme.slack.com/archives/C0999ZZZZZ",
            "label": "added slack",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["channel"]["provider"], "slack");
    assert_eq!(body["channel"]["writable"], true);
    assert_eq!(first.slack.registration_calls().len(), 1);

    // A new process over the same store: the row's namespace says which provider owns it.
    let second = deployment(Some(first.store.clone()));
    second.slack.register_channel(&added);
    second.slack.seed(&added, "grace", "after restart");
    second
        .state
        .restore_added_channels()
        .await
        .expect("restore");
    let restored = second.state.channel(ADDED_SLACK_CHANNEL).expect("restored");
    assert_eq!(restored.provider.as_deref(), Some("slack"));
    assert!(restored.writable);
    let (status, page) = call(
        &second.state,
        "GET",
        &format!("/api/v1/channels/{ADDED_SLACK_CHANNEL}/messages"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page.to_string().contains("after restart"));

    // Removal goes back to the provider that registered it, and the channel stops routing.
    second
        .slack
        .enable_channel_registration(&added, false, true);
    let (status, body) = call(
        &second.state,
        "DELETE",
        &format!("/api/v1/channels/{ADDED_SLACK_CHANNEL}"),
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(second.slack.unregistration_calls(), std::slice::from_ref(&added));
    assert!(second.state.providers.key_for(&added).is_none());
    assert!(second.discord.unregistration_calls().is_empty());
}
