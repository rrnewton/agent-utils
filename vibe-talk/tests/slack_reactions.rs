//! `#312 slack-reactions`: the reactions Slack reports on each message reach the page.
//!
//! `conversations.history` and `conversations.replies` list each message's reactions as
//! `reactions: [{name, count, users}]`. Every read here carries them as the provider-neutral
//! tallies the page draws: a standard name as its emoji, any other as a custom emoji's name, the
//! count, and whether the owner (`UOWNER001`, the fake's configured `owner_user_id`) is among the
//! users. Checked on a thread root, a reply and an edited message, through history, Main, Flat, a
//! thread, the newest messages, the live poller, a forward read, and the HTTP API; and an absent
//! field stays "not reported", so a bridge that cannot supply reactions reads exactly as before.

mod slack_fake;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use slack_fake::{add_reply, profiled, thread, ts, Fake, FakeState, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::ChatClient;
use vibe_talk::live::{poll_once, LiveHub};
use vibe_talk::model::{ChannelId, Message, Reaction};
use vibe_talk::providers::ChatRouter;
use vibe_talk::slack::{ts_from_message_id, HttpSlackClient, SlackConfig};
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::{TimelineRequest, TimelineView};
use vibe_talk::timeline_forward::{generic_since, position_of};

const CH: &str = "C0000MAIN1";
const OWNER: &str = "UOWNER001";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// One Slack reaction entry, as `conversations.history` lists it.
fn slack_reaction(name: &str, users: &[&str]) -> Value {
    json!({"name": name, "count": users.len(), "users": users})
}

/// The message timestamps of the fixture.
struct Stamps {
    /// A thread root with reactions, at 30 s.
    root: String,
    /// Its first reply, with reactions.
    reacted_reply: String,
    /// Its second reply, which nobody reacted to: Slack sends no `reactions` field.
    plain_reply: String,
    /// A message edited after it was reacted to, at 40 s.
    edited: String,
    /// A message nobody reacted to, at 50 s.
    plain: String,
}

/// Six messages ten seconds apart; the one at 30 s roots a thread of two replies, and the one at
/// 40 s was edited. Reactions on the root, the first reply and the edited message.
fn fixture() -> (FakeState, Stamps) {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..6)
        .map(|i| profiled(&ts(i * 10, 0), "UADA00001", "Ada", &format!("message {i}")))
        .collect();
    let stamps = Stamps {
        root: ts(30, 0),
        reacted_reply: ts(31, 0),
        plain_reply: ts(32, 0),
        edited: ts(40, 0),
        plain: ts(50, 0),
    };
    thread(
        &mut state,
        CH,
        &mut list[3],
        &[
            (stamps.reacted_reply.clone(), "first reply".to_owned()),
            (stamps.plain_reply.clone(), "second reply".to_owned()),
        ],
    );
    list[3]["reactions"] = json!([
        slack_reaction("eyes", &["UADA00001", OWNER]),
        slack_reaction("white_check_mark", &["UBOT00001"]),
    ]);
    list[4]["text"] = json!("message 4, with the typo fixed");
    list[4]["edited"] = json!({"user": "UADA00001", "ts": ts(45, 0)});
    list[4]["reactions"] = json!([slack_reaction("+1::skin-tone-3", &[OWNER])]);
    state
        .replies
        .get_mut(&(CH.to_owned(), stamps.root.clone()))
        .expect("the thread's replies")[0]["reactions"] = json!([
        slack_reaction("party-parrot", &["UADA00001", "UGRACE001"]),
        slack_reaction("tada", &[OWNER]),
    ]);
    state.history.insert(CH.to_owned(), list);
    (state, stamps)
}

/// The tallies of the message at `stamp` among `messages`: `None` when its copy says nothing.
fn tallies<'m>(messages: &'m [Message], stamp: &str) -> Option<&'m [Reaction]> {
    messages
        .iter()
        .find(|message| ts_from_message_id(&message.id).as_deref() == Some(stamp))
        .unwrap_or_else(|| panic!("no message at {stamp}"))
        .reactions
        .as_deref()
}

/// A message's tallies as `(emoji, custom, count, owner_reacted)`.
fn shown(reactions: &[Reaction]) -> Vec<(&str, bool, u32, bool)> {
    reactions
        .iter()
        .map(|r| (r.emoji.as_str(), r.custom, r.count, r.owner_reacted))
        .collect()
}

fn root_tallies() -> Vec<(&'static str, bool, u32, bool)> {
    vec![("👀", false, 2, true), ("✅", false, 1, false)]
}

fn reply_tallies() -> Vec<(&'static str, bool, u32, bool)> {
    vec![("party-parrot", true, 2, false), ("🎉", false, 1, true)]
}

fn edited_tallies() -> Vec<(&'static str, bool, u32, bool)> {
    vec![("👍🏼", false, 1, true)]
}

