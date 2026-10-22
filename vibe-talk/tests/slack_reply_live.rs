//! `#307 slack-reply-live`: a reply posted in a Slack thread reaches the live stream, and the
//! recent window the voice tools read, within one poll.
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
use vibe_talk::slack::{
    message_id_from_ts, ts_from_message_id, LIVE_FAILED_READS, LIVE_LAG_READS,
    LIVE_TRANSIENT_FAILED_READS,
};
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
            hue: None,
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

/// Slack answers a failure on its side as HTTP 200 with `ok: false` and a code such as
/// `internal_error`, and a bridge in front of it answers a command that failed or timed out the
/// same way. The client cannot tell such an answer from a refusal, so a thread whose replies read
/// fails so is read again on the next poll, from where the failed read started.
#[tokio::test]
async fn a_replies_read_that_fails_once_leaves_its_replies_for_the_next_poll() {
    for code in [
        "internal_error",
        "fatal_error",
        "request_timeout",
        "service_unavailable",
    ] {
        let (state, root) = one_thread();
        let fake = Fake::start(state, PAGE_CAP).await;
        let client = fake.client();
        let mut poller = Poller::new();
        poller.poll(&fake, &client).await;

        fake.with(|state| {
            add_reply(state, CH, &root, &ts(100, 0), "a new reply");
            state
                .fail
                .insert("conversations.replies".to_owned(), code.to_owned());
        });
        let failed = poller.poll(&fake, &client).await;
        assert!(failed.published.is_empty(), "{code}");
        assert_eq!(failed.replies.len(), 1, "{code}: the read was made");
        assert!(
            failed.tick.replies_behind,
            "{code}: the thread waits for the next poll, and the tick says so"
        );

        fake.with(|state| state.fail.remove("conversations.replies"));
        let next = poller.poll(&fake, &client).await;
        assert_eq!(next.texts(), ["a new reply"], "{code}");
        assert_eq!(next.replies.len(), 1, "{code}");
        assert_eq!(
            next.replies[0]["oldest"],
            ts(32, 0),
            "{code}: read again from where the failed read started"
        );
        assert!(!next.tick.replies_behind, "{code}");

        let quiet = poller.poll(&fake, &client).await;
        assert!(quiet.published.is_empty(), "{code}: published once");
        assert!(quiet.replies.is_empty(), "{code}: and read once");
    }
}

/// A thread whose reads keep failing, other than for a moment, is given up after
/// `LIVE_FAILED_READS` failed reads in a row, so a refusal costs a bounded number of calls; a read
/// that succeeds in between starts the count again.
#[tokio::test]
async fn a_thread_whose_replies_reads_keep_failing_is_given_up_after_a_bounded_run() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;
    let failing = |on: bool| {
        fake.with(|state| {
            if on {
                state.fail.insert(
                    "conversations.replies".to_owned(),
                    "invalid_arguments".to_owned(),
                );
            } else {
                state.fail.remove("conversations.replies");
            }
        });
    };

    // One failed read short of the bound, then a read that succeeds: delivered.
    fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "first"));
    failing(true);
    for n in 1..LIVE_FAILED_READS {
        let poll = poller.poll(&fake, &client).await;
        assert_eq!(poll.replies.len(), 1, "read {n} is made");
        assert!(poll.published.is_empty());
        assert!(
            poll.tick.replies_behind,
            "read {n} leaves the thread waiting"
        );
    }
    failing(false);
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["first"]);
    assert!(!poll.tick.replies_behind);

    // The count started again: it takes the whole bound to give the thread up.
    fake.with(|state| add_reply(state, CH, &root, &ts(110, 0), "missed by the live stream"));
    failing(true);
    for n in 1..=LIVE_FAILED_READS {
        let poll = poller.poll(&fake, &client).await;
        assert_eq!(poll.replies.len(), 1, "read {n} is made");
        assert!(poll.published.is_empty());
        assert_eq!(
            poll.tick.replies_behind,
            n < LIVE_FAILED_READS,
            "read {n}: waiting until the last"
        );
    }
    let after = poller.poll(&fake, &client).await;
    assert!(
        after.replies.is_empty(),
        "a thread given up costs no further call"
    );
    assert!(!after.tick.replies_behind);

    // A later reply in it reaches the stream once reads succeed again, after what was given up.
    failing(false);
    fake.with(|state| add_reply(state, CH, &root, &ts(120, 0), "later"));
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["later"]);
    assert_eq!(poll.replies.len(), 1);
    assert_eq!(poll.replies[0]["oldest"], ts(110, 0));
}

