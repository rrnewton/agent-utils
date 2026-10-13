//! What one read of a Slack conversation's Flat view costs in Web API calls, counted at a loopback
//! fake that answers 20 rows a call, as the bridge in front of Slack does. `#282 slack-flat-skip`,
//! remote-ops audit W8.
//!
//! Flat is the page's default view, and on Slack every poll of it is a full read. Each one used to
//! walk the replies of up to ten threads from their first reply: 3 + Σ min(ceil((R+1)/20), 10)
//! calls, 23 for ten threads of thirty replies, every 45 seconds. A root carries its `reply_count`
//! and `latest_reply`, and the read has just fetched every root on the page, so a thread whose root
//! says what it said last time has nothing new to read, and one that moved needs only what came
//! after the newest reply already held.

mod slack_fake;

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use slack_fake::{profiled, thread, ts, Fake, FakeState};
use vibe_talk::chat::ChatClient;
use vibe_talk::model::{ChannelId, Message};
use vibe_talk::slack::{ts_from_message_id, HttpSlackClient};
use vibe_talk::threads::{TimelineRequest, TimelineView};

const CH: &str = "C0000MAIN1";
/// Rows the fake answers per call, as the bridge in front of Slack does.
const PAGE_CAP: usize = 20;
/// Replies in each thread: two calls each to walk in full at [`PAGE_CAP`].
const REPLIES: i64 = 30;

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

fn ts_list(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|message| ts_from_message_id(&message.id).expect("a Slack-encoded id"))
        .collect()
}

/// The `ts` of reply `index` in the thread rooted on message `root` of [`ten_threads`].
fn reply_ts(root: i64, index: i64) -> String {
    ts(1000 + root * 100 + index, 0)
}

/// Fifty messages, every fifth the root of a thread of [`REPLIES`] replies: one Flat page of
/// fifty holds all ten roots. Returns the roots' `ts` and each root's message index.
fn ten_threads() -> (FakeState, Vec<(String, i64)>) {
    let mut state = FakeState::default();
    let mut list = Vec::new();
    let mut roots = Vec::new();
    for i in 0..50 {
        let at = ts(i * 10, 0);
        let mut message = profiled(&at, "UADA00001", "Ada", &format!("message {i}"));
        if i % 5 == 0 {
            let replies: Vec<(String, String)> = (0..REPLIES)
                .map(|r| (reply_ts(i, r), format!("reply {r} to {i}")))
                .collect();
            thread(&mut state, CH, &mut message, &replies);
            roots.push((at, i));
        }
        list.push(message);
    }
    state.history.insert(CH.to_owned(), list);
    (state, roots)
}

/// Make the root `root` say what Slack's would after its replies changed.
fn restate_root(state: &mut FakeState, root: &str) {
    let replies = &state.replies[&(CH.to_owned(), root.to_owned())];
    let count = replies.len();
    let latest = replies.last().map(|reply| reply["ts"].clone());
    let message = state
        .history
        .get_mut(CH)
        .expect("history")
        .iter_mut()
        .find(|message| message["ts"] == json!(root))
        .expect("root");
    message["reply_count"] = json!(count);
    message["latest_reply"] = latest.unwrap_or(serde_json::Value::Null);
}

/// Post a reply in `root`'s thread, as somebody else would.
fn add_reply(fake: &Fake, root: &str, at: &str) {
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), root.to_owned()))
            .expect("thread")
            .push(
                json!({"type": "message", "ts": at, "thread_ts": root, "user": "UREPLIER1",
                "text": format!("news at {at}"), "user_profile": {"display_name": "Replier"}}),
            );
        restate_root(state, root);
    });
}

/// What one Flat read returned, and the Web API calls it made.
struct Read {
    messages: Vec<String>,
    history: usize,
    /// Every `conversations.replies` request the read sent, in order.
    replies: Vec<HashMap<String, String>>,
}

impl Read {
    fn calls(&self) -> usize {
        self.history + self.replies.len()
    }

