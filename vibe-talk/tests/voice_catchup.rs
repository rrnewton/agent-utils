//! `#210 voice-catchup-speed`: a catch-up answers by a deadline against a slow chat provider that
//! reads one channel at a time, from what the server holds when a channel is not read again in
//! time.
//!
//! The provider here is [`vibe_talk::testing::SlowChat`]: every read queues for one slot and then
//! takes two seconds, the measured median of the adapter this was written against. The clock is
//! paused, so the durations asserted below are exact and the suite spends none of them.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tokio::time::Instant;
use tower::ServiceExt as _;

use vibe_talk::auth::Scope;
use vibe_talk::chat::ChatClient;
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::mcp::protocol::{dispatch, Outcome};
use vibe_talk::model::ChannelId;
use vibe_talk::ops::{self, ActivityRequest, Currency, OpError};
use vibe_talk::recent::{self, ACTIVITY_DEADLINE, COLD_DEADLINE};
use vibe_talk::state::AppState;
use vibe_talk::testing::{self, SlowChat, READ_CHANNEL, WRITE_CHANNEL, WRITE_TOKEN};

/// A third channel, so a catch-up costs what the measured ones did: three reads.
const THIRD: &str = "3333333333";

/// One read once it holds the slot: the measured median was 1.9 seconds.
const READ: Duration = Duration::from_secs(2);

const HOUR_MS: i64 = 3_600_000;

struct Harness {
    state: AppState,
    fake: Arc<FakeDiscord>,
    slow: Arc<SlowChat>,
}

fn harness_with(delay: Duration) -> Harness {
    let toml = testing::config_toml().replace(
        "[elevenlabs]",
        &format!("[[channels]]\nid = \"{THIRD}\"\nlabel = \"night shift\"\nwritable = false\n\n[elevenlabs]"),
    );
    let (mut state, fake, _elevenlabs) = testing::state_from_toml(&toml);
    fake.register_channel(&channel(THIRD));
    let now = ops::now_ms();
    for (id, text) in [
        (READ_CHANNEL, "build is green"),
        (WRITE_CHANNEL, "ship it"),
        (THIRD, "pager is quiet"),
    ] {
        fake.seed_at(&channel(id), "agent", text, now - HOUR_MS);
    }
    let slow = Arc::new(SlowChat::new(Arc::clone(&fake), delay));
    let client: Arc<dyn ChatClient> = slow.clone();
    state.replace_chat(client);
    Harness { state, fake, slow }
}

fn harness() -> Harness {
    harness_with(READ)
}

fn channel(id: &str) -> ChannelId {
    ChannelId(id.to_owned())
}

async fn catch_up(state: &AppState) -> ops::Activity {
    ops::activity(
        state,
        ActivityRequest {
            hours: None,
            per_channel_limit: None,
            now_ms: ops::now_ms(),
        },
    )
    .await
    .expect("a catch-up answers")
}

fn part<'a>(activity: &'a ops::Activity, id: &str) -> &'a ops::ChannelActivity {
    activity
        .channels
        .iter()
        .find(|c| c.channel.id.as_str() == id)
        .unwrap_or_else(|| panic!("{id} is not in the answer"))
}

fn said(activity: &ops::Activity, id: &str) -> Vec<String> {
    part(activity, id)
        .read
        .as_ref()
        .unwrap_or_else(|e| panic!("{id} was not answered: {e}"))
        .entries
        .iter()
        .map(|e| e.summary.clone())
        .collect()
}

fn current(activity: &ops::Activity, id: &str) -> bool {
    part(activity, id).currency == Currency::Current
}

fn tool_call() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "recent_activity", "arguments": {} },
    })
}

fn text_of(outcome: &Outcome) -> String {
    let Outcome::Reply(response) = outcome else {
        panic!("expected a reply, got {outcome:?}");
    };
    let result = response.result.as_ref().expect("a result");
    assert_eq!(result["isError"], false, "{result}");
    result["content"][0]["text"]
        .as_str()
        .expect("text")
        .to_owned()
}

/// The measured case, cold: nothing held, three reads in series. The catch-up waits for reads it
/// has nothing to stand in for, and still answers inside the voice runtime's fifteen seconds.
#[tokio::test(start_paused = true)]
async fn a_cold_catch_up_waits_for_the_reads_it_has_nothing_to_stand_in_for() {
    let h = harness();
    let began = Instant::now();
    let activity = catch_up(&h.state).await;
    let took = began.elapsed();

    assert_eq!(took, 3 * READ, "three reads, one at a time: {took:?}");
    assert!(took < COLD_DEADLINE);
    for (id, text) in [
        (READ_CHANNEL, "build is green"),
        (WRITE_CHANNEL, "ship it"),
        (THIRD, "pager is quiet"),
    ] {
        assert_eq!(said(&activity, id), [text]);
        assert!(current(&activity, id), "{id}");
    }
    assert_eq!(h.slow.reads(), 3, "one read per channel");
}

