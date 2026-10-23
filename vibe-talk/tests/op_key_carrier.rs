//! `#465 replication-c6-op-key-marker`: every post carries its operation key where the provider can hold one invisibly, and
//! every read hands back a key it finds in the provider-neutral `Message::op_key`, through the
//! provider trait, so nothing above it switches on the backend.
//!
//! Per provider: Slack files the key as message metadata and returns it to reads that ask; a
//! transport in front of Slack that drops metadata posts without it, and reads find none. The
//! in-memory provider carries it, deduplicates by it, or lacks it, as each test says.

mod slack_fake;

use slack_fake::{profiled, ts, Fake, FakeState};
use vibe_talk::chat::{ChatClient, OpKeyCarrier, OpKeyDedupe};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, MessageId};

const CH: &str = "C0000MAIN1";
const PAGE_CAP: usize = 20;
const KEY: &str = "out-1.00000000deadbeef.0";

fn channel() -> ChannelId {
    ChannelId(CH.to_owned())
}

fn fixture(drops_metadata: bool) -> FakeState {
    let mut state = FakeState {
        drops_metadata,
        ..FakeState::default()
    };
    state.history.insert(
        CH.to_owned(),
        vec![profiled(&ts(1_700_000_000, 100), "U0ADA", "Ada", "hello")],
    );
    state
}

#[tokio::test]
async fn slack_files_the_key_as_metadata_and_reads_hand_it_back() {
    let fake = Fake::start(fixture(false), PAGE_CAP).await;
    let client = fake.client();
    let carrier = client.op_key_carrier(&channel());
    assert!(
        carrier.carried,
        "every keyed Slack post carries its key: {carrier:?}"
    );
    assert!(!carrier.readable, "{carrier:?}");
    assert_eq!(
        carrier.dedupes,
        OpKeyDedupe::Never,
        "Slack does not deduplicate by metadata"
    );
    let posted = client
        .post_message_keyed(&channel(), "on it", None, Some(KEY))
        .await
        .expect("posted");
    let request = &fake.requests("chat.postMessage")[0];
    let metadata: serde_json::Value =
        serde_json::from_str(request.get("metadata").expect("metadata sent")).expect("JSON");
    assert_eq!(metadata["event_type"], "vibe_talk_op_key");
    assert_eq!(metadata["event_payload"]["op_key"], KEY);
    assert_eq!(
        request.get("text").map(String::as_str),
        Some("on it"),
        "the words carry no key"
    );
    assert_eq!(posted.op_key.as_deref(), Some(KEY));
    // The fake is a bridge in front of Slack (it is not Slack's own Web API): one that echoes
    // the keys of the posts it saw succeed is still never readable, because a post whose answer
    // was lost would not show its key, and a read without one proves nothing.
    let carrier = client.op_key_carrier(&channel());
    assert!(carrier.carried && !carrier.readable, "{carrier:?}");
    // A read finds it on the message, and only on that message.
    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    let found: Vec<(String, Option<String>)> = page
        .iter()
        .map(|m| (m.content.clone(), m.op_key.clone()))
        .collect();
    assert_eq!(
        found,
        [
            ("hello".to_owned(), None),
            ("on it".to_owned(), Some(KEY.to_owned()))
        ]
    );
    assert!(
        fake.requests("conversations.history")
            .iter()
            .all(|r| r.get("include_all_metadata").map(String::as_str) == Some("true")),
        "a read that does not ask for metadata never sees it"
    );
    // An unkeyed post files no metadata at all.
    client
        .post_message(&channel(), "plain", None)
        .await
        .expect("posted");
    assert!(!fake.requests("chat.postMessage")[1].contains_key("metadata"));
}

#[tokio::test]
async fn a_thread_post_carries_the_key_too() {
    let fake = Fake::start(fixture(false), PAGE_CAP).await;
    let client = fake.client();
    let root = ts(1_700_000_000, 100);
    let posted = client
        .post_in_thread_keyed(&channel(), &root, "in the thread", None, Some(KEY))
        .await
        .expect("posted");
    assert_eq!(posted.op_key.as_deref(), Some(KEY));
    let request = &fake.requests("chat.postMessage")[0];
    assert_eq!(request.get("thread_ts"), Some(&root));
    assert!(request.get("metadata").is_some_and(|m| m.contains(KEY)));
}

