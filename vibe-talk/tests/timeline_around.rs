//! `#2426 vibe-talk-sparse-loading-program`, slice A: a timeline read `around` one message,
//! `GET /api/v1/channels/{channel}/timeline?view=main&around=<id>&below=<n>&above=<n>`, through the
//! real router. It answers the message and its neighbours from this server's own copy: one stretch
//! the copy proves has no hole, oldest first, `more_below` and `more_above` honest at each end, and
//! a `next_before` the page's own walk back continues from without a gap or a repeat. It never
//! calls the provider, before the answer or behind it: each case below counts the calls.

use crate::slack_fake;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::{json, Value};
use tower::ServiceExt as _;
use vibe_talk::model::{ChannelId, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::state::AppState;
use vibe_talk::testing::{
    self, CountingChat, ManualClock, TempDir, TimelineChat, READ_CHANNEL, READ_TOKEN, WRITE_TOKEN,
};

/// 2026-08-18T12:00:00Z: the fixture's messages are written a minute apart from here.
const T0: i64 = 1_787_054_400_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// Hang backstop for the writer to settle; never a latency assertion.
const WAIT: Duration = Duration::from_secs(20);
/// How many messages the read channel holds.
const COUNT: usize = 30;
/// A table that keeps every day (`#595 cache-full-history`): with no age limit the copy's claims
/// reach the channel's start. The walk itself is not started in a test.
const FULL_HISTORY: &str = "\n[message_cache]\nbackfill = \"full\"\n";

async fn call(state: &AppState, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = vibe_talk::http::router(state.clone())
        .oneshot(request)
        .await
        .expect("the router answers");
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

async fn ok(state: &AppState, uri: &str, token: &str) -> Value {
    let (status, body) = call(state, uri, token).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    body
}

fn timeline(rest: &str) -> String {
    format!("/api/v1/channels/{READ_CHANNEL}/timeline?{rest}")
}

fn url_encode(raw: &str) -> String {
    raw.chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
            _ => format!("%{:02X}", u32::from(c)).chars().collect(),
        })
        .collect()
}

/// Wait for every shadow evaluation to be counted, and for the cache's writer.
async fn settled(state: &AppState) {
    assert!(
        state.message_cache.shadow_settled(WAIT).await,
        "every evaluation was counted"
    );
    if let Some(cache) = state.message_cache.cache() {
        assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    }
}

/// The texts an answer carries, oldest first.
fn texts(body: &Value) -> Vec<String> {
    body["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| {
            message["content"]
                .as_str()
                .expect("content is text")
                .to_owned()
        })
        .collect()
}

/// `message a` through `message b`, inclusive.
fn run(a: usize, b: usize) -> Vec<String> {
    (a..=b).map(|n| format!("message {n}")).collect()
}

/// A server whose read channel holds [`COUNT`] messages a minute apart, over a provider that
/// counts every call, with a full-history message cache behind it.
struct Deployment {
    state: AppState,
    counting: Arc<CountingChat>,
    clock: Arc<ManualClock>,
    ids: Vec<MessageId>,
    _dir: TempDir,
}

async fn deployment(tag: &str) -> Deployment {
    let dir = TempDir::new(tag);
    let (mut state, fake, _voice) =
        testing::state_from_toml(&format!("{}{FULL_HISTORY}", testing::config_toml()));
    let read = ChannelId(READ_CHANNEL.to_owned());
    let ids = (0..COUNT)
        .map(|n| {
            let at = T0 + i64::try_from(n).expect("small") * MINUTE;
            fake.seed_at(&read, "Ada", &format!("message {n}"), at)
        })
        .collect();
    let counting = Arc::new(CountingChat::new(Arc::new(TimelineChat::new(fake))));
    state.replace_chat(counting.clone());
    let clock = ManualClock::at(T0 + HOUR);
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    Deployment {
        state,
        counting,
        clock,
        ids,
        _dir: dir,
    }
}

impl Deployment {
    /// A provider read of Main's newest `limit`, with serving off: what fills the copy.
    async fn read_main(&self, limit: u16) {
        ok(
            &self.state,
            &timeline(&format!("view=main&limit={limit}")),
            WRITE_TOKEN,
        )
        .await;
        settled(&self.state).await;
    }

    /// The whole channel in the copy, from its start (its newest page holds it all), read again
    /// past the unsettled band so the claims reach its newest message; then serving on.
    async fn filled(tag: &str) -> Self {
        let d = deployment(tag).await;
        d.read_main(50).await;
        d.clock.advance(11 * MINUTE);
        d.read_main(50).await;
        d.state.message_cache.set_serve(true);
        d
    }