/// What the call-start warm-up buys: the greeting covers the reads, and the agent's first
/// catch-up is answered from them well inside the deadline, without asking again.
#[tokio::test(start_paused = true)]
async fn a_catch_up_after_a_call_starts_answers_inside_the_deadline() {
    let h = harness();
    assert_eq!(recent::warm(&h.state), 3);
    // A second call start in the same breath reads nothing more.
    recent::warm(&h.state);
    // The greeting: measured at five to six seconds before the first tool call.
    tokio::time::sleep(Duration::from_millis(5_500)).await;

    let began = Instant::now();
    let activity = catch_up(&h.state).await;
    let took = began.elapsed();

    assert!(took <= ACTIVITY_DEADLINE, "{took:?}");
    assert_eq!(
        took,
        Duration::from_millis(500),
        "it waited only for the third read to land"
    );
    for id in [READ_CHANNEL, WRITE_CHANNEL, THIRD] {
        assert!(current(&activity, id), "{id}");
    }
    assert_eq!(
        h.slow.reads(),
        3,
        "the catch-up used the call start's reads instead of queueing its own"
    );
}

/// The incident of 10-07: one provider command stalled for a minute. The catch-up answers at its
/// deadline, the stalled channel from its held copy, labelled; the channel queued behind it too.
/// A second catch-up during the stall answers the same way and asks the provider for nothing.
#[tokio::test(start_paused = true)]
async fn a_stalled_channel_is_answered_from_its_held_copy_and_a_retry_adds_no_reads() {
    let h = harness();
    catch_up(&h.state).await;
    // Long enough that every window must be read again.
    tokio::time::sleep(Duration::from_secs(20)).await;
    h.slow
        .stall(&channel(WRITE_CHANNEL), Duration::from_secs(58));
    let before = h.slow.reads();

    let began = Instant::now();
    let activity = catch_up(&h.state).await;
    let took = began.elapsed();

    assert_eq!(
        took, ACTIVITY_DEADLINE,
        "answered at the deadline, not after the stall"
    );
    assert!(
        current(&activity, READ_CHANNEL),
        "the channel read in time is current"
    );
    for id in [WRITE_CHANNEL, THIRD] {
        match part(&activity, id).currency {
            Currency::Held { age, failed } => {
                assert!(age >= Duration::from_secs(20), "{id}: {age:?}");
                assert_eq!(failed, None, "{id}: still being read, not failed");
            }
            Currency::Current => panic!("{id} was presented as current"),
        }
    }
    assert_eq!(
        said(&activity, WRITE_CHANNEL),
        ["ship it"],
        "stale, not dropped"
    );
    assert_eq!(h.slow.reads() - before, 3);

    // The retry, at the voice runtime's pace.
    let began = Instant::now();
    let text = text_of(&dispatch(&h.state, Scope::Read, &tool_call()).await);
    assert!(
        began.elapsed() <= ACTIVITY_DEADLINE,
        "{:?}",
        began.elapsed()
    );
    assert_eq!(
        h.slow.reads() - before,
        3,
        "the retry queued more reads behind the stalled one"
    );
    let lead = text
        .lines()
        .find(|line| line.starts_with("lead team"))
        .expect("the stalled channel has a block");
    assert!(lead.contains("NOT CURRENT"), "{lead}");
    assert!(lead.contains("did not answer in time"), "{lead}");
    assert!(lead.contains("may be missing newer messages"), "{lead}");
    let build = text
        .lines()
        .find(|line| line.starts_with("build noise"))
        .expect("the current channel has a block");
    assert!(!build.contains("NOT CURRENT"), "{build}");

    // When the stall clears, the reads already running bring every channel up to date.
    tokio::time::sleep(Duration::from_secs(60)).await;
    let before = h.slow.reads();
    let activity = catch_up(&h.state).await;
    for id in [WRITE_CHANNEL, THIRD] {
        assert!(current(&activity, id), "{id}");
    }
    assert_eq!(
        h.slow.reads() - before,
        1,
        "only the window past reuse is read again"
    );
}