/// The conversation's own messages: the root, the edited message, and one without reactions.
fn assert_own_messages(messages: &[Message], stamps: &Stamps, read: &str) {
    assert_eq!(
        tallies(messages, &stamps.root).map(shown),
        Some(root_tallies()),
        "{read}: the root"
    );
    assert_eq!(
        tallies(messages, &stamps.edited).map(shown),
        Some(edited_tallies()),
        "{read}: the edited message"
    );
    assert_eq!(
        tallies(messages, &stamps.plain),
        None,
        "{read}: a message Slack lists without reactions is not reported, not known to have none"
    );
}

/// The thread's replies: one with reactions, one without.
fn assert_replies(messages: &[Message], stamps: &Stamps, read: &str) {
    assert_eq!(
        tallies(messages, &stamps.reacted_reply).map(shown),
        Some(reply_tallies()),
        "{read}: the reply"
    );
    assert_eq!(
        tallies(messages, &stamps.plain_reply),
        None,
        "{read}: the reply nobody reacted to"
    );
}

fn view(view: TimelineView, thread_id: Option<&str>) -> TimelineRequest {
    TimelineRequest {
        view,
        thread_id: thread_id.map(str::to_owned),
        before: None,
        after: None,
        limit: 50,
    }
}

#[tokio::test]
async fn history_main_flat_and_thread_reads_carry_slacks_reactions() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();

    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("a page of history");
    assert_own_messages(&page, &stamps, "history");
    let edited = page
        .iter()
        .find(|m| ts_from_message_id(&m.id).as_deref() == Some(stamps.edited.as_str()))
        .expect("the edited message");
    assert_eq!(
        edited.content, "message 4, with the typo fixed",
        "the edit is read with its reactions"
    );

    let main = client
        .fetch_timeline(&channel(), &view(TimelineView::Main, None))
        .await
        .expect("Main");
    assert_own_messages(&main.messages, &stamps, "Main");

    let flat = client
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert_own_messages(&flat.messages, &stamps, "Flat");
    assert_replies(&flat.messages, &stamps, "Flat");

    let thread = client
        .fetch_timeline(
            &channel(),
            &view(TimelineView::Thread, Some(stamps.root.as_str())),
        )
        .await
        .expect("the thread");
    assert_eq!(
        tallies(&thread.messages, &stamps.root).map(shown),
        Some(root_tallies()),
        "the thread's root"
    );
    assert_replies(&thread.messages, &stamps, "the thread");

    // The newest messages, which the voice tools read, bring the replies with their reactions too.
    let recent = client
        .fetch_recent(&channel(), 50)
        .await
        .expect("the newest messages");
    assert_own_messages(&recent, &stamps, "the newest messages");
    assert_replies(&recent, &stamps, "the newest messages");
}

#[tokio::test]
async fn the_live_poller_delivers_new_messages_and_replies_with_their_reactions() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let hub = LiveHub::new();
    let mut receiver = hub.subscribe(&channel(), None).receiver;
    let mut cursor = None;
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("the seeding poll");
    assert!(receiver.try_recv().is_err(), "seeding publishes nothing");

    // A reaction lands within seconds of a message, so a poll can find both at once.
    let fresh = ts(60, 0);
    let reply = ts(61, 0);
    fake.with(|state| {
        let mut message = profiled(&fresh, "UGRACE001", "Grace", "deploying now");
        message["reactions"] = json!([slack_reaction("rocket", &["UADA00001", OWNER])]);
        state.history.get_mut(CH).expect("channel").push(message);
        add_reply(state, CH, &stamps.root, &reply, "a later reply");
        state
            .replies
            .get_mut(&(CH.to_owned(), stamps.root.clone()))
            .expect("the thread's replies")
            .last_mut()
            .expect("the new reply")["reactions"] = json!([slack_reaction("eyes", &["UBOT00001"])]);
    });
    poll_once(&client, &hub, &channel(), 50, &mut cursor)
        .await
        .expect("a poll");
    let mut published = Vec::new();
    while let Ok(live) = receiver.try_recv() {
        published.push(live.message);
    }
    assert_eq!(published.len(), 2, "{published:?}");
    assert_eq!(
        tallies(&published, &fresh).map(shown),
        Some(vec![("🚀", false, 2, true)]),
        "a new message of the conversation's own"
    );
    assert_eq!(
        tallies(&published, &reply).map(shown),
        Some(vec![("👀", false, 1, false)]),
        "a new reply in a thread"
    );
}