/// A `404` on a replies read (`thread_not_found`, or another code the client reads as one) does
/// not prove the thread gone: a bridge may answer so for a command that failed for another reason.
/// The thread keeps its place and is read again on the next poll from where the failed read
/// started, so a reply delivered before the `404` is not delivered again, and none after it is
/// lost.
#[tokio::test]
async fn a_replies_read_answered_404_once_neither_repeats_nor_loses_a_reply() {
    for code in ["thread_not_found", "channel_not_found", "message_not_found"] {
        let (state, root) = one_thread();
        let fake = Fake::start(state, PAGE_CAP).await;
        let client = fake.client();
        let mut poller = Poller::new();
        poller.poll(&fake, &client).await;
        fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "delivered"));
        let poll = poller.poll(&fake, &client).await;
        assert_eq!(poll.texts(), ["delivered"], "{code}");

        fake.with(|state| {
            add_reply(state, CH, &root, &ts(110, 0), "after the 404");
            state
                .fail
                .insert("conversations.replies".to_owned(), code.to_owned());
        });
        let failed = poller.poll(&fake, &client).await;
        assert!(failed.published.is_empty(), "{code}");
        assert_eq!(failed.replies.len(), 1, "{code}: the read was made");
        assert!(
            failed.tick.replies_behind,
            "{code}: the thread waits for the next poll, and the tick says so"
        );

        fake.with(|state| state.fail.remove("conversations.replies"));
        let next = poller.poll(&fake, &client).await;
        assert_eq!(
            next.texts(),
            ["after the 404"],
            "{code}: the reply after the 404 once, and the one before it not again"
        );
        assert_eq!(next.replies.len(), 1, "{code}");
        assert_eq!(
            next.replies[0]["oldest"],
            ts(100, 0),
            "{code}: read again from the reply last delivered"
        );
        assert!(!next.tick.replies_behind, "{code}");

        let quiet = poller.poll(&fake, &client).await;
        assert!(quiet.published.is_empty(), "{code}: published once");
        assert!(quiet.replies.is_empty(), "{code}: and read once");
    }
}

/// A thread whose replies reads keep answering `404` while its root is still on the newest page is
/// given up after `LIVE_FAILED_READS` reads in a row, as any failure that does not read as a moment
/// is, rather than read on every poll for as long as its root is listed. A later reply in it is
/// read from where the poller stood, so nothing delivered before goes out again.
#[tokio::test]
async fn a_thread_whose_replies_reads_keep_answering_404_is_given_up_after_a_bounded_run() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;
    fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "delivered"));
    assert_eq!(poller.poll(&fake, &client).await.texts(), ["delivered"]);

    fake.with(|state| {
        add_reply(state, CH, &root, &ts(110, 0), "missed by the live stream");
        state.fail.insert(
            "conversations.replies".to_owned(),
            "thread_not_found".to_owned(),
        );
    });
    let mut calls = Vec::new();
    for n in 1..=2 * LIVE_FAILED_READS {
        let poll = poller.poll(&fake, &client).await;
        assert!(poll.published.is_empty(), "poll {n} publishes nothing");
        assert_eq!(
            poll.tick.replies_behind,
            n < LIVE_FAILED_READS,
            "poll {n}: waiting until the last read"
        );
        calls.push(poll.replies.len());
    }
    let bound = usize::try_from(LIVE_FAILED_READS).expect("a small bound");
    let mut expected = vec![1; bound];
    expected.resize(2 * bound, 0);
    assert_eq!(
        calls, expected,
        "one replies call a poll up to the bound, then none while the root stays as it was"
    );

    fake.with(|state| {
        state.fail.remove("conversations.replies");
        add_reply(state, CH, &root, &ts(120, 0), "later");
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["later"]);
    assert_eq!(poll.replies.len(), 1);
    assert_eq!(poll.replies[0]["oldest"], ts(110, 0));
}

