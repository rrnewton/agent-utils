//! `#549 own-read-stored` (after `#536 mark-own-messages-read`): with "Mark my own messages as read"
//! on, the owner's own messages START read, in the one stored read bit, and from then on are toggled
//! like anyone's.
//!
//! The owner, 2026-10-26: "I think it was simpler to just do what the setting says which is to mark
//! them as read IE record the state that they are read and then I can toggle one individually ...
//! it's just a question of whether the state starts as read or starts as unread when the message is
//! ingested". Checked against the bridge-shaped fake the Google Chat instance reads through (the owner
//! named by `owner_user_id`) and against a loopback Slack fake shaped as the Slack instance is (the
//! owner named only by `[owner] user_ids`, the coding agents by `coding_agent_user_ids`), through the
//! HTTP routes a page reads, and through the live hub a pushed message arrives by.

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

/// The channel's messages a window read says are Done, as the page is told.
async fn window_done(state: &AppState, channel: &str, token: &str) -> BTreeSet<String> {
    let body = get(
        state,
        &format!("/api/v1/channels/{channel}/messages"),
        token,
    )
    .await;
    body["dismissed"]
        .as_array()
        .unwrap_or_else(|| panic!("no dismissed in {body}"))
        .iter()
        .map(|id| id.as_str().expect("id").to_owned())
        .collect()
}

/// Put messages back, as the page's Unarchive and Undo do.
async fn put_back(state: &AppState, channel: &str, ids: &[&MessageId]) {
    let messages: Vec<&str> = ids.iter().map(|id| id.0.as_str()).collect();
    let (status, body) = call(
        state,
        "POST",
        &format!("/api/v1/channels/{channel}/restore"),
        WRITE_TOKEN,
        Some(json!({ "messages": messages })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// The same server, restarted: the store and the chat service are what they were, and everything
/// held in memory — the settings read, the live hub's memory of what it published and what this
/// server posted — is gone.
fn restarted(state: &AppState) -> AppState {
    let mut fresh = state.clone();
    fresh.own_read = Arc::new(vibe_talk::own_read::OwnRead::default());
    fresh.live = Arc::new(vibe_talk::live::LiveHub::new());
    fresh
}

#[tokio::test]
async fn a_read_that_first_meets_his_messages_records_them_done() {
    let b = bridge();
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
    // Recorded, as Done is: the one read bit, not a judgement made again on every read.
    assert_eq!(held(&b.store, WRITE_CHANNEL).await, set_of(&b.owner));
    // And every surface reads that bit: a window read tells the page so.
    assert_eq!(
        window_done(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.owner)
    );
}

#[tokio::test]
async fn a_read_scope_read_answers_the_same_and_writes_nothing() {
    let b = bridge();
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, READ_TOKEN).await,
        set_of(&b.agent)
    );
    assert_eq!(
        window_done(&b.state, WRITE_CHANNEL, READ_TOKEN).await,
        set_of(&b.owner)
    );
    assert!(
        held(&b.store, WRITE_CHANNEL).await.is_empty(),
        "a read-scope read wrote a mark"
    );
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    assert!(b
        .store
        .own_decided(&channel, &b.owner)
        .await
        .expect("decided")
        .is_empty());
}

#[tokio::test]
async fn one_of_his_put_back_stays_unread_across_rereads_a_restart_and_a_re_ingest() {
    let b = bridge();
    todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await;
    // He puts one of his back, as Unarchive does: an ordinary read bit, toggled.
    put_back(&b.state, WRITE_CHANNEL, &[&b.owner[0]]).await;
    let waiting: BTreeSet<String> = set_of(b.agent.iter().chain([&b.owner[0]]));
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, waiting);
    // Read again, for the page and the to-do list: nothing marks it again.
    window_done(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await;
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, waiting);
    // Restarted: the decision is in the store, not in memory.
    let after = restarted(&b.state);
    assert_eq!(todo(&after, WRITE_CHANNEL, WRITE_TOKEN).await, waiting);
    // And the live hub, which has forgotten what it published, publishes it again — the poller's
    // first walk after a restart, or an adapter re-delivering it: still not marked again.
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let copy = b.discord.message(&b.owner[0]).expect("held");
    after.live.publish(&channel, copy.clone());
    assert!(after.live.publish_ingested(
        "redelivered".to_owned(),
        vibe_talk::live::LiveKind::Create,
        false,
        &channel,
        copy,
    ));
    vibe_talk::own_read::decide_waiting(&after).await;
    assert_eq!(todo(&after, WRITE_CHANNEL, WRITE_TOKEN).await, waiting);
    assert!(!held(&b.store, WRITE_CHANNEL).await.contains(&b.owner[0].0));
}