#[tokio::test]
async fn a_transport_that_drops_metadata_posts_without_it_and_reads_find_none() {
    let fake = Fake::start(fixture(true), PAGE_CAP).await;
    let client = fake.client();
    let posted = client
        .post_message_keyed(&channel(), "on it", None, Some(KEY))
        .await
        .expect("posted");
    assert_eq!(
        posted.op_key, None,
        "no key is invented that the provider did not return"
    );
    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert!(page.iter().all(|m| m.op_key.is_none()));
    assert!(
        !client.op_key_carrier(&channel()).readable,
        "a transport that never handed a key back is not promised to"
    );
}

#[tokio::test]
async fn metadata_another_party_files_is_never_read_as_a_key() {
    // A transport in front of Slack may file metadata of its own on the posts it makes.
    let mut state = fixture(false);
    let mut theirs = profiled(
        &ts(1_700_000_001, 0),
        "U0BOT",
        "bot",
        "posted by the transport",
    );
    theirs["metadata"] = serde_json::json!({
        "event_type": "vibe_talk_post",
        "event_payload": {"request_id": "abc", "op_key": "not-ours"},
    });
    state.history.get_mut(CH).expect("channel").push(theirs);
    let fake = Fake::start(state, PAGE_CAP).await;
    let client = fake.client();
    let page = client
        .fetch_page(&channel(), 50, None, None)
        .await
        .expect("history");
    assert!(page.iter().all(|m| m.op_key.is_none()), "{page:?}");
    assert!(!client.op_key_carrier(&channel()).readable);
}

#[tokio::test]
async fn the_in_memory_provider_lacks_a_carrier_unless_given_one() {
    let fake = FakeDiscord::new();
    let room = ChannelId("123".to_owned());
    fake.seed(&room, "ada", "hello");
    assert_eq!(fake.op_key_carrier(&room), OpKeyCarrier::NONE);
    assert!(!fake.op_key_carrier(&room).carries());
    let first = fake
        .post_message_keyed(&room, "on it", None, Some(KEY))
        .await
        .expect("posted");
    assert_eq!(first.op_key, None);
    let again = fake
        .post_message_keyed(&room, "on it", None, Some(KEY))
        .await
        .expect("posted");
    assert_ne!(first.id, again.id, "with no carrier a repeat posts again");
    assert!(fake.keyed_posts().is_empty());
}

#[tokio::test]
async fn the_in_memory_provider_carries_readable_and_deduplicated_keys() {
    let fake = FakeDiscord::new();
    let room = ChannelId("123".to_owned());
    fake.seed(&room, "ada", "hello");
    fake.carry_op_keys(OpKeyCarrier {
        carried: true,
        dedupes: OpKeyDedupe::Always,
        readable: true,
    });
    assert!(fake.supports_idempotent_posts());
    let first = fake
        .post_message_keyed(&room, "on it", None, Some(KEY))
        .await
        .expect("posted");
    assert_eq!(first.op_key.as_deref(), Some(KEY));
    let again = fake
        .post_message_keyed(&room, "on it", None, Some(KEY))
        .await
        .expect("answered");
    assert_eq!(
        again.id, first.id,
        "a deduplicating carrier answers a repeat with the first"
    );
    assert_eq!(fake.posted().len(), 1);
    let read = fake.fetch_page(&room, 50, None, None).await.expect("read");
    let keyed: Vec<&MessageId> = read
        .iter()
        .filter(|m| m.op_key.as_deref() == Some(KEY))
        .map(|m| &m.id)
        .collect();
    assert_eq!(keyed, [&first.id]);
    assert_eq!(fake.keyed_posts(), [(KEY.to_owned(), first.id.clone())]);
}