/// The roots, by `ts`, that a poll's replies calls read, in order.
fn read_roots(poll: &Poll) -> Vec<String> {
    poll.replies.iter().map(|r| r["ts"].clone()).collect()
}

/// A failure that reads as a moment (a 5xx, a 408) never counts towards `LIVE_FAILED_READS`, and a
/// bridge whose command fails on one thread's content answers so on every read of it. Such a
/// thread waits behind every thread whose last read did not fail, so the others are read, and
/// their replies published, while it keeps failing.
#[tokio::test]
async fn a_thread_whose_replies_reads_keep_failing_for_a_moment_holds_up_no_other_thread() {
    for status in [500, 502, 503, 408] {
        let (state, roots) = six_threads();
        let fake = Fake::start(state, PAGE_CAP).await;
        let client = fake.client();
        let mut poller = Poller::new();
        poller.poll(&fake, &client).await;

        // Two threads move in the same interval, and the first one's reads fail every time.
        fake.with(|state| {
            add_reply(state, CH, &roots[0], &ts(200, 0), "in the broken thread");
            add_reply(state, CH, &roots[3], &ts(205, 0), "in a healthy thread");
            state.fail_threads.insert(roots[0].clone(), status);
        });
        let first = poller.poll(&fake, &client).await;
        assert_eq!(
            read_roots(&first),
            [roots[0].clone()],
            "{status}: the read stops at the failure, so an outage costs one call a poll"
        );
        assert!(first.published.is_empty(), "{status}");
        assert!(first.tick.replies_behind, "{status}");

        let second = poller.poll(&fake, &client).await;
        assert_eq!(
            second.texts(),
            ["in a healthy thread"],
            "{status}: the thread behind the failing one is read"
        );
        assert_eq!(
            read_roots(&second),
            [roots[3].clone(), roots[0].clone()],
            "{status}: the failing thread is read after it"
        );
        assert!(second.tick.replies_behind, "{status}");

        // A thread that moves while the broken one waits is read first, in the poll after its
        // reply.
        fake.with(|state| add_reply(state, CH, &roots[4], &ts(210, 0), "later, elsewhere"));
        let third = poller.poll(&fake, &client).await;
        assert_eq!(third.texts(), ["later, elsewhere"], "{status}");
        assert_eq!(
            read_roots(&third),
            [roots[4].clone(), roots[0].clone()],
            "{status}"
        );

        // Once its reads succeed again, the broken thread's reply arrives, read from where the
        // poller stood in it.
        fake.with(|state| state.fail_threads.clear());
        let healed = poller.poll(&fake, &client).await;
        assert_eq!(healed.texts(), ["in the broken thread"], "{status}");
        assert_eq!(read_roots(&healed), [roots[0].clone()], "{status}");
        assert_eq!(healed.replies[0]["oldest"], ts(1, 0), "{status}");
        assert!(!healed.tick.replies_behind, "{status}");
    }
}

/// While every read of a thread fails for a moment, as during an outage of `conversations.replies`
/// alone, a poll costs one replies call, and the waiting threads take turns at it.
#[tokio::test]
async fn while_every_replies_read_fails_for_a_moment_the_waiting_threads_take_turns() {
    let (state, roots) = six_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        for (n, root) in roots.iter().enumerate().take(3) {
            let n = i64::try_from(n).expect("small");
            add_reply(state, CH, root, &ts(200 + n, 0), &format!("reply {n}"));
            state.fail_threads.insert(root.clone(), 502);
        }
    });
    let mut calls = Vec::new();
    for _ in 0..6 {
        let poll = poller.poll(&fake, &client).await;
        assert!(poll.published.is_empty());
        assert!(poll.tick.replies_behind);
        calls.push(read_roots(&poll));
    }
    let turns: Vec<Vec<String>> = roots[..3]
        .iter()
        .chain(&roots[..3])
        .map(|root| vec![root.clone()])
        .collect();
    assert_eq!(calls, turns, "one call a poll, each thread in turn");

    fake.with(|state| state.fail_threads.clear());
    let healed = poller.poll(&fake, &client).await;
    assert_eq!(healed.texts(), ["reply 0", "reply 1", "reply 2"]);
    assert!(!healed.tick.replies_behind);
}