/// The voice runtime gives up at fifteen seconds and drops the call. The reads it started do not
/// go to waste: they land in the windows, and the retry is answered from them at once.
#[tokio::test(start_paused = true)]
async fn an_abandoned_catch_up_still_fills_the_windows_for_the_next_one() {
    let h = harness();
    let abandoned = tokio::time::timeout(Duration::from_millis(500), catch_up(&h.state)).await;
    assert!(abandoned.is_err(), "the caller gave up first");

    tokio::time::sleep(Duration::from_secs(7)).await;
    for id in [READ_CHANNEL, WRITE_CHANNEL, THIRD] {
        assert!(h.state.live.recent().holds(&channel(id)), "{id}");
    }
    let before = h.slow.reads();
    let began = Instant::now();
    let activity = catch_up(&h.state).await;
    assert_eq!(
        began.elapsed(),
        Duration::ZERO,
        "answered from what the abandoned call read"
    );
    assert_eq!(h.slow.reads(), before);
    for id in [READ_CHANNEL, WRITE_CHANNEL, THIRD] {
        assert!(current(&activity, id), "{id}");
    }
}

/// A read that fails is answered from the held copy, naming the failure, rather than failing the
/// whole answer the way it does when nothing is held.
#[tokio::test(start_paused = true)]
async fn a_failed_read_is_answered_from_the_held_copy_and_names_the_failure() {
    let h = harness_with(Duration::from_millis(500));
    catch_up(&h.state).await;
    tokio::time::sleep(Duration::from_secs(20)).await;
    h.fake.revoke_token();

    let activity = catch_up(&h.state).await;
    for id in [READ_CHANNEL, WRITE_CHANNEL, THIRD] {
        match part(&activity, id).currency {
            Currency::Held { failed, .. } => assert_eq!(failed, Some("chat_error"), "{id}"),
            Currency::Current => panic!("{id} was presented as current"),
        }
        assert!(
            part(&activity, id).read.is_ok(),
            "{id}: the held copy was not served"
        );
    }
    let text = text_of(&dispatch(&h.state, Scope::Read, &tool_call()).await);
    assert!(
        text.contains("the latest read of it failed (chat_error)"),
        "{text}"
    );
}

/// A channel with nothing held and a read still running is said to be not read in time — not an
/// error that fails the answer, and not a channel with no messages.
#[tokio::test(start_paused = true)]
async fn a_channel_with_nothing_held_says_it_was_not_read_in_time() {
    let h = harness();
    h.slow
        .stall(&channel(READ_CHANNEL), Duration::from_secs(58));
    let activity = catch_up(&h.state).await;
    assert!(matches!(
        part(&activity, READ_CHANNEL).read,
        Err(OpError::NotReadInTime)
    ));
    for id in [WRITE_CHANNEL, THIRD] {
        assert!(
            matches!(part(&activity, id).read, Err(OpError::NotReadInTime)),
            "{id}: queued behind the stall"
        );
    }
    let text = text_of(&dispatch(&h.state, Scope::Read, &tool_call()).await);
    assert!(
        text.contains("not read — the chat service did not answer in time"),
        "{text}"
    );
    assert!(
        text.contains("call recent_activity again in a moment"),
        "{text}"
    );
}

/// The live feed keeps a held window current: a message pushed after the read is in the next
/// catch-up, which reuses the window and asks the provider for nothing.
#[tokio::test(start_paused = true)]
async fn a_pushed_message_reaches_a_reused_window_without_a_read() {
    let h = harness();
    catch_up(&h.state).await;
    let before = h.slow.reads();
    let lead = channel(WRITE_CHANNEL);
    let id = h.fake.seed_at(&lead, "lead", "just landed", ops::now_ms());
    let pushed = h.fake.message(&id).expect("seeded");
    h.state.live.publish(&lead, pushed);

    let activity = catch_up(&h.state).await;
    assert_eq!(h.slow.reads(), before);
    assert_eq!(said(&activity, WRITE_CHANNEL), ["ship it", "just landed"]);
}

/// Opening a voice session starts the reads its first catch-up will want.
#[tokio::test(start_paused = true)]
async fn opening_a_voice_session_reads_the_channels_ahead_of_the_agent() {
    let h = harness();
    let app = vibe_talk::http::router(h.state.clone());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/voice-session")
                .header("authorization", format!("Bearer {WRITE_TOKEN}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK);
    tokio::time::sleep(3 * READ + Duration::from_millis(1)).await;
    assert_eq!(h.slow.reads(), 3);
    for id in [READ_CHANNEL, WRITE_CHANNEL, THIRD] {
        assert!(h.state.live.recent().holds(&channel(id)), "{id}");
    }
}