#[tokio::test]
async fn a_message_of_his_the_live_hub_publishes_is_done_before_any_read() {
    let b = bridge();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let pushed = b.discord.seed(&channel, "ada", "typed in the chat app");
    let message = b.discord.message(&pushed).expect("held");
    b.state.live.publish(&channel, message);
    vibe_talk::own_read::decide_waiting(&b.state).await;
    assert_eq!(
        held(&b.store, WRITE_CHANNEL).await,
        set_of([&pushed]),
        "the push did not record his message read"
    );
    // The agent's, pushed the same way, waits.
    let answer = b.discord.seed(&channel, "codex-bot", "an answer");
    b.state
        .live
        .publish(&channel, b.discord.message(&answer).expect("held"));
    vibe_talk::own_read::decide_waiting(&b.state).await;
    assert!(!held(&b.store, WRITE_CHANNEL).await.contains(&answer.0));
}

#[tokio::test]
async fn the_setting_decides_only_messages_first_seen_after_it_changes() {
    let b = bridge();
    // Seen while on: his start read.
    todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await;
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "0")]).await;
    // Turned off, they stay read: the setting is where a message starts, not a rule over it.
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
    // One first seen while off starts unread...
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let later = b.discord.seed(&channel, "ada", "said while it was off");
    assert!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN)
        .await
        .contains(&later.0));
    // ...and turning it on again does not reach back to it.
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "1")]).await;
    assert!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN)
        .await
        .contains(&later.0));
}