/// A thread whose reads fail for a moment on every read is given up, with a warning, after
/// `LIVE_TRANSIENT_FAILED_READS` failed reads in a row, so it does not cost a call on every poll
/// with no end; a read that succeeds in between starts the count again.
#[tokio::test]
async fn a_thread_whose_replies_reads_keep_failing_for_a_moment_is_given_up_after_a_longer_run() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;
    let failing = |on: bool| {
        fake.with(|state| {
            if on {
                state.fail_threads.insert(root.clone(), 502);
            } else {
                state.fail_threads.clear();
            }
        });
    };

    // One failed read short of the bound, then a read that succeeds: delivered.
    fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "first"));
    failing(true);
    for n in 1..LIVE_TRANSIENT_FAILED_READS {
        let poll = poller.poll(&fake, &client).await;
        assert_eq!(poll.replies.len(), 1, "read {n} is made");
        assert!(poll.published.is_empty());
        assert!(
            poll.tick.replies_behind,
            "read {n} leaves the thread waiting"
        );
    }
    failing(false);
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["first"]);
    assert!(!poll.tick.replies_behind);

    // The count started again: it takes the whole bound to give the thread up.
    fake.with(|state| add_reply(state, CH, &root, &ts(110, 0), "missed by the live stream"));
    failing(true);
    let mut calls = Vec::new();
    for n in 1..=LIVE_TRANSIENT_FAILED_READS + 3 {
        let poll = poller.poll(&fake, &client).await;
        assert!(poll.published.is_empty(), "poll {n} publishes nothing");
        assert_eq!(
            poll.tick.replies_behind,
            n < LIVE_TRANSIENT_FAILED_READS,
            "poll {n}: waiting until the last read"
        );
        calls.push(poll.replies.len());
    }
    let bound = usize::try_from(LIVE_TRANSIENT_FAILED_READS).expect("a small bound");
    let mut expected = vec![1; bound];
    expected.resize(bound + 3, 0);
    assert_eq!(
        calls, expected,
        "one replies call a poll up to the bound, then none while the root stays as it was"
    );

    // A later reply in it reaches the stream once reads succeed again, after what was given up.
    failing(false);
    fake.with(|state| add_reply(state, CH, &root, &ts(120, 0), "later"));
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.texts(), ["later"]);
    assert_eq!(poll.replies.len(), 1);
    assert_eq!(poll.replies[0]["oldest"], ts(110, 0));
}

/// The calls a run of polls made, one count a poll, and whether each left replies waiting.
async fn run(
    poller: &mut Poller,
    fake: &Fake,
    client: &dyn vibe_talk::chat::ChatClient,
    polls: usize,
) -> (Vec<usize>, Vec<bool>) {
    let mut calls = Vec::new();
    let mut behind = Vec::new();
    for n in 1..=polls {
        let poll = poller.poll(fake, client).await;
        assert!(poll.published.is_empty(), "poll {n} publishes nothing");
        calls.push(poll.replies.len());
        behind.push(poll.tick.replies_behind);
    }
    (calls, behind)
}

/// `first` of `value`, then `rest` of `otherwise`.
fn then<T: Clone>(value: T, first: usize, otherwise: T, rest: usize) -> Vec<T> {
    let mut list = vec![value; first];
    list.resize(first + rest, otherwise);
    list
}

/// `LIVE_LAG_READS`, as a count of polls.
fn lag_reads() -> usize {
    usize::try_from(LIVE_LAG_READS).expect("a small bound")
}

