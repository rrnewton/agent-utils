//! `#536 mark-own-messages-read`: with "Mark my own messages as read" on, the owner's own messages
//! are read to this server too, not only greyed by one page.
//!
//! The owner, 2026-10-26: "They are rendering as MY messages... but they are not marked read." The
//! setting was a rule of the page alone: the server's to-do list still counted them, "Done through
//! here" wrote marks for them, and an account only `[owner] user_ids` named was not even drawn as his.
//! Checked against the bridge-shaped fake the Google Chat instance reads through (the owner named by
//! `owner_user_id`) and against a loopback Slack fake shaped as the Slack instance is (the owner named
//! only by `[owner] user_ids`, the coding agents by `coding_agent_user_ids`), through the HTTP routes a
//! page reads. Like noise, the rule is judged on every read and nothing is written per message.

mod slack_fake;

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{plain, thread, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::StateStore as _;
use vibe_talk::testing::{self, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN};

/// The fake bridge's first author: the owner, by the configuration's `owner_user_id`.
const GCHAT_OWNER: &str = "2000000000000000001";

/// One request through the whole router, answering the status and the JSON body.
async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    let body = match body {
        Some(value) => {
            request = request.header("content-type", "application/json");
            Body::from(value.to_string())
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

/// A read that must succeed.
async fn get(state: &AppState, uri: &str, token: &str) -> Value {
    let (status, body) = call(state, "GET", uri, token, None).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

/// Set synced settings as a page does.
async fn set(state: &AppState, entries: &[(&str, &str)]) {
    let set: Vec<Value> = entries
        .iter()
        .map(|(key, value)| json!({"key": key, "value": value, "updated_ms": 0}))
        .collect();
    let (status, body) = call(
        state,
        "PUT",
        "/api/v1/client-settings",
        WRITE_TOKEN,
        Some(json!({"set": set, "seed": []})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The ids of a to-do answer's messages.
async fn todo(state: &AppState, channel: &str, token: &str) -> BTreeSet<String> {
    let body = get(state, &format!("/api/v1/channels/{channel}/todo"), token).await;
    body["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("no messages in {body}"))
        .iter()
        .map(|message| message["id"].as_str().expect("id").to_owned())
        .collect()
}

fn set_of<'a>(list: impl IntoIterator<Item = &'a MessageId>) -> BTreeSet<String> {
    list.into_iter().map(|id| id.0.clone()).collect()
}

async fn held(store: &FakeStore, channel: &str) -> BTreeSet<String> {
    store
        .dismissals(&ChannelId(channel.to_owned()))
        .await
        .expect("dismissals")
        .into_iter()
        .map(|id| id.0)
        .collect()
}

// --- the Google Chat instance's shape: a bridge, and the owner's `owner_user_id` ---------------

struct Bridge {
    state: AppState,
    store: Arc<FakeStore>,
    discord: Arc<vibe_talk::discord::fake::FakeDiscord>,
    owner: Vec<MessageId>,
    agent: Vec<MessageId>,
}

/// Two messages of the owner's and two of the coding agent's, taking turns.
fn bridge() -> Bridge {
    let text = testing::config_toml().replace(
        "[discord]\n",
        &format!("[discord]\nowner_user_id = \"{GCHAT_OWNER}\"\n"),
    );
    let (state, discord, store) = testing::state_with_store_from_toml(&text);
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let mut owner = Vec::new();
    let mut agent = Vec::new();
    for n in 0..2 {
        owner.push(discord.seed(&channel, "ada", &format!("question {n}")));
        agent.push(discord.seed(&channel, "codex-bot", &format!("answer {n}")));
    }
    Bridge {
        state,
        store,
        discord,
        owner,
        agent,
    }
}

#[tokio::test]
async fn the_to_do_list_leaves_his_own_messages_out_and_writes_nothing() {
    let b = bridge();
    // For either token: this is a judgement on the read, not a write.
    for token in [WRITE_TOKEN, READ_TOKEN] {
        assert_eq!(todo(&b.state, WRITE_CHANNEL, token).await, set_of(&b.agent));
    }
    assert!(
        held(&b.store, WRITE_CHANNEL).await.is_empty(),
        "a mark was written for a message nobody marked"
    );
}

#[tokio::test]
async fn turned_off_they_are_back_at_once_and_on_again_they_go() {
    let b = bridge();
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "0")]).await;
    let everything: BTreeSet<String> = set_of(b.owner.iter().chain(&b.agent));
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, everything);
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "1")]).await;
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
}

#[tokio::test]
async fn who_is_who_decides_whose_messages_are_his() {
    let b = bridge();
    let agent_id = b
        .discord
        .message(&b.agent[0])
        .expect("the agent's message is held")
        .author_id
        .0;
    // The reader called the configured owner a person and the agent's account himself: the choice
    // he made in Settings stands over the configuration, as it does on the page.
    let choice = json!({ GCHAT_OWNER: "human", agent_id: "me" }).to_string();
    set(&b.state, &[("vibe-talk.voice.identities", &choice)]).await;
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.owner)
    );
}

#[tokio::test]
async fn a_message_this_server_posted_is_his() {
    let b = bridge();
    // An account nobody configured, posting through this server: the owner's dictation.
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let dictated = b.discord.seed(&channel, "vibe-talk", "sent from the page");
    assert!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN)
        .await
        .contains(&dictated.0));
    b.state.live.note_self_posted(&dictated, None);
    assert!(!todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN)
        .await
        .contains(&dictated.0));
}