#[tokio::test]
async fn apply_to_existing_marks_his_undone_messages_once_and_the_undo_puts_back_that_set() {
    let b = bridge();
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "0")]).await;
    let everything: BTreeSet<String> = set_of(b.owner.iter().chain(&b.agent));
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, everything);
    let (status, body) = call(
        &b.state,
        "POST",
        "/api/v1/own-read/apply",
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], json!(2), "{body}");
    let changes = body["changes"].as_array().expect("changes");
    assert_eq!(changes.len(), 1, "{body}");
    assert_eq!(changes[0]["channel"]["id"], json!(WRITE_CHANNEL), "{body}");
    let marked: Vec<String> = changes[0]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|id| id.as_str().expect("id").to_owned())
        .collect();
    assert_eq!(
        marked,
        b.owner.iter().map(|id| id.0.clone()).collect::<Vec<_>>()
    );
    assert_eq!(
        todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await,
        set_of(&b.agent)
    );
    // Nothing left to apply.
    let (_, again) = call(
        &b.state,
        "POST",
        "/api/v1/own-read/apply",
        WRITE_TOKEN,
        None,
    )
    .await;
    assert_eq!(again["count"], json!(0), "{again}");
    // The undo puts back exactly what it marked.
    put_back(&b.state, WRITE_CHANNEL, &b.owner.iter().collect::<Vec<_>>()).await;
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, everything);
    // A read-scope caller cannot apply it.
    let (status, _) = call(&b.state, "POST", "/api/v1/own-read/apply", READ_TOKEN, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
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
async fn done_through_here_clears_what_was_waiting_his_put_back_included() {
    let b = bridge();
    todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await;
    put_back(&b.state, WRITE_CHANNEL, &[&b.owner[1]]).await;
    let (status, body) = call(
        &b.state,
        "POST",
        &format!("/api/v1/channels/{WRITE_CHANNEL}/dismiss"),
        WRITE_TOKEN,
        Some(json!({"through": b.agent[1].0})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    // What it cleared is what was waiting: the agent's, and the one of his he put back. The other of
    // his started read, and is not in the set an undo would put back.
    let cleared: BTreeSet<String> = body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|id| id.as_str().expect("id").to_owned())
        .collect();
    assert_eq!(
        cleared,
        set_of(b.agent.iter().chain([&b.owner[1]])),
        "{body}"
    );
    assert_eq!(
        held(&b.store, WRITE_CHANNEL).await,
        set_of(b.owner.iter().chain(&b.agent))
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
    // The setting went with the purge, so the shipped default stands again: on. And so did the
    // record of which of his messages were decided, so each is first seen again.
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
    // His question and his reply in its thread, which the to-do list's window holds too.
    assert_eq!(
        held(&store, CH).await,
        BTreeSet::from([id_of(&stamps.question), id_of(&stamps.follow_up)])
    );
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
    // The thread's read told the page his reply is Done, and the app's answer is not.
    let done: Vec<&str> = thread["dismissed"]
        .as_array()
        .expect("dismissed")
        .iter()
        .filter_map(|id| id.as_str())
        .collect();
    assert!(
        done.contains(&id_of(&stamps.follow_up).as_str()),
        "{thread}"
    );
    assert!(!done.contains(&id_of(&stamps.answer).as_str()), "{thread}");
    let left = todo(&app, CH, WRITE_TOKEN).await;
    for his in [&stamps.question, &stamps.follow_up] {
        assert!(
            !left.contains(&id_of(his)),
            "{his} is still to do: {left:?}"
        );
    }
    assert_eq!(
        held(&store, CH).await,
        BTreeSet::from([id_of(&stamps.question), id_of(&stamps.follow_up)])
    );
}

#[tokio::test]
async fn a_settings_read_that_fails_decides_nothing_and_the_next_sight_decides() {
    // The review of #549 own-read-stored: a decision is written once, so it must never be made from
    // the default stood in for a failed read — on, nobody named — when he had turned it off.
    let b = bridge();
    set(&b.state, &[("vibe-talk.voice.mark-own-read", "0")]).await;
    b.state.own_read.forget();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let messages: Vec<_> = b
        .owner
        .iter()
        .map(|id| b.discord.message(id).expect("held"))
        .collect();
    b.store.fail_next("the store is busy");
    let done =
        vibe_talk::own_read::settle(&b.state, vibe_talk::auth::Scope::Write, &channel, &messages)
            .await;
    assert!(done.is_empty(), "{done:?}");
    assert!(b
        .store
        .own_decided(&channel, &b.owner)
        .await
        .expect("decided")
        .is_empty());
    // The next sight reads the setting as it is, off, and decides them unread.
    let everything: BTreeSet<String> = set_of(b.owner.iter().chain(&b.agent));
    assert_eq!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN).await, everything);
    assert_eq!(
        b.store
            .own_decided(&channel, &b.owner)
            .await
            .expect("decided")
            .len(),
        2
    );
}

#[tokio::test]
async fn one_of_his_put_back_before_its_live_decision_landed_stays_put_back() {
    // Drawn read on the page as it arrives live, put back at once: the decision the hub queued must
    // not land after the put back.
    let b = bridge();
    let channel = ChannelId(WRITE_CHANNEL.to_owned());
    let pushed = b.discord.seed(&channel, "ada", "typed in the chat app");
    b.state
        .live
        .publish(&channel, b.discord.message(&pushed).expect("held"));
    put_back(&b.state, WRITE_CHANNEL, &[&pushed]).await;
    vibe_talk::own_read::decide_waiting(&b.state).await;
    assert!(todo(&b.state, WRITE_CHANNEL, WRITE_TOKEN)
        .await
        .contains(&pushed.0));
}