/// A lenient server may ignore `oldest` on `conversations.replies`, answering every read of a
/// thread longer than one page with its first page and `has_more: true`. Nothing on that page is
/// new, so such a read counts towards `LIVE_LAG_READS` like any read that brings nothing, and the
/// thread is given up after that many in a row rather than read at the head of the queue on every
/// poll with no end, which four such threads would make of every poll.
#[tokio::test]
async fn a_server_that_ignores_oldest_cannot_hold_a_thread_at_the_head_for_good() {
    let (mut state, root) = one_thread();
    // Thirty more replies before the seed: the thread is longer than one page.
    for i in 0..30 {
        add_reply(&mut state, CH, &root, &ts(33, i), &format!("old {i}"));
    }
    state.replies_ignore_oldest = true;
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| add_reply(state, CH, &root, &ts(100, 0), "past the first page"));
    let lag = lag_reads();
    let (calls, behind) = run(&mut poller, &fake, &client, lag + 3).await;
    assert_eq!(
        calls,
        then(1, lag, 0, 3),
        "one call a poll up to the bound, then none while the root stays as it was"
    );
    assert_eq!(behind, then(true, lag - 1, false, 4));
}

/// A server that answers `conversations.replies` without `has_more` cuts a long burst of replies
/// into pages without saying so. A read that brings new replies is followed by another while the
/// root's latest reply has not come back, however many that takes, so the burst is read to its
/// end: only a read that brings nothing counts towards `LIVE_LAG_READS`.
#[tokio::test]
async fn a_long_burst_from_a_server_that_omits_has_more_is_read_to_its_end() {
    let (mut state, root) = one_thread();
    state.replies_omit_has_more = true;
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    // More pages of new replies than `LIVE_LAG_READS`.
    let burst = PAGE_CAP * (lag_reads() + 1) + PAGE_CAP / 2;
    fake.with(|state| {
        for i in 0..burst {
            let i = i64::try_from(i).expect("small");
            add_reply(state, CH, &root, &ts(100 + i, 0), &format!("burst {i}"));
        }
    });
    let mut all: Vec<String> = Vec::new();
    let mut polls = 0;
    loop {
        let poll = poller.poll(&fake, &client).await;
        polls += 1;
        assert_eq!(poll.replies.len(), 1, "one call a thread a poll");
        all.extend(poll.texts().into_iter().map(str::to_owned));
        if !poll.tick.replies_behind {
            break;
        }
        assert!(polls < 2 * burst.div_ceil(PAGE_CAP), "the burst never ends");
    }
    let expected: Vec<String> = (0..burst).map(|i| format!("burst {i}")).collect();
    assert_eq!(all, expected, "every reply once, in order");
    assert_eq!(polls, burst.div_ceil(PAGE_CAP), "one page a poll");
    let quiet = poller.poll(&fake, &client).await;
    assert!(quiet.replies.is_empty(), "read to its root's latest reply");
}

/// Make the root `root` count its replies but name no `latest_reply`.
fn forget_latest_reply(state: &mut FakeState, root: &str) {
    state
        .history
        .get_mut(CH)
        .expect("channel")
        .iter_mut()
        .find(|message| message["ts"] == json!(root))
        .expect("root")
        .as_object_mut()
        .expect("an object")
        .remove("latest_reply");
}

/// A root that counts its replies but names no `latest_reply` says nothing of where its thread
/// ends, so only Slack's `has_more` says a read was cut short: the thread is read again while it
/// says so, and taken as read once it does not.
#[tokio::test]
async fn a_thread_whose_root_names_no_latest_reply_is_read_on_while_slack_says_has_more() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        for i in 0..25 {
            add_reply(state, CH, &root, &ts(100 + i, 0), &format!("flood {i}"));
        }
        forget_latest_reply(state, &root);
    });
    let first = poller.poll(&fake, &client).await;
    assert_eq!(first.published.len(), PAGE_CAP, "what that call returned");
    assert!(first.tick.replies_behind, "Slack said has_more");
    let second = poller.poll(&fake, &client).await;
    assert_eq!(
        second.replies.len(),
        1,
        "read again while Slack says has_more"
    );
    assert_eq!(second.replies[0]["oldest"], ts(100 + 19, 0));
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
    let quiet = poller.poll(&fake, &client).await;
    assert!(quiet.replies.is_empty(), "its count has not changed since");
}