    /// An around read as `token`, requiring success and that no provider call began.
    async fn around_as(&self, rest: &str, token: &str) -> Value {
        let before = self.counting.calls().len();
        let body = ok(&self.state, &timeline(rest), token).await;
        settled(&self.state).await;
        let calls: Vec<&str> = self.counting.calls()[before..]
            .iter()
            .map(|call| call.method)
            .collect();
        assert!(calls.is_empty(), "{rest} called the provider: {calls:?}");
        body
    }

    async fn around(&self, rest: &str) -> Value {
        self.around_as(rest, WRITE_TOKEN).await
    }
}

/// The served backward cursor an answer carries.
fn served_cursor(body: &Value) -> String {
    let cursor = body["next_before"]
        .as_str()
        .unwrap_or_else(|| panic!("a backward cursor: {body}"))
        .to_owned();
    assert!(
        cursor.starts_with(vibe_talk::timeline_served::PREFIX),
        "the server's own cursor: {body}"
    );
    cursor
}

#[tokio::test]
async fn around_answers_a_message_and_its_neighbours_from_the_copy_alone() {
    let d = Deployment::filled("around-middle").await;
    let target = &d.ids[15];
    let body = d
        .around(&format!("view=main&around={target}&below=3&above=4"))
        .await;
    assert_eq!(texts(&body), run(12, 19), "{body}");
    assert_eq!(
        body["around"],
        json!({
            "target": target.as_str(),
            "held": true,
            "below": 3,
            "above": 4,
            "more_below": true,
            "more_above": true,
        }),
        "{body}"
    );
    assert_eq!(body["view"], "main");
    assert_eq!(body["returned"], 8);
    assert_eq!(body["has_more"], true, "{body}");
    served_cursor(&body);
    assert!(body.get("refresh_failed").is_none(), "{body}");
    assert!(body.get("next_after").is_none(), "{body}");

    // Ten each way when the counts are left out (the owner's choice O3).
    let defaults = d.around(&format!("view=main&around={target}")).await;
    assert_eq!(texts(&defaults), run(5, 25), "{defaults}");
    assert_eq!(defaults["around"]["below"], 10);
    assert_eq!(defaults["around"]["above"], 10);

    // The read token is answered from the copy too: the read writes nothing and begins nothing.
    let read = d
        .around_as(
            &format!("view=main&around={target}&below=3&above=4"),
            READ_TOKEN,
        )
        .await;
    assert_eq!(texts(&read), run(12, 19), "{read}");
}

#[tokio::test]
async fn at_the_channels_start_and_at_its_newest_message_there_is_nothing_more() {
    let d = Deployment::filled("around-edges").await;
    let oldest = d
        .around(&format!("view=main&around={}&below=5&above=2", d.ids[0]))
        .await;
    assert_eq!(texts(&oldest), run(0, 2), "{oldest}");
    assert_eq!(oldest["around"]["more_below"], false, "{oldest}");
    assert_eq!(oldest["around"]["more_above"], true, "{oldest}");
    assert_eq!(oldest["has_more"], false, "{oldest}");
    assert!(oldest["next_before"].is_null(), "{oldest}");

    let newest = d
        .around(&format!("view=main&around={}&below=2&above=5", d.ids[29]))
        .await;
    assert_eq!(texts(&newest), run(27, 29), "{newest}");
    assert_eq!(newest["around"]["more_above"], false, "{newest}");
    assert_eq!(newest["around"]["more_below"], true, "{newest}");

    // A walk up is an around read at the newest message held, with nothing below.
    let up = d
        .around(&format!("view=main&around={}&below=0&above=10", d.ids[26]))
        .await;
    assert_eq!(texts(&up), run(26, 29), "{up}");
    assert_eq!(up["around"]["more_above"], false, "{up}");
    assert_eq!(up["around"]["more_below"], true, "{up}");
}

#[tokio::test]
async fn the_pages_walk_back_continues_from_an_around_answer_without_a_gap_or_a_repeat() {
    let d = Deployment::filled("around-walk").await;
    let body = d
        .around(&format!("view=main&around={}&below=3&above=0", d.ids[15]))
        .await;
    let mut seen = texts(&body);
    assert_eq!(seen, run(12, 15), "{body}");
    let mut cursor = served_cursor(&body);
    for _ in 0..4 {
        let step = ok(
            &d.state,
            &timeline(&format!("view=main&limit=5&before={}", url_encode(&cursor))),
            WRITE_TOKEN,
        )
        .await;
        settled(&d.state).await;
        let mut older = texts(&step);
        older.extend(seen);
        seen = older;
        if step["has_more"] == false {
            break;
        }
        cursor = served_cursor(&step);
    }
    assert_eq!(seen, run(0, 15), "every message below once, in order");
}

