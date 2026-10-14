//! `slack-reply-live`: a reply posted in a Slack thread reaches the live stream, and the recent
//! window the voice tools read, within one poll.
//!
//! `conversations.history` lists no thread reply, so a poller that only walks it forward never
//! delivered one. Each poll now reads the newest page of history once, which also answers the
//! forward read, and reads the new replies of each thread whose root's `latest_reply` moved: one
//! `conversations.replies` call per moved thread, `oldest` at the newest reply already delivered,
//! at most four threads a poll, the rest carried to the next. Counted here at a loopback fake that
//! answers 20 rows a call, as the bridge in front of Slack does.

mod slack_fake;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use slack_fake::{add_reply, profiled, thread, ts, Fake, FakeState, TOKEN};
use vibe_talk::auth::Scope;
use vibe_talk::live::{poll_once, LiveHub, LiveMessage, Tick};
use vibe_talk::mcp::protocol::{dispatch, Outcome};
use vibe_talk::model::ChannelId;
use vibe_talk::providers::ChatRouter;
use vibe_talk::slack::{message_id_from_ts, ts_from_message_id};
use vibe_talk::state::AppState;
use vibe_talk::testing::WRITE_TOKEN;
use vibe_talk::threads::MessageThread;

const CH: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;
/// The page size the poller asks for.
const LIMIT: u16 = 50;

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

/// Ten messages ten seconds apart; message 3 roots a thread of two replies.
fn one_thread() -> (FakeState, String) {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..10)
        .map(|i| profiled(&ts(i * 10, 0), "UADA00001", "Ada", &format!("message {i}")))
        .collect();
    let root = ts(30, 0);
    thread(
        &mut state,
        CH,
        &mut list[3],
        &[
            (ts(31, 0), "first reply".to_owned()),
            (ts(32, 0), "second reply".to_owned()),
        ],
    );
    state.history.insert(CH.to_owned(), list);
    (state, root)
}

/// Twelve messages; messages 0, 2, 4, 6, 8 and 10 each root a thread of one reply. Returns the
/// roots' `ts`, oldest first.
fn six_threads() -> (FakeState, Vec<String>) {
    let mut state = FakeState::default();
    let mut list: Vec<Value> = (0..12)
        .map(|i| profiled(&ts(i * 10, 0), "UADA00001", "Ada", &format!("message {i}")))
        .collect();
    let mut roots = Vec::new();
    for i in (0..12).step_by(2) {
        let root = ts(i64::try_from(i).expect("small") * 10, 0);
        let reply = (
            ts(i64::try_from(i).expect("small") * 10 + 1, 0),
            format!("old reply to {i}"),
        );
        thread(&mut state, CH, &mut list[i], &[reply]);
        roots.push(root);
    }
    state.history.insert(CH.to_owned(), list);
    (state, roots)
}

/// What one poll published, and the Web API calls it made.
struct Poll {
    tick: Tick,
    published: Vec<LiveMessage>,
    history: usize,
    /// Every `conversations.replies` request the poll sent, in order.
    replies: Vec<HashMap<String, String>>,
}

impl Poll {
    fn texts(&self) -> Vec<&str> {
        self.published
            .iter()
            .map(|live| live.message.content.as_str())
            .collect()
    }
}

/// One poller over one channel, with its subscription.
struct Poller {
    hub: LiveHub,
    receiver: tokio::sync::broadcast::Receiver<LiveMessage>,
    cursor: Option<u64>,
}

impl Poller {
    fn new() -> Self {
        let hub = LiveHub::new();
        let receiver = hub.subscribe(&channel(), None).receiver;
        Self {
            hub,
            receiver,
            cursor: None,
        }
    }

    async fn poll(&mut self, fake: &Fake, client: &dyn vibe_talk::chat::ChatClient) -> Poll {
        let history = fake.calls("conversations.history");
        let replies = fake.requests("conversations.replies").len();
        let tick = poll_once(client, &self.hub, &channel(), LIMIT, &mut self.cursor)
            .await
            .expect("a poll");
        let mut published = Vec::new();
        while let Ok(live) = self.receiver.try_recv() {
            published.push(live);
        }
        Poll {
            tick,
            published,
            history: fake.calls("conversations.history") - history,
            replies: fake.requests("conversations.replies")[replies..].to_vec(),
        }
    }
}