/// A read that succeeds starts a thread's run of failed reads again even when it leaves the thread
/// waiting for more, so a thread whose reads fail now and then, between reads that each bring part
/// of a long burst, is never given up for failures that were not in a row. Both bounds:
/// `LIVE_FAILED_READS` for a failure that does not read as a moment, and
/// `LIVE_TRANSIENT_FAILED_READS` for any.
#[tokio::test]
async fn a_read_that_succeeds_starts_the_run_of_failures_again_though_the_thread_still_waits() {
    for transient in [false, true] {
        let bound = if transient {
            LIVE_TRANSIENT_FAILED_READS
        } else {
            LIVE_FAILED_READS
        };
        let (state, root) = one_thread();
        let fake = Fake::start(state, PAGE_CAP).await;
        let client = fake.client();
        let mut poller = Poller::new();
        poller.poll(&fake, &client).await;
        let failing = |on: bool| {
            fake.with(|state| match (transient, on) {
                (true, true) => {
                    state.fail_threads.insert(root.clone(), 502);
                }
                (true, false) => state.fail_threads.clear(),
                (false, true) => {
                    state.fail.insert(
                        "conversations.replies".to_owned(),
                        "invalid_arguments".to_owned(),
                    );
                }
                (false, false) => {
                    state.fail.remove("conversations.replies");
                }
            });
        };

        fake.with(|state| {
            for i in 0..25 {
                add_reply(state, CH, &root, &ts(100 + i, 0), &format!("flood {i}"));
            }
        });
        // One failed read short of the bound...
        failing(true);
        for n in 1..bound {
            let poll = poller.poll(&fake, &client).await;
            assert!(poll.published.is_empty(), "transient {transient}, read {n}");
            assert!(poll.tick.replies_behind, "transient {transient}, read {n}");
        }
        // ...then a read that brings the first page, and leaves the thread waiting for the rest...
        failing(false);
        let partial = poller.poll(&fake, &client).await;
        assert_eq!(partial.published.len(), PAGE_CAP, "transient {transient}");
        assert!(partial.tick.replies_behind, "transient {transient}");
        // ...then one more failed read: the run started again, so the thread still waits.
        failing(true);
        let failed = poller.poll(&fake, &client).await;
        assert_eq!(failed.replies.len(), 1, "transient {transient}");
        assert!(
            failed.tick.replies_behind,
            "transient {transient}: given up for failures that were not in a row"
        );
        failing(false);
        let rest = poller.poll(&fake, &client).await;
        assert_eq!(rest.published.len(), 5, "transient {transient}");
        assert_eq!(
            rest.replies[0]["oldest"],
            ts(100 + 19, 0),
            "transient {transient}"
        );
        assert!(!rest.tick.replies_behind, "transient {transient}");
    }
}

/// While `conversations.replies` answers `ok: false` for every thread, as during an outage of it
/// alone, a poll stops at its first failed read: one replies call a poll, however many threads
/// wait.
#[tokio::test]
async fn while_every_replies_read_is_refused_a_poll_makes_one_replies_call() {
    let (state, roots) = six_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        for (n, root) in roots.iter().enumerate().take(3) {
            let n = i64::try_from(n).expect("small");
            add_reply(state, CH, root, &ts(200 + n, 0), &format!("reply {n}"));
        }
        state.fail.insert(
            "conversations.replies".to_owned(),
            "internal_error".to_owned(),
        );
    });
    let (calls, behind) = run(&mut poller, &fake, &client, 3).await;
    assert_eq!(calls, [1, 1, 1], "one call a poll");
    assert_eq!(behind, [true, true, true]);

    fake.with(|state| state.fail.remove("conversations.replies"));
    let healed = poller.poll(&fake, &client).await;
    assert_eq!(healed.texts(), ["reply 0", "reply 1", "reply 2"]);
    assert!(!healed.tick.replies_behind);
}

/// Take the newest reply out of the feed of the thread rooted on `root`, leaving its root as it
/// is: the root names a latest reply its feed does not list, as when the feed lags it.
fn hide_from_the_feed(state: &mut FakeState, root: &str) -> Value {
    state
        .replies
        .get_mut(&(CH.to_owned(), root.to_owned()))
        .expect("thread")
        .pop()
        .expect("a reply")
}