#[tokio::test]
async fn a_partly_proven_copy_answers_only_what_it_proves() {
    // The copy holds the newest ten only, its newest end's band unsettled: below message 20 it
    // proves nothing, and its claims stop short of the band.
    let d = deployment("around-partial").await;
    d.read_main(10).await;
    d.state.message_cache.set_serve(true);

    // Both neighbours below are covered up to the target, but nothing proves the copy holds
    // all that is older: served, with more below.
    let body = d
        .around(&format!("view=main&around={}&below=5&above=2", d.ids[22]))
        .await;
    assert_eq!(texts(&body), run(20, 24), "{body}");
    assert_eq!(body["around"]["more_below"], true, "{body}");
    assert_eq!(body["around"]["more_above"], true, "{body}");

    // In the unsettled band no claim holds either side: the target alone.
    let banded = d
        .around(&format!("view=main&around={}&below=2&above=2", d.ids[28]))
        .await;
    assert_eq!(texts(&banded), run(28, 28), "{banded}");
    assert_eq!(banded["around"]["held"], true, "{banded}");
    assert_eq!(banded["around"]["more_below"], true, "{banded}");
    assert_eq!(banded["around"]["more_above"], true, "{banded}");
    served_cursor(&banded);

    // Older than anything held: unheld, and nothing is fetched for it.
    let unheld = d.around(&format!("view=main&around={}", d.ids[5])).await;
    assert_eq!(unheld["around"]["held"], false, "{unheld}");
    assert!(texts(&unheld).is_empty(), "{unheld}");
}

#[tokio::test]
async fn a_message_the_copy_cannot_answer_for_is_answered_unheld() {
    let d = Deployment::filled("around-unheld").await;
    let absent = d.around("view=main&around=404404&below=3&above=3").await;
    assert_eq!(
        absent["around"],
        json!({
            "target": "404404",
            "held": false,
            "below": 3,
            "above": 3,
            "more_below": true,
            "more_above": true,
        }),
        "{absent}"
    );
    assert!(texts(&absent).is_empty(), "{absent}");
    assert_eq!(absent["has_more"], false, "{absent}");
    assert!(absent["next_before"].is_null(), "{absent}");

    // With serving off the copy is not consulted at all, and still nothing is fetched.
    d.state.message_cache.set_serve(false);
    let off = d.around(&format!("view=main&around={}", d.ids[15])).await;
    assert_eq!(off["around"]["held"], false, "{off}");
    assert!(texts(&off).is_empty(), "{off}");
}

#[tokio::test]
async fn an_all_view_the_copy_keeps_no_order_of_answers_the_message_alone() {
    // Discord's All view is no scope of the copy's: the target, and more both ways. Its cursor is
    // bound to the view it was read in.
    let d = Deployment::filled("around-flat").await;
    let body = d
        .around(&format!("view=flat&around={}&below=3&above=3", d.ids[15]))
        .await;
    assert_eq!(texts(&body), run(15, 15), "{body}");
    assert_eq!(body["view"], "flat");
    assert_eq!(body["around"]["held"], true, "{body}");
    assert_eq!(body["around"]["more_below"], true, "{body}");
    assert_eq!(body["around"]["more_above"], true, "{body}");
    let cursor = served_cursor(&body);
    let (status, refused) = call(
        &d.state,
        &timeline(&format!("view=main&limit=5&before={}", url_encode(&cursor))),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
}

#[tokio::test]
async fn the_counts_each_way_are_clamped_to_the_page_cap() {
    let d = Deployment::filled("around-clamp").await;
    let cap = vibe_talk::ops::MAX_PAGE;
    let body = d
        .around(&format!(
            "view=main&around={}&below=500&above=60000",
            d.ids[15]
        ))
        .await;
    assert_eq!(body["around"]["below"], cap, "{body}");
    assert_eq!(body["around"]["above"], cap, "{body}");
    assert_eq!(body["limit"], 2 * cap + 1, "{body}");
    assert_eq!(texts(&body), run(0, COUNT - 1), "{body}");
    assert_eq!(body["around"]["more_below"], false, "{body}");
    assert_eq!(body["around"]["more_above"], false, "{body}");
}

#[tokio::test]
async fn around_is_refused_beside_a_cursor_outside_main_and_flat_or_without_a_message() {
    let d = Deployment::filled("around-refused").await;
    let target = d.ids[15].clone();
    for rest in [
        format!("view=main&around={target}&before=vt-served.x"),
        format!("view=main&around={target}&after=forward"),
        format!("view=thread&thread_id={target}&around={target}"),
        format!("view=threads&around={target}"),
        "view=main&around=".to_owned(),
        "view=main&around=%20".to_owned(),
        "view=main&below=3".to_owned(),
        "view=main&limit=5&above=3".to_owned(),
        format!("view=main&around={target}&below=-1"),
    ] {
        let before = d.counting.calls().len();
        let (status, body) = call(&d.state, &timeline(&rest), WRITE_TOKEN).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{rest}: {body}");
        assert_eq!(
            d.counting.calls().len(),
            before,
            "{rest} called the provider"
        );
    }
}

// --- Slack's All: Main's stretch with the replies the copy proves -------------------------------

/// The Slack provider's key in [`slack_all_deployment`].
const SLACK: &str = "slack";
/// The Slack channel [`slack_all_deployment`] reads.
pub(crate) const SLACK_ALL_CH: &str = "CAROUND01";

/// Slack time `seconds` into the fixture, in milliseconds.
fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1_000
}