#[tokio::test]
async fn a_reply_in_an_existing_thread_reaches_the_live_stream_within_one_poll_for_one_call() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();

    let seed = poller.poll(&fake, &client).await;
    assert!(
        seed.published.is_empty(),
        "the first poll seeds and publishes nothing"
    );
    assert!(seed.replies.is_empty(), "seeding reads no thread");

    fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "a new reply"));
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(
        poll.texts(),
        ["a new reply"],
        "and none of the replies from before the seed"
    );
    assert_eq!(poll.tick.published, 1);
    assert!(!poll.tick.replies_behind);
    assert_eq!(
        poll.history, 1,
        "one read of the newest page of history answers the forward read too"
    );
    assert_eq!(poll.replies.len(), 1, "exactly one replies call");
    assert_eq!(poll.replies[0]["ts"], root);
    assert_eq!(
        poll.replies[0]["oldest"],
        ts(32, 0),
        "read only after the newest reply already held"
    );
    let reply = &poll.published[0];
    assert!(!reply.self_posted);
    assert_eq!(reply.message.channel_id, channel());
    assert_eq!(
        ts_from_message_id(&reply.message.id).as_deref(),
        Some(ts(100, 0).as_str())
    );
    assert_eq!(
        reply.message.thread,
        Some(MessageThread {
            id: root.clone(),
            root_message_id: message_id_from_ts(&root),
            is_root: false,
            reply_count: None,
            reply_count_exact: false,
        }),
        "a reply arrives as a message event with its thread"
    );

    let quiet = poller.poll(&fake, &client).await;
    assert!(quiet.published.is_empty(), "each reply is published once");
    assert!(quiet.replies.is_empty(), "its root has not moved since");
}

#[tokio::test]
async fn an_unchanged_root_costs_no_replies_call() {
    let (state, _roots) = six_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    for _ in 0..3 {
        let poll = poller.poll(&fake, &client).await;
        assert!(poll.published.is_empty());
        assert_eq!(poll.history, 1, "one history call a poll, as before");
        assert!(
            poll.replies.is_empty(),
            "six threads whose roots did not move cost nothing: {:?}",
            poll.replies
        );
    }

    fake.with(|state| {
        state.history.get_mut(CH).expect("channel").push(profiled(
            &ts(500, 0),
            "UGRACE001",
            "Grace",
            "a new top-level message",
        ));
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["a new top-level message"]);
    assert_eq!(poll.history, 1);
    assert!(poll.replies.is_empty());
}

#[tokio::test]
async fn a_burst_over_the_cap_carries_over_to_the_next_poll() {
    let (state, roots) = six_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    // A reply in each of six threads in the same interval: more than one poll reads.
    fake.with(|state| {
        for (n, root) in roots.iter().enumerate() {
            let n = i64::try_from(n).expect("small");
            add_reply(state, CH, root, &ts(200 + n, 0), &format!("burst {n}"));
        }
    });
    let first = poller.poll(&fake, &client).await;
    assert_eq!(
        first.replies.len(),
        vibe_talk::slack::LIVE_THREAD_READS,
        "the cap bounds the calls of one poll"
    );
    assert_eq!(first.texts(), ["burst 0", "burst 1", "burst 2", "burst 3"]);
    assert!(
        first.tick.replies_behind,
        "the rest are carried, and the tick says so"
    );

    // Then enough of the channel's own messages to push every root off the newest page: the
    // threads left waiting are read all the same.
    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("channel");
        for i in 0..25 {
            list.push(profiled(
                &ts(300 + i, 0),
                "UGRACE001",
                "Grace",
                &format!("later {i}"),
            ));
        }
    });
    let second = poller.poll(&fake, &client).await;
    assert_eq!(
        second
            .replies
            .iter()
            .map(|r| r["ts"].clone())
            .collect::<Vec<_>>(),
        roots[4..],
        "the two carried threads, and no other"
    );
    let texts = second.texts();
    assert_eq!(texts.len(), 27, "{texts:?}");
    assert_eq!(
        &texts[..2],
        ["burst 4", "burst 5"],
        "in time order among the rest"
    );
    assert!(!second.tick.replies_behind);

    let third = poller.poll(&fake, &client).await;
    assert!(third.published.is_empty());
    assert!(third.replies.is_empty());
}

#[tokio::test]
async fn a_thread_with_more_new_replies_than_one_call_returns_carries_over() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        for i in 0..25 {
            add_reply(state, CH, &root, &ts(100 + i, 0), &format!("flood {i}"));
        }
    });
    let first = poller.poll(&fake, &client).await;
    assert_eq!(first.replies.len(), 1, "one call a thread a poll");
    assert_eq!(first.published.len(), PAGE_CAP, "what that call returned");
    assert!(
        first.tick.replies_behind,
        "a short page is never taken for the end of the thread"
    );
    let second = poller.poll(&fake, &client).await;
    assert_eq!(second.replies.len(), 1);
    assert_eq!(
        second.replies[0]["oldest"],
        ts(100 + 19, 0),
        "continued after the newest reply delivered"
    );
    assert_eq!(second.published.len(), 5);
    assert!(!second.tick.replies_behind);
    let all: Vec<String> = first
        .published
        .iter()
        .chain(&second.published)
        .map(|live| live.message.content.clone())
        .collect();
    let expected: Vec<String> = (0..25).map(|i| format!("flood {i}")).collect();
    assert_eq!(all, expected, "every reply once, in order");
}