/// A root may name a latest reply its thread's feed does not list yet, as when the feed lags it.
/// A read that comes back without that reply leaves the thread waiting, and the reply is published
/// by the read after the feed lists it. A thread whose feed never does is given up after
/// `LIVE_LAG_READS` reads in a row that bring nothing new, taken as read to what its root said.
#[tokio::test]
async fn a_thread_whose_feed_lags_its_root_is_read_again_until_the_reply_arrives() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    let late = fake.with(|state| {
        add_reply(state, CH, &root, &ts(100, 0), "late in the feed");
        hide_from_the_feed(state, &root)
    });
    let lagging = poller.poll(&fake, &client).await;
    assert_eq!(lagging.replies.len(), 1);
    assert!(lagging.published.is_empty());
    assert!(
        lagging.tick.replies_behind,
        "the root's latest reply did not come back, so the thread waits"
    );
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), root.clone()))
            .expect("thread")
            .push(late);
    });
    let arrived = poller.poll(&fake, &client).await;
    assert_eq!(arrived.texts(), ["late in the feed"]);
    assert_eq!(arrived.replies[0]["oldest"], ts(32, 0));
    assert!(!arrived.tick.replies_behind);

    // A reply its feed never lists: given up after the bound.
    fake.with(|state| {
        add_reply(state, CH, &root, &ts(110, 0), "never in the feed");
        hide_from_the_feed(state, &root);
    });
    let lag = lag_reads();
    let (calls, behind) = run(&mut poller, &fake, &client, lag + 2).await;
    assert_eq!(calls, then(1, lag, 0, 2));
    assert_eq!(behind, then(true, lag - 1, false, 3));

    // A later reply is read from the latest reply its root named.
    fake.with(|state| add_reply(state, CH, &root, &ts(120, 0), "later"));
    let later = poller.poll(&fake, &client).await;
    assert_eq!(later.texts(), ["later"]);
    assert_eq!(later.replies[0]["oldest"], ts(110, 0));
}