    /// The threads whose replies the read asked for, in order.
    fn threads(&self) -> Vec<String> {
        self.replies
            .iter()
            .map(|params| params["ts"].clone())
            .collect()
    }
}

async fn flat(fake: &Fake, client: &HttpSlackClient) -> Read {
    let history = fake.calls("conversations.history");
    let replies = fake.requests("conversations.replies").len();
    let page = client
        .fetch_timeline(
            &channel(),
            &TimelineRequest {
                view: TimelineView::Flat,
                thread_id: None,
                before: None,
                after: None,
                limit: 50,
            },
        )
        .await
        .expect("flat");
    let messages = ts_list(&page.messages);
    assert!(
        messages.windows(2).all(|pair| pair[0] < pair[1]),
        "oldest first, each message once"
    );
    Read {
        messages,
        history: fake.calls("conversations.history") - history,
        replies: fake.requests("conversations.replies")[replies..].to_vec(),
    }
}

#[tokio::test]
async fn a_flat_read_again_reads_replies_only_for_the_threads_that_moved() {
    for root_always in [false, true] {
        only_the_threads_that_moved(root_always).await;
    }
}

/// `root_always`: the server repeats the root on every replies page, as some do.
async fn only_the_threads_that_moved(root_always: bool) {
    let (mut state, roots) = ten_threads();
    state.root_always = root_always;
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();

    let first = flat(&fake, &client).await;
    assert_eq!(first.messages.len(), 50 + 10 * 30, "every root's replies");
    assert_eq!(
        (first.history, first.replies.len()),
        (3, 20),
        "the first read walks every thread: 3 history pages and 2 replies pages per thread"
    );
    assert_eq!(first.calls(), 23, "the audit's count for ten threads of 30");

    // Nothing new: no thread is read.
    let again = flat(&fake, &client).await;
    assert_eq!(again.messages, first.messages);
    assert_eq!(
        again.history, 3,
        "history is read every time, roots and all"
    );
    assert!(
        again.replies.is_empty(),
        "no thread moved, yet these were read: {:?}",
        again.threads()
    );

    // One new reply: one call, for that thread, from the newest reply already held.
    let (moved, index) = roots[3].clone();
    let news = ts(9000, 0);
    add_reply(&fake, &moved, &news);
    let one = flat(&fake, &client).await;
    assert_eq!(
        one.threads(),
        std::slice::from_ref(&moved),
        "only the thread that moved"
    );
    assert_eq!(
        one.replies[0].get("oldest"),
        Some(&reply_ts(index, REPLIES - 1)),
        "read from the newest reply already held"
    );
    let mut expected = first.messages.clone();
    expected.push(news.clone());
    expected.sort();
    assert_eq!(one.messages, expected, "the new reply, and nothing lost");
    assert_eq!(one.calls(), 3 + 1);

    // What that read brought is held: the next read reads nothing again.
    let settled = flat(&fake, &client).await;
    assert!(settled.replies.is_empty(), "{:?}", settled.threads());
    assert_eq!(settled.messages, expected);

    // A deletion leaves the count short of what was held: that thread is read in full.
    let (pruned, index) = roots[7].clone();
    let gone = reply_ts(index, 4);
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), pruned.clone()))
            .expect("thread")
            .retain(|reply| reply["ts"] != json!(gone));
        restate_root(state, &pruned);
    });
    let deleted = flat(&fake, &client).await;
    assert_eq!(deleted.threads(), [pruned.clone(), pruned.clone()]);
    assert!(
        deleted
            .replies
            .iter()
            .all(|params| !params.contains_key("oldest")),
        "a deletion is read in full, not forward"
    );
    expected.retain(|at| *at != gone);
    assert_eq!(deleted.messages, expected, "the deleted reply is gone");

    // One reply added and another deleted leave the count where it was: read in full as well.
    let (churned, index) = roots[1].clone();
    let swapped = reply_ts(index, 10);
    let late = ts(9100, 0);
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), churned.clone()))
            .expect("thread")
            .retain(|reply| reply["ts"] != json!(swapped));
    });
    add_reply(&fake, &churned, &late);
    let churn = flat(&fake, &client).await;
    assert_eq!(churn.threads(), [churned.clone(), churned.clone()]);
    expected.retain(|at| *at != swapped);
    expected.push(late);
    expected.sort();
    assert_eq!(churn.messages, expected);

    // More new replies than Flat shows of a thread: the newest of them, read in full at once,
    // never the oldest of the new ones.
    let (flooded, _) = roots[9].clone();
    for second in 0..60 {
        add_reply(&fake, &flooded, &ts(10_000 + second, 0));
    }
    let flood = flat(&fake, &client).await;
    assert_eq!(flood.replies.len(), 5, "the root and 90 replies, 20 a call");
    assert!(
        flood
            .replies
            .iter()
            .all(|params| params["ts"] == flooded && !params.contains_key("oldest")),
        "{:?}",
        flood.replies
    );
    for second in 10..60 {
        assert!(flood.messages.contains(&ts(10_000 + second, 0)));
    }
    assert!(
        !flood.messages.contains(&ts(10_000 + 9, 0)),
        "the newest 50 only"
    );
}