#[tokio::test]
async fn the_first_reply_to_an_older_message_reaches_the_live_stream() {
    let (state, _root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    // Message 5 had no thread when the poller started.
    let older = ts(50, 0);
    fake.with(|state| {
        add_reply(
            state,
            CH,
            &older,
            &ts(120, 0),
            "first words in a new thread",
        )
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["first words in a new thread"]);
    assert_eq!(poll.replies.len(), 1);
    assert_eq!(poll.replies[0]["ts"], older);
}

#[tokio::test]
async fn a_reply_also_sent_to_the_channel_is_published_once() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        add_reply(state, CH, &root, &ts(100, 0), "sent to the channel too");
        let replies = state
            .replies
            .get_mut(&(CH.to_owned(), root.clone()))
            .expect("thread");
        let broadcast = replies.last_mut().expect("the reply");
        broadcast["subtype"] = json!("thread_broadcast");
        let copy = broadcast.clone();
        state.history.get_mut(CH).expect("channel").push(copy);
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["sent to the channel too"]);
}

/// A Slack deployment of one channel, whose messages are minutes old, so a catch-up of the last
/// day sees them.
mod voice {
    use super::*;

    const TEAM: &str = "C0000MAIN1";

    /// A canonical `ts` this many seconds before now.
    pub fn ago(seconds: i64) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs();
        format!("{}.000000", i64::try_from(now).expect("now") - seconds)
    }

    pub fn fixture() -> (FakeState, String) {
        let mut state = FakeState::default();
        let root = ago(600);
        let mut list = vec![
            profiled(&ago(900), "UADA00001", "Ada", "good morning"),
            profiled(&root, "UADA00001", "Ada", "deploy started"),
            profiled(&ago(300), "UGRACE001", "Grace", "lunch at noon"),
        ];
        thread(
            &mut state,
            TEAM,
            &mut list[1],
            &[(ago(590), "build is green".to_owned())],
        );
        state.history.insert(TEAM.to_owned(), list);
        (state, root)
    }

    pub fn deployment(fake: &Fake) -> AppState {
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
owner_user_id = "UOWNER001"

[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
            read = vibe_talk::testing::READ_TOKEN,
            base = fake.base,
        );
        let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
        let router = ChatRouter::from_config(&state.config).expect("providers build");
        state.replace_providers(Arc::new(router));
        state
    }

    /// The `recent_activity` tool's answer, as the voice agent reads it.
    pub async fn catch_up(state: &AppState) -> String {
        let call = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "recent_activity", "arguments": {} },
        });
        let Outcome::Reply(response) = dispatch(state, Scope::Read, &call).await else {
            panic!("expected a reply");
        };
        let result = response.result.as_ref().expect("a result");
        assert_eq!(result["isError"], false, "{result}");
        result["content"][0]["text"]
            .as_str()
            .expect("text")
            .to_owned()
    }

    pub fn held_texts(state: &AppState) -> Vec<String> {
        state
            .live
            .recent()
            .snapshot(&ChannelId(TEAM.to_owned()))
            .held
            .expect("a held window")
            .messages
            .into_iter()
            .map(|message| message.content)
            .collect()
    }
}

#[tokio::test]
async fn the_recent_window_the_voice_tools_read_includes_the_reply() {
    let (state, root) = voice::fixture();
    let fake = Fake::start(state, PAGE_CAP).await;
    let server = voice::deployment(&fake);

    let before = voice::catch_up(&server).await;
    assert!(
        before.contains("build is green"),
        "a recent read carries thread replies too: {before}"
    );
    assert_eq!(
        voice::held_texts(&server),
        [
            "good morning",
            "deploy started",
            "build is green",
            "lunch at noon"
        ]
    );

    // The poller, through the provider's own client as `main` spawns it.
    let client = Arc::clone(&server.providers.entry("slack").expect("slack").client);
    let mut cursor = None;
    poll_once(
        client.as_ref(),
        &server.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("seed");
    fake.with(|state| add_reply(state, CH, &root, &voice::ago(5), "rollout finished"));
    let tick = poll_once(
        client.as_ref(),
        &server.live,
        &channel(),
        LIMIT,
        &mut cursor,
    )
    .await
    .expect("a poll");
    assert_eq!(tick.published, 1);

    assert_eq!(
        voice::held_texts(&server).last().map(String::as_str),
        Some("rollout finished"),
        "the live reply is merged into the held window"
    );
    let after = voice::catch_up(&server).await;
    assert!(after.contains("rollout finished"), "{after}");

    // A read of the channel afresh keeps it: a server with nothing held reads it from Slack.
    let fresh = voice::deployment(&fake);
    let read = voice::catch_up(&fresh).await;
    assert!(read.contains("rollout finished"), "{read}");
    assert!(read.contains("build is green"), "{read}");
}
