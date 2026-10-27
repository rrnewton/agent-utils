//! `#408 copy-upstream-link`: a message's own address in the service it came from, as each
//! provider forms it and as a served read carries it.
//!
//! The pure shapes are proven in `crate::upstream_url`'s own tests. Here: a Slack client's
//! workspace — configured, or learned from `auth.test` — a Discord-protocol client's guild and
//! its Google Chat shapes, and one served timeline read carrying the links the page copies.

use crate::slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use slack_fake::{plain, thread, ts, Fake, FakeState};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::config::{DiscordConfig, Secret};
use vibe_talk::discord::http::HttpDiscordClient;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::slack::{message_id_from_ts, HttpSlackClient, SlackConfig};
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::MessageThread;

const CH: &str = "C0000MAIN1";

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

fn mid(ts: &str) -> MessageId {
    message_id_from_ts(ts).expect("encodable ts")
}

fn slack_thread(root_ts: &str, is_root: bool) -> MessageThread {
    MessageThread {
        hue: None,
        display_name: None,
        summary: None,
        id: root_ts.to_owned(),
        root_message_id: Some(mid(root_ts)),
        is_root,
        reply_count: None,
        reply_count_exact: false,
    }
}

/// A root at `ts(0, 100)` with one reply, and the two `ts` values.
fn threaded_state() -> (FakeState, String, String) {
    let mut state = FakeState::default();
    let root_ts = ts(0, 100);
    let reply_ts = ts(60, 200);
    let mut root = plain(&root_ts, "UADA00001", "root words");
    thread(
        &mut state,
        CH,
        &mut root,
        &[(reply_ts.clone(), "reply words".to_owned())],
    );
    state.history.insert(CH.to_owned(), vec![root]);
    (state, root_ts, reply_ts)
}

fn permalink(workspace: &str, ts: &str) -> String {
    format!("{workspace}/archives/{CH}/p{}", ts.replace('.', ""))
}

#[tokio::test]
async fn a_slack_clients_links_come_from_its_configured_workspace() {
    let (state, root_ts, reply_ts) = threaded_state();
    let fake = Fake::start(state, 20).await;
    let client = HttpSlackClient::new(&SlackConfig {
        workspace_url: Some("https://configured.slack.com/".to_owned()),
        ..fake.config()
    })
    .expect("client");
    // No identity call has run: the configured workspace is enough, and a root links plainly.
    assert_eq!(
        client.upstream_url(
            &channel(),
            &mid(&root_ts),
            Some(&slack_thread(&root_ts, true))
        ),
        Some(permalink("https://configured.slack.com", &root_ts))
    );
    // A reply links into its thread.
    assert_eq!(
        client.upstream_url(
            &channel(),
            &mid(&reply_ts),
            Some(&slack_thread(&root_ts, false))
        ),
        Some(format!(
            "{}?thread_ts={root_ts}&cid={CH}",
            permalink("https://configured.slack.com", &reply_ts)
        ))
    );
    // A channel narrowed to the thread names the same conversation before its `~`.
    let scoped = ChannelId(format!("{CH}~{root_ts}"));
    assert_eq!(
        client.upstream_url(&scoped, &mid(&reply_ts), None),
        Some(format!(
            "{}?thread_ts={root_ts}&cid={CH}",
            permalink("https://configured.slack.com", &reply_ts)
        ))
    );
}

#[tokio::test]
async fn a_slack_client_learns_its_workspace_from_auth_test() {
    let (state, root_ts, _) = threaded_state();
    let fake = Fake::start(state, 20).await;
    let client = fake.client();
    // Nothing configured and nothing learned yet: no link is formed.
    assert_eq!(
        client.upstream_url(
            &channel(),
            &mid(&root_ts),
            Some(&slack_thread(&root_ts, true))
        ),
        None
    );
    client.identity().await.expect("identity");
    // The fake's `auth.test` reports `https://example.slack.com/`, as Slack's does.
    assert_eq!(
        client.upstream_url(
            &channel(),
            &mid(&root_ts),
            Some(&slack_thread(&root_ts, true))
        ),
        Some(permalink("https://example.slack.com", &root_ts))
    );
    // Parts that cannot be read form none.
    assert_eq!(
        client.upstream_url(&ChannelId("general".to_owned()), &mid(&root_ts), None),
        None
    );
    assert_eq!(
        client.upstream_url(&channel(), &MessageId("1500".to_owned()), None),
        None,
        "an id no Slack ts decodes from: its low bits are no microsecond remainder"
    );
}

fn discord_config(guild_id: Option<String>) -> DiscordConfig {
    DiscordConfig {
        provider_name: "Discord".to_owned(),
        thread_api: vibe_talk::threads::ThreadApi::Native,
        bot_token: Secret::new("abc"),
        owner_user_id: None,
        guild_id,
        api_base: "https://discord.com/api/v10".to_owned(),
        request_timeout_seconds: 20,
        channel_registration: false,
        upstream_read_marks: false,
        markup: vibe_talk::render::Markup::Discord,
        max_post_chars: None,
        max_post_bytes: None,
        recent: vibe_talk::config::RecentReads::default(),
        range_reads: false,
        id_reads: false,
        head_reads: false,
        recent_many_reads: false,
        thread_reads: false,
        gateway_reactions: false,
    }
}