/// Remembered replies are trusted for the page's reconcile period, fifteen minutes by default, and
/// then read in full again: that is how an edit to a reply, which changes nothing on its root,
/// reaches Flat. Here the period is cut to a tenth of a second.
#[tokio::test]
async fn remembered_replies_are_read_in_full_again_after_the_reconcile_period() {
    let (state, roots) = ten_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake
        .client()
        .with_flat_reconcile(Duration::from_millis(100));
    let first = flat(&fake, &client).await;
    assert_eq!(first.replies.len(), 20);

    let (edited, index) = roots[0].clone();
    let target = reply_ts(index, 29);
    fake.with(|state| {
        let reply = state
            .replies
            .get_mut(&(CH.to_owned(), edited.clone()))
            .expect("thread")
            .iter_mut()
            .find(|reply| reply["ts"] == json!(target))
            .expect("reply");
        reply["text"] = json!("edited after the read");
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    let page = client
        .fetch_timeline(
            &channel(),
            &TimelineRequest {
                view: TimelineView::Flat,
                thread_id: None,
                before: None,
                after: None,
                limit: 50,
            },
        )
        .await
        .expect("flat");
    assert_eq!(
        fake.requests("conversations.replies").len(),
        40,
        "every thread read in full again"
    );
    assert!(page
        .messages
        .iter()
        .any(|message| message.content == "edited after the read"));

    // Reconciling every read is what Flat did before it remembered anything.
    let every = fake.client().with_flat_reconcile(Duration::ZERO);
    for _ in 0..2 {
        assert_eq!(flat(&fake, &every).await.replies.len(), 20);
    }
}

/// The `conversations.replies` requests that asked for `root`'s thread.
fn reads_of(read: &Read, root: &str) -> Vec<HashMap<String, String>> {
    read.replies
        .iter()
        .filter(|params| params["ts"] == root)
        .cloned()
        .collect()
}

/// A full read of a thread is held only when it agrees with the root the same Flat read fetched:
/// a replies feed that lags the history row (a replica or a cache in the bridge) leaves the
/// newest reply out, and the next read, the root unchanged, must not reuse that. Remote-ops audit
/// W8 review.
#[tokio::test]
async fn a_full_read_that_lagged_its_root_is_read_again() {
    let (state, roots) = ten_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let (lagging, index) = roots[2].clone();
    let newest = reply_ts(index, REPLIES - 1);
    // The root says thirty replies, the newest of them latest; the feed has only twenty-nine.
    let hidden = fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), lagging.clone()))
            .expect("thread")
            .pop()
            .expect("a reply")
    });
    let first = flat(&fake, &client).await;
    assert_eq!(first.calls(), 23);
    assert!(!first.messages.contains(&newest), "the feed lags");

    // The feed catches up. The root already said thirty, so it reads exactly as before.
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), lagging.clone()))
            .expect("thread")
            .push(hidden);
    });
    let second = flat(&fake, &client).await;
    assert_eq!(
        second.threads(),
        [lagging.clone(), lagging.clone()],
        "only the thread whose read did not agree with its root, read in full"
    );
    assert!(reads_of(&second, &lagging)
        .iter()
        .all(|params| !params.contains_key("oldest")));
    assert!(
        second.messages.contains(&newest),
        "the reply the root reported is shown"
    );

    // That read agreed with its root: held from now on.
    let third = flat(&fake, &client).await;
    assert!(third.replies.is_empty(), "{:?}", third.threads());
    assert_eq!(third.messages, second.messages);
}