#[tokio::test]
async fn done_through_here_writes_no_mark_for_his_messages() {
    let b = bridge();
    let (status, body) = call(
        &b.state,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/dismiss"),
        WRITE_TOKEN,
        Some(json!({"through": b.agent[1].0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // Only the agent's: a mark for one of his would outlast the setting.
    assert_eq!(
        held(&b.store, WRITE_CHANNEL).await,
        set_of(&b.agent),
        "{body}"
    );
}

#[tokio::test]
async fn the_page_is_told_every_configured_account_of_his() {
    let b = bridge();
    let config = get(&b.state, "/api/v1/client-config", READ_TOKEN).await;
    assert_eq!(config["owner_author_ids"], json!([GCHAT_OWNER]), "{config}");
}

#[tokio::test]
async fn a_purge_forgets_the_choice_with_the_settings() {
    let b = bridge();
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "0")]).await;
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await.len(), 4);
    let (status, body) = call(&b.state, "DELETE", "/api/v1/storage", WRITE_TOKEN, None).await;
    assert!(status.is_success(), "{status}: {body}");
    // The setting went with the purge, so the shipped default stands again: on.
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
}

// --- the Slack instance's shape: `[owner] user_ids`, and the agents' shared Slack app ----------

const CH: &str = "C0000MAIN1";
const OWNER: &str = "UOWNER777";
const APP: &str = "UAPP00001";

/// The owner asks in the channel, the agents' app answers in a thread, and he answers back there.
async fn slack() -> (Fake, AppState, Arc<FakeStore>, Stamps) {
    let mut state = FakeState::default();
    let stamps = Stamps {
        question: ts(10, 0),
        answer: ts(11, 0),
        follow_up: ts(12, 0),
        heads_up: ts(20, 0),
        self_post: ts(30, 0),
    };
    let mut list = vec![
        plain(&stamps.question, OWNER, "Why is the gate red?"),
        plain(&stamps.heads_up, APP, "Heads-up: the host is loaded."),
        // The account this server posts as, which the configuration names as a coding agent too.
        plain(&stamps.self_post, slack_fake::SELF_USER, "[agent] status"),
    ];
    thread(
        &mut state,
        CH,
        &mut list[0],
        &[(stamps.answer.clone(), "A flaky step.".to_owned())],
    );
    state.history.insert(CH.to_owned(), list);
    slack_fake::add_reply_message(
        &mut state,
        CH,
        &stamps.question,
        plain(&stamps.follow_up, OWNER, "Rerun it alone."),
    );
    let fake = Fake::start(state, 20).await;
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"
coding_agent_user_ids = ["{APP}", "{self_user}"]

[[channels]]
id = "{CH}"
label = "team"
writable = true

[owner]
name = "Ada Lovelace"
user_ids = ["{OWNER}"]
"#,
        base = fake.base,
        self_user = slack_fake::SELF_USER,
    );
    let (mut app, _discord, store) = testing::state_with_store_from_toml(&text);
    app.replace_providers(Arc::new(
        ChatRouter::from_config(&app.config).expect("providers build"),
    ));
    (fake, app, store, stamps)
}

struct Stamps {
    question: String,
    answer: String,
    follow_up: String,
    heads_up: String,
    self_post: String,
}

fn id_of(stamp: &str) -> String {
    vibe_talk::slack::message_id_from_ts(stamp)
        .expect("encodable")
        .0
}

#[tokio::test]
async fn slack_his_messages_are_read_and_the_apps_are_not() {
    let (_fake, app, store, stamps) = slack().await;
    let left = todo(&app, CH, WRITE_TOKEN).await;
    assert!(!left.contains(&id_of(&stamps.question)), "{left:?}");
    // The app's heads-up, and the server's own account where it is named as a coding agent, wait.
    for stamp in [&stamps.heads_up, &stamps.self_post] {
        assert!(
            left.contains(&id_of(stamp)),
            "{stamp} is missing from {left:?}"
        );
    }
    assert!(held(&store, CH).await.is_empty());
    // The page is told his account, which only `[owner] user_ids` names.
    let config = get(&app, "/api/v1/client-config", READ_TOKEN).await;
    assert_eq!(config["owner_author_ids"], json!([OWNER]), "{config}");
}

#[tokio::test]
async fn slack_a_thread_mixing_his_messages_and_the_apps_leaves_only_the_apps_to_do() {
    // The owner, 2026-10-26: his messages' Done icons said unread while the thread card counted none.
    // The page's side is pinned in its own suite; here, the to-do list over the same shape of thread.
    let (_fake, app, store, stamps) = slack().await;
    let thread = get(
        &app,
        &format!(
            "/api/v1/channels/{CH}/timeline?view=thread&thread_id={}",
            stamps.question
        ),
        WRITE_TOKEN,
    )
    .await;
    let ids: Vec<&str> = thread["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["id"].as_str())
        .collect();
    assert!(ids.contains(&id_of(&stamps.follow_up).as_str()), "{thread}");
    let left = todo(&app, CH, WRITE_TOKEN).await;
    for his in [&stamps.question, &stamps.follow_up] {
        assert!(
            !left.contains(&id_of(his)),
            "{his} is still to do: {left:?}"
        );
    }
    assert!(held(&store, CH).await.is_empty());
}