#[tokio::test]
async fn a_reaction_added_later_brings_the_message_back_in_a_forward_read() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let main = view(TimelineView::Main, None);
    let newest = client
        .fetch_timeline(&channel(), &main)
        .await
        .expect("Main");
    let position = position_of(&newest, TimelineView::Main);

    let quiet = generic_since(&client, &channel(), &main, &position)
        .await
        .expect("a forward read");
    assert!(quiet.page.messages.is_empty(), "{:?}", quiet.page.messages);

    // Somebody adds ✅ to the message nobody had reacted to, and the owner adds 👀 to the root.
    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("channel");
        for message in list.iter_mut() {
            if message["ts"] == json!(stamps.plain) {
                message["reactions"] = json!([slack_reaction("white_check_mark", &["UBOT00001"])]);
            }
            if message["ts"] == json!(stamps.root) {
                message["reactions"] = json!([
                    slack_reaction("eyes", &["UADA00001", OWNER, "UGRACE001"]),
                    slack_reaction("white_check_mark", &["UBOT00001"]),
                ]);
            }
        }
    });
    let delta = generic_since(&client, &channel(), &main, &quiet.position)
        .await
        .expect("a forward read");
    let mut changed: Vec<String> = delta
        .page
        .messages
        .iter()
        .filter_map(|m| ts_from_message_id(&m.id))
        .collect();
    changed.sort();
    assert_eq!(changed, [stamps.root.clone(), stamps.plain.clone()]);
    assert_eq!(
        tallies(&delta.page.messages, &stamps.plain).map(shown),
        Some(vec![("✅", false, 1, false)])
    );
    assert_eq!(
        tallies(&delta.page.messages, &stamps.root).map(shown),
        Some(vec![("👀", false, 3, true), ("✅", false, 1, false)])
    );
}

#[tokio::test]
async fn without_an_owner_id_no_reaction_is_the_owners() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = HttpSlackClient::new(&SlackConfig {
        owner_user_id: None,
        ..fake.config()
    })
    .expect("client");
    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("a page of history");
    assert_eq!(
        tallies(&page, &stamps.root).map(shown),
        Some(vec![("👀", false, 2, false), ("✅", false, 1, false)]),
        "the owner's own account cannot be recognised without its id"
    );
}

#[tokio::test]
async fn a_bridge_that_cannot_supply_reactions_reads_as_before() {
    let (mut state, stamps) = fixture();
    for message in state
        .history
        .values_mut()
        .chain(state.replies.values_mut())
        .flatten()
    {
        message
            .as_object_mut()
            .expect("an object")
            .remove("reactions");
    }
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let flat = client
        .fetch_timeline(&channel(), &view(TimelineView::Flat, None))
        .await
        .expect("Flat");
    assert!(
        flat.messages.iter().all(|m| m.reactions.is_none()),
        "{:?}",
        flat.messages
    );
    let thread = client
        .fetch_timeline(
            &channel(),
            &view(TimelineView::Thread, Some(stamps.root.as_str())),
        )
        .await
        .expect("the thread");
    assert!(thread.messages.iter().all(|m| m.reactions.is_none()));
}

/// The page's own read, through a deployment assembled from configuration as `main` assembles one,
/// with `owner_user_id` set.
#[tokio::test]
async fn the_page_reads_slack_reactions_through_the_api() {
    let (state, stamps) = fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
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
owner_user_id = "{OWNER}"

[[channels]]
id = "{CH}"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
        base = fake.base,
    );
    let (mut app, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    app.replace_providers(Arc::new(
        ChatRouter::from_config(&app.config).expect("providers build"),
    ));
    let read = |uri: String| {
        let app = app.clone();
        async move {
            let response = vibe_talk::http::router(app)
                .oneshot(
                    Request::builder()
                        .uri(uri)
                        .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("router responds");
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .expect("body")
                .to_bytes();
            let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            assert_eq!(status, StatusCode::OK, "{body}");
            body
        }
    };
    let flat = read(format!("/api/v1/channels/{CH}/timeline?view=flat")).await;
    let wire = |stamp: &str| -> Value {
        let id = vibe_talk::slack::message_id_from_ts(stamp).expect("encodable");
        flat["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .find(|message| message["id"] == json!(id.0))
            .unwrap_or_else(|| panic!("no message at {stamp} in {flat}"))
            .get("reactions")
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        wire(&stamps.root),
        json!([
            {"emoji": "👀", "count": 2, "owner_reacted": true},
            {"emoji": "✅", "count": 1},
        ])
    );
    assert_eq!(
        wire(&stamps.reacted_reply),
        json!([
            {"emoji": "party-parrot", "custom": true, "count": 2},
            {"emoji": "🎉", "count": 1, "owner_reacted": true},
        ])
    );
    assert_eq!(
        wire(&stamps.edited),
        json!([{"emoji": "👍🏼", "count": 1, "owner_reacted": true}])
    );
    assert_eq!(
        wire(&stamps.plain),
        Value::Null,
        "not reported is absent on the wire"
    );
}