/// A reply posted after a Flat read fetched history and before it fetched that thread is newer
/// than the latest reply the root named, so that read is not held. Deleted again before the next
/// read, the root reads exactly as before, and the reply must not linger. Remote-ops audit W8
/// review.
#[tokio::test]
async fn a_reply_posted_during_a_read_and_deleted_is_not_kept() {
    let (state, roots) = ten_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let (raced, _) = roots[4].clone();
    let racer = ts(9500, 0);
    // In the feed, but not yet on the root the read sees.
    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), raced.clone()))
            .expect("thread")
            .push(
                json!({"type": "message", "ts": racer, "thread_ts": raced, "user": "UREPLIER1",
                "text": "the wrong thread", "user_profile": {"display_name": "Replier"}}),
            );
    });
    let first = flat(&fake, &client).await;
    assert_eq!(first.calls(), 23);
    assert!(first.messages.contains(&racer));

    fake.with(|state| {
        state
            .replies
            .get_mut(&(CH.to_owned(), raced.clone()))
            .expect("thread")
            .retain(|reply| reply["ts"] != json!(racer));
    });
    let second = flat(&fake, &client).await;
    assert_eq!(second.threads(), [raced.clone(), raced.clone()]);
    assert!(
        !second.messages.contains(&racer),
        "a deleted reply is still shown"
    );

    let third = flat(&fake, &client).await;
    assert!(third.replies.is_empty(), "{:?}", third.threads());
    assert_eq!(third.messages, second.messages);
}

/// A root that carries `reply_count` but no `latest_reply` leaves the count as the only sign of
/// change, and one reply deleted and another added leave it where it was. Such a thread is read in
/// full on every Flat read, as before W8: no saving, and nothing stale. Remote-ops audit W8 review.
#[tokio::test]
async fn a_root_without_latest_reply_is_read_in_full_every_time() {
    let (state, roots) = ten_threads();
    let fake = Fake::start(state, PAGE_CAP).await;
    fake.with(|state| {
        for message in state.history.get_mut(CH).expect("history") {
            message
                .as_object_mut()
                .expect("an object")
                .remove("latest_reply");
        }
    });
    let client = fake.client();
    let first = flat(&fake, &client).await;
    assert_eq!(first.calls(), 23);

    let (churned, index) = roots[1].clone();
    let gone = reply_ts(index, 3);
    let added = ts(9200, 0);
    fake.with(|state| {
        let replies = state
            .replies
            .get_mut(&(CH.to_owned(), churned.clone()))
            .expect("thread");
        replies.retain(|reply| reply["ts"] != json!(gone));
        replies.push(
            json!({"type": "message", "ts": added, "thread_ts": churned, "user": "UREPLIER1",
            "text": "a new reply", "user_profile": {"display_name": "Replier"}}),
        );
    });
    let second = flat(&fake, &client).await;
    assert_eq!(
        (second.history, second.replies.len()),
        (3, 20),
        "every thread read in full, as before"
    );
    assert!(second.messages.contains(&added), "the new reply is shown");
    assert!(
        !second.messages.contains(&gone),
        "the deleted reply is gone"
    );
}