#[test]
fn a_discord_clients_links_come_from_its_configured_guild() {
    let guild = "900000000000000001";
    let chan = ChannelId("900000000000000002".to_owned());
    let id = MessageId("900000000000000003".to_owned());
    let client = HttpDiscordClient::new(&discord_config(Some(guild.to_owned()))).expect("client");
    assert_eq!(
        client.upstream_url(&chan, &id, None),
        Some(format!(
            "https://discord.com/channels/{guild}/900000000000000002/900000000000000003"
        ))
    );
    // No guild configured and none learned: no link, rather than a guessed one.
    let guildless = HttpDiscordClient::new(&discord_config(None)).expect("client");
    assert_eq!(guildless.upstream_url(&chan, &id, None), None);
    // A thread reply links by its THREAD channel, as Discord's own copy-link writes it: a
    // native thread read serves the reply under its parent channel's id, and a link naming
    // the parent finds no such message there.
    let thread = MessageThread {
        hue: None,
        display_name: None,
        summary: None,
        id: "900000000000000004".to_owned(),
        root_message_id: Some(MessageId("900000000000000004".to_owned())),
        is_root: false,
        reply_count: None,
        reply_count_exact: false,
    };
    assert_eq!(
        client.upstream_url(&chan, &id, Some(&thread)),
        Some(format!(
            "https://discord.com/channels/{guild}/900000000000000004/900000000000000003"
        ))
    );
    // Its root, a message of the parent channel itself, links by the parent.
    let root = MessageThread {
        hue: None,
        display_name: None,
        summary: None,
        is_root: true,
        ..thread
    };
    assert_eq!(
        client.upstream_url(&chan, &id, Some(&root)),
        Some(format!(
            "https://discord.com/channels/{guild}/900000000000000002/900000000000000003"
        ))
    );
    // Against a bridge — any API base that is not Discord itself — no discord.com address is
    // formed from numeric ids, guild configured or not: they are the bridge's numbering.
    let mut bridge = discord_config(Some(guild.to_owned()));
    bridge.api_base = "http://127.0.0.1:9/api/v10".to_owned();
    let bridged = HttpDiscordClient::new(&bridge).expect("client");
    assert_eq!(bridged.upstream_url(&chan, &id, None), None);
}

#[test]
fn a_google_chat_bridges_links_come_from_its_resource_names() {
    // The same Discord-protocol client, fronting Google Chat: the ids are Chat resource
    // names, and the shapes cannot be snowflakes, so no guild setting is involved.
    let client = HttpDiscordClient::new(&discord_config(None)).expect("client");
    let chan = ChannelId("spaces/AAAa1".to_owned());
    let thread = MessageThread {
        hue: None,
        display_name: None,
        summary: None,
        id: "spaces/AAAa1/threads/BBBb2".to_owned(),
        root_message_id: None,
        is_root: false,
        reply_count: None,
        reply_count_exact: false,
    };
    assert_eq!(
        client.upstream_url(
            &chan,
            &MessageId("spaces/AAAa1/messages/BBBb2.CCCc3".to_owned()),
            Some(&thread)
        ),
        Some("https://chat.google.com/room/AAAa1/BBBb2/CCCc3".to_owned())
    );
    // The bridge's own message numbering, in a known thread: the thread's link.
    assert_eq!(
        client.upstream_url(
            &chan,
            &MessageId("900000000000000003".to_owned()),
            Some(&thread)
        ),
        Some("https://chat.google.com/room/AAAa1/BBBb2".to_owned())
    );
}

async fn call(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
        .body(Body::empty())
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
async fn a_served_read_carries_each_messages_link() {
    let (state, root_ts, reply_ts) = threaded_state();
    let fake = Fake::start(state, 20).await;
    let slack = HttpSlackClient::new(&SlackConfig {
        workspace_url: Some("https://configured.slack.com".to_owned()),
        ..fake.config()
    })
    .expect("client");
    let text = format!(
        "[[providers]]\nkey = \"slack\"\nkind = \"slack\"\ntoken = \"xoxb-test\"\n\n\
         [[channels]]\nid = \"{CH}\"\nlabel = \"slack team\"\nwritable = true\nprovider = \"slack\"\n\n{}",
        vibe_talk::testing::config_toml()
    );
    let (mut state, discord, _eleven) = vibe_talk::testing::state_from_toml(&text);
    let entries = vec![
        ProviderEntry {
            key: "discord".to_owned(),
            namespace: state.config.providers[0].namespace(),
            client: Arc::clone(&discord) as Arc<dyn ChatClient>,
            live_poll_seconds: 0,
        },
        ProviderEntry {
            key: "slack".to_owned(),
            namespace: state.config.providers[1].namespace(),
            client: Arc::new(slack),
            live_poll_seconds: 0,
        },
    ];
    let router = ChatRouter::new(entries, state.config.default_provider_key());
    state.replace_providers(Arc::new(router));
    let (status, page) = call(&state, &format!("/api/v1/channels/{CH}/timeline?view=flat")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let messages = page["messages"].as_array().expect("messages");
    let link = |wanted: &str| {
        messages
            .iter()
            .find(|message| message["content"] == wanted)
            .map(|message| message["upstream_url"].clone())
    };
    assert_eq!(
        link("root words"),
        Some(Value::String(permalink(
            "https://configured.slack.com",
            &root_ts
        )))
    );
    assert_eq!(
        link("reply words"),
        Some(Value::String(format!(
            "{}?thread_ts={root_ts}&cid={CH}",
            permalink("https://configured.slack.com", &reply_ts)
        )))
    );
}