/// Only reads in a row that bring nothing count towards `LIVE_LAG_READS`: a read that brings a
/// reply, though not yet the root's latest, starts the count again, and leaves the thread waiting
/// for the rest without counting itself.
#[tokio::test]
async fn a_read_that_brings_a_reply_starts_the_run_of_reads_that_bring_none_again() {
    let (state, root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    // The root names two replies its feed lists neither of yet.
    let (sooner, latest) = fake.with(|state| {
        add_reply(state, CH, &root, &ts(100, 0), "sooner");
        add_reply(state, CH, &root, &ts(105, 0), "latest");
        let latest = hide_from_the_feed(state, &root);
        (hide_from_the_feed(state, &root), latest)
    });
    let lag = lag_reads();
    let (calls, behind) = run(&mut poller, &fake, &client, lag - 1).await;
    assert_eq!(calls, vec![1; lag - 1]);
    assert_eq!(behind, vec![true; lag - 1], "one read short of the bound");

    // The feed lists the sooner reply: a read that brings it starts the count again.
    let push = |reply: Value| {
        fake.with(|state| {
            state
                .replies
                .get_mut(&(CH.to_owned(), root.clone()))
                .expect("thread")
                .push(reply);
        });
    };
    push(sooner);
    let partial = poller.poll(&fake, &client).await;
    assert_eq!(partial.texts(), ["sooner"]);
    assert!(
        partial.tick.replies_behind,
        "the root's latest has not come back"
    );
    let (calls, behind) = run(&mut poller, &fake, &client, lag - 1).await;
    assert_eq!(calls, vec![1; lag - 1]);
    assert_eq!(
        behind,
        vec![true; lag - 1],
        "the count started again at the read that brought a reply"
    );

    push(latest);
    let arrived = poller.poll(&fake, &client).await;
    assert_eq!(arrived.texts(), ["latest"]);
    assert_eq!(arrived.replies[0]["oldest"], ts(100, 0));
    assert!(!arrived.tick.replies_behind);
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

/// `#378 live-reactions`: a thumbs-up added in the native app to a message already read is
/// published as one update on the next poll, from the newest page that poll already reads —
/// no extra Web API call — and its removal (the field disappears) is published the same way.
#[tokio::test]
async fn a_reaction_added_in_the_native_app_arrives_as_an_update_with_no_extra_read() {
    let (state, _root) = one_thread();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let mut poller = Poller::new();
    let seed = poller.poll(&fake, &client).await;
    assert!(seed.published.is_empty(), "seeding publishes nothing");

    // Another message keeps a set throughout, so every history page proves the endpoint
    // reports reactions — the gate under which a vanished field may read as a removal below.
    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("history");
        list[6]["reactions"] = json!([{"name": "eyes", "count": 1, "users": ["UADA00001"]}]);
    });
    // Learn that standing set before the target's own changes begin.
    poller.poll(&fake, &client).await;

    let target = ts(50, 0);
    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("history");
        let message = list
            .iter_mut()
            .find(|message| message["ts"] == json!(target))
            .expect("the message");
        message["reactions"] = json!([{"name": "+1", "count": 1, "users": ["UADA00001"]}]);
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(poll.tick.published, 0, "no new message arrived");
    assert_eq!(poll.tick.changed, 1, "one reaction update");
    assert_eq!(
        poll.history, 1,
        "the page the poll already reads carried it"
    );
    assert!(
        poll.replies.is_empty(),
        "no thread moved, so no thread is read"
    );
    assert_eq!(poll.published.len(), 1, "{:?}", poll.texts());
    let update = &poll.published[0];
    assert_eq!(update.kind, vibe_talk::live::LiveKind::Update);
    let reactions = update.message.reactions.clone().expect("reported");
    assert_eq!(reactions.len(), 1);
    assert_eq!(reactions[0].emoji, "👍");
    assert_eq!(reactions[0].count, 1);
    assert!(!reactions[0].owner_reacted);

    let quiet = poller.poll(&fake, &client).await;
    assert!(
        quiet.published.is_empty(),
        "an unchanged set is not announced again"
    );
    assert_eq!(quiet.tick.changed, 0);

    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("history");
        let message = list
            .iter_mut()
            .find(|message| message["ts"] == json!(target))
            .expect("the message");
        message.as_object_mut().expect("object").remove("reactions");
    });
    let removed = poller.poll(&fake, &client).await;
    assert_eq!(removed.tick.changed, 1, "the removal is an update too");
    assert_eq!(
        removed.published[0].message.reactions,
        Some(Vec::new()),
        "and it says none outright, so the page takes the chip away"
    );
}

/// `#378 live-reactions`: a reaction on a message older than the hub's seed window — the poll
/// limit caps what seeding teaches the hub, while Slack's own memory holds its whole newest
/// page — is still published: the provider's report is trusted, not re-gated on the hub.
#[tokio::test]
async fn a_reaction_beyond_the_seed_window_arrives() {
    let mut state = FakeState::default();
    let list: Vec<Value> = (0..60)
        .map(|i| profiled(&ts(i * 10, 0), "UADA00001", "Ada", &format!("message {i}")))
        .collect();
    state.history.insert(CH.to_owned(), list);
    // One page holds them all here, so Slack seeds 60 sets while the hub learns the newest 50.
    let fake = Fake::start(state, 200).await;
    let client = fake.client();
    let mut poller = Poller::new();
    poller.poll(&fake, &client).await;

    fake.with(|state| {
        let list = state.history.get_mut(CH).expect("history");
        list[0]["reactions"] = json!([{"name": "+1", "count": 1, "users": ["UADA00001"]}]);
    });
    let poll = poller.poll(&fake, &client).await;
    assert_eq!(
        poll.tick.changed, 1,
        "the oldest message's reaction is published"
    );
    assert_eq!(poll.published.len(), 1);
    assert_eq!(poll.published[0].message.content, "message 0");
    assert_eq!(
        poll.published[0]
            .message
            .reactions
            .as_ref()
            .map(|sets| sets[0].emoji.as_str()),
        Some("👍")
    );
}