/// A Slack channel of eight messages a minute apart, `message 0` to `message 7`, with three
/// threads: on `message 2` (`reply 2a`, `reply 2b`) and on `message 5` (`late reply 5`, written
/// after `message 7`), both read as threads, so the copy proves their replies; and on `message 4`
/// (`reply 4a`), whose replies only an All read brought, with no listing of the thread behind
/// them. Main is read again past its unsettled band, the copy keeps every day, and serving is on.
pub(crate) async fn slack_all_deployment(
    tag: &str,
) -> (AppState, slack_fake::Fake, Arc<ManualClock>, TempDir) {
    let mut fixture = slack_fake::FakeState::default();
    let mut history: Vec<Value> = (0..8)
        .map(|n| {
            slack_fake::profiled(
                &slack_fake::ts(n * 60, 0),
                "UADA00001",
                "Ada",
                &format!("message {n}"),
            )
        })
        .collect();
    for (root, replies) in [
        (2, vec![(130, "reply 2a"), (140, "reply 2b")]),
        (4, vec![(250, "reply 4a")]),
        (5, vec![(450, "late reply 5")]),
    ] {
        let replies: Vec<(String, String)> = replies
            .into_iter()
            .map(|(at, text)| (slack_fake::ts(at, 0), text.to_owned()))
            .collect();
        slack_fake::thread(&mut fixture, SLACK_ALL_CH, &mut history[root], &replies);
    }
    fixture.history.insert(SLACK_ALL_CH.to_owned(), history);
    let fake = slack_fake::Fake::start(fixture, 50).await;
    let dir = TempDir::new(tag);
    let clock = ManualClock::at(slack_ms(3_600));
    let text = format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{SLACK}"
kind = "slack"
api_base = "{base}"
token = "{token}"

[[channels]]
id = "{SLACK_ALL_CH}"
label = "team"
writable = true
{FULL_HISTORY}"#,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    let state = testing::with_message_cache(state, dir.path(), clock.clone()).await;
    let read = |rest: String| {
        let state = state.clone();
        async move {
            ok(
                &state,
                &format!("/api/v1/channels/{SLACK_ALL_CH}/timeline?{rest}"),
                WRITE_TOKEN,
            )
            .await;
            settled(&state).await;
        }
    };
    // All first: it brings every thread's replies, as rows no listing of their thread stands
    // behind. Then Main, and the two threads a reader opened.
    read("view=flat&limit=50".to_owned()).await;
    read("view=main&limit=50".to_owned()).await;
    for root in [2, 5] {
        read(format!(
            "view=thread&thread_id={}&limit=50",
            slack_fake::ts(root * 60, 0)
        ))
        .await;
    }
    clock.advance(11 * MINUTE);
    read("view=main&limit=50".to_owned()).await;
    state.message_cache.set_serve(true);
    (state, fake, clock, dir)
}

/// The id of the Slack fixture's message written `seconds` in.
fn slack_id(seconds: i64) -> String {
    vibe_talk::slack::ids::message_id_from_ts(&slack_fake::ts(seconds, 0))
        .expect("a Slack ts")
        .0
}

/// Every call the Slack fake has answered for a read.
fn slack_reads(fake: &slack_fake::Fake) -> usize {
    fake.calls("conversations.history") + fake.calls("conversations.replies")
}

/// An around read of the Slack channel, requiring success and that no Slack read began.
pub(crate) async fn slack_around(state: &AppState, fake: &slack_fake::Fake, rest: &str) -> Value {
    let before = slack_reads(fake);
    let body = ok(
        state,
        &format!("/api/v1/channels/{SLACK_ALL_CH}/timeline?{rest}"),
        WRITE_TOKEN,
    )
    .await;
    settled(state).await;
    assert_eq!(slack_reads(fake), before, "{rest} read Slack");
    body
}

#[tokio::test]
async fn slack_s_all_around_carries_the_replies_it_proves_and_names_the_threads_it_cannot() {
    let (state, fake, _clock, _dir) = slack_all_deployment("around-slack-all").await;
    let body = slack_around(
        &state,
        &fake,
        &format!("view=flat&around={}&below=2&above=2", slack_id(180)),
    )
    .await;
    // Main's stretch, message 1 to message 5; thread 2's replies in it, and thread 5's late reply
    // with its root though it is dated after the stretch; thread 4's replies left out, named.
    assert_eq!(
        texts(&body),
        [
            "message 1",
            "message 2",
            "reply 2a",
            "reply 2b",
            "message 3",
            "message 4",
            "message 5",
            "late reply 5",
        ],
        "{body}"
    );
    assert_eq!(
        body["around"],
        json!({
            "target": slack_id(180),
            "held": true,
            "below": 2,
            "above": 2,
            "more_below": true,
            "more_above": true,
            "replies_left_out": [slack_fake::ts(240, 0)],
        }),
        "{body}"
    );
    assert_eq!(body["view"], "flat");
    assert_eq!(body["has_threads"], true, "{body}");
    served_cursor(&body);

    // Main itself: the stretch alone, no replies, nothing left out.
    let main = slack_around(
        &state,
        &fake,
        &format!("view=main&around={}&below=2&above=2", slack_id(180)),
    )
    .await;
    assert_eq!(texts(&main), run(1, 5), "{main}");
    assert!(main["around"].get("replies_left_out").is_none(), "{main}");
}

#[tokio::test]
async fn a_reply_asked_about_in_slack_s_all_is_placed_with_its_root_or_answered_alone() {
    let (state, fake, _clock, _dir) = slack_all_deployment("around-slack-reply").await;
    // A reply of a proven thread: Main's stretch around its root, with the thread's replies.
    let proven = slack_around(
        &state,
        &fake,
        &format!("view=flat&around={}&below=1&above=1", slack_id(140)),
    )
    .await;
    assert_eq!(
        texts(&proven),
        [
            "message 1",
            "message 2",
            "reply 2a",
            "reply 2b",
            "message 3"
        ],
        "{proven}"
    );
    assert_eq!(proven["around"]["held"], true, "{proven}");
    assert_eq!(proven["around"]["target"], slack_id(140), "{proven}");

    // A reply of a thread the copy cannot prove: the reply alone, more both ways.
    let alone = slack_around(
        &state,
        &fake,
        &format!("view=flat&around={}&below=1&above=1", slack_id(250)),
    )
    .await;
    assert_eq!(texts(&alone), ["reply 4a"], "{alone}");
    assert_eq!(alone["around"]["held"], true, "{alone}");
    assert_eq!(alone["around"]["more_below"], true, "{alone}");
    assert_eq!(alone["around"]["more_above"], true, "{alone}");
    assert_eq!(alone["has_threads"], true, "{alone}");
}

#[tokio::test]
async fn slack_s_all_walks_back_from_an_around_answer_as_from_any_page_from_the_copy() {
    let (state, fake, _clock, _dir) = slack_all_deployment("around-slack-walk").await;
    let body = slack_around(
        &state,
        &fake,
        &format!("view=flat&around={}&below=1&above=0", slack_id(300)),
    )
    .await;
    let mut seen = texts(&body);
    assert_eq!(seen, ["message 4", "message 5", "late reply 5"], "{body}");
    let mut cursor = served_cursor(&body);
    for _ in 0..5 {
        let step = ok(
            &state,
            &format!(
                "/api/v1/channels/{SLACK_ALL_CH}/timeline?view=flat&limit=2&before={}",
                url_encode(&cursor)
            ),
            WRITE_TOKEN,
        )
        .await;
        settled(&state).await;
        let mut older = texts(&step);
        older.extend(seen);
        seen = older;
        if step["has_more"] == false {
            break;
        }
        cursor = served_cursor(&step);
    }
    assert_eq!(
        seen,
        [
            "message 0",
            "message 1",
            "message 2",
            "reply 2a",
            "reply 2b",
            "message 3",
            "message 4",
            "message 5",
            "late reply 5",
        ],
        "every message once, each reply with its root"
    );
}
