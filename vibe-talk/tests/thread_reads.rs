//! remote-ops-audit W14 `#287 bulk-reads`: the newest messages of several threads of one channel in
//! one call.
//!
//! The first test pins what naming threads (`#182 thread-display-names`) reads upstream, and what
//! it names them, on a provider that overrides nothing but the methods every provider must write
//! and the thread view: twenty due threads of one channel, the same list again with nothing due,
//! two channels listed one after the other, three threads that grew, a channel where one thread
//! cannot be read, and a read token listing due threads. Its fixture was recorded by the code as
//! it stood BEFORE `ChatClient::fetch_threads` existed, so it is the proof that a provider without
//! a read of several threads of its own still gets those names byte for byte, at the same cost, in
//! the same order, now that the trait has one. Rewrite it only for a change that is MEANT to alter
//! what such a provider is asked or answers:
//! `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test thread_reads`.
//!
//! Naming reads each channel at most once per `summaries.thread_naming_interval_seconds`, takes a
//! channel's due threads in its own order, and pauses a channel whose read failed at the provider
//! (`#286 naming-budget`). So the test runs on tokio's paused clock at the default interval, and
//! the fixture records that order and that pause, as the code before `fetch_threads` made them.

mod slack_fake;

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use vibe_talk::chat::{
    newer_than_trimmed, ChatClient, ChatError, ChatIdentity, ReadCaps, Support, ThreadAnswers,
    ThreadAsk, THREADS_IN_FLIGHT,
};
use vibe_talk::model::{ChannelId, ChannelInfo, Message, MessageId, UserId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::store::fake::FakeStore;
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::{ThreadDeps, ThreadNames};
use vibe_talk::testing::CountingChat;
use vibe_talk::threads::{
    ThreadSummary, TimelineDelta, TimelinePage, TimelineRequest, TimelineView,
};
use vibe_talk::upstream_profile::{ProfiledClient, UpstreamProfile};

/// When every thread of the test starts, in Unix milliseconds.
const BASE_MS: i64 = 1_791_000_000_000;
const MINUTE_MS: i64 = 60_000;

fn channel(id: &str) -> ChannelId {
    ChannelId(id.to_owned())
}

fn iso(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .expect("in range")
        .to_string()
}

/// One thread upstream: its root and `replies` replies, a minute apart from `start_ms`.
#[derive(Clone, Debug)]
struct Thread {
    id: String,
    replies: u64,
    start_ms: i64,
}

impl Thread {
    fn message(&self, channel: &ChannelId, n: u64) -> Message {
        let (id, content) = if n == 0 {
            (
                format!("{}-root", self.id),
                format!("{} starts here", self.id),
            )
        } else {
            (
                format!("{}-r{n}", self.id),
                format!("reply {n} in {}", self.id),
            )
        };
        Message {
            edited_timestamp: None,
            thread: None,
            id: MessageId(id),
            channel_id: channel.clone(),
            author: "claude-integ".to_owned(),
            author_id: UserId("7".to_owned()),
            author_is_bot: false,
            content,
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            timestamp: iso(self.start_ms + i64::try_from(n).expect("small") * MINUTE_MS),
            spoken_time: String::new(),
            reply_to: None,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        }
    }

    fn summary(&self, channel: &ChannelId) -> ThreadSummary {
        ThreadSummary {
            hue: None,
            id: self.id.clone(),
            root: Some(self.message(channel, 0)),
            title: format!("{} starts here", self.id),
            reply_count: Some(self.replies),
            reply_count_exact: true,
            updated_at: iso(self.start_ms + i64::try_from(self.replies).expect("small") * MINUTE_MS),
            display_name: None,
            summary: None,
        }
    }

    /// The thread view's newest page of `limit`: root and replies, oldest first.
    fn page(&self, channel: &ChannelId, limit: u16) -> TimelinePage {
        let all: Vec<Message> = (0..=self.replies)
            .map(|n| self.message(channel, n))
            .collect();
        let keep = usize::from(limit).min(all.len());
        TimelinePage {
            has_more: all.len() > keep,
            messages: all[all.len() - keep..].to_vec(),
            thread: Some(self.summary(channel)),
            has_threads: true,
            ..TimelinePage::default()
        }
    }
}

/// A provider that writes only what the trait requires and the thread view, and logs every call
/// that may reach upstream. Everything else runs the trait's default body on THIS client, which is
/// the case the fixture pins: a provider with no read of several threads of its own.
#[derive(Default)]
struct Plain {
    threads: Mutex<BTreeMap<String, Vec<Thread>>>,
    /// Threads whose reads fail, as a provider's do when it cannot reach one.
    failing: Mutex<HashSet<String>>,
    log: Mutex<Vec<String>>,
}

impl Plain {
    fn note(&self, line: String) {
        self.log.lock().expect("log").push(line);
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().expect("log"))
    }

    /// Give `channel` threads named `<prefix>00`, `<prefix>01`, … with the given reply counts.
    fn seed(&self, channel: &ChannelId, prefix: &str, replies: &[u64]) {
        let threads = replies
            .iter()
            .enumerate()
            .map(|(n, replies)| Thread {
                id: format!("{prefix}{n:02}"),
                replies: *replies,
                start_ms: BASE_MS + i64::try_from(n).expect("small") * 10 * MINUTE_MS,
            })
            .collect();
        self.threads
            .lock()
            .expect("threads")
            .insert(channel.0.clone(), threads);
    }

    fn grow(&self, thread: &str, replies: u64) {
        for list in self.threads.lock().expect("threads").values_mut() {
            for found in list.iter_mut().filter(|t| t.id == thread) {
                found.replies = replies;
            }
        }
    }

    fn fail(&self, thread: &str) {
        self.failing
            .lock()
            .expect("failing")
            .insert(thread.to_owned());
    }

    /// The thread view's newest page of `limit` of thread `id`, as upstream holds it now, without
    /// logging: what every read of one thread here answers.
    fn read(&self, channel: &ChannelId, id: &str, limit: u16) -> Result<TimelinePage, ChatError> {
        if self.failing.lock().expect("failing").contains(id) {
            return Err(ChatError::Status {
                status: 502,
                body: "the thread is unreachable".to_owned(),
            });
        }
        let threads = self.threads.lock().expect("threads");
        threads
            .get(channel.as_str())
            .and_then(|list| list.iter().find(|t| t.id == id))
            .map(|thread| thread.page(channel, limit))
            .ok_or_else(|| ChatError::Status {
                status: 404,
                body: "no such thread in this channel".to_owned(),
            })
    }

    /// The thread list of `channel`, as a timeline answer, without reading upstream: the page a
    /// test hands naming, as the timeline route would.
    fn list(&self, channel: &ChannelId) -> TimelinePage {
        let threads = self.threads.lock().expect("threads");
        TimelinePage {
            threads: threads
                .get(channel.as_str())
                .map(|list| list.iter().map(|t| t.summary(channel)).collect())
                .unwrap_or_default(),
            has_threads: true,
            ..TimelinePage::default()
        }
    }
}

#[async_trait]
impl ChatClient for Plain {
    fn supports_threading(&self) -> bool {
        true
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        self.note(format!(
            "fetch_timeline {channel} view={} thread={} limit={} before={} after={}",
            request.view.as_str(),
            request.thread_id.as_deref().unwrap_or("-"),
            request.limit,
            request.before.as_deref().unwrap_or("-"),
            request.after.as_deref().unwrap_or("-"),
        ));
        if request.view != TimelineView::Thread {
            return Err(ChatError::Refused(
                "this test reads threads only".to_owned(),
            ));
        }
        self.read(
            channel,
            request.thread_id.as_deref().unwrap_or_default(),
            request.limit,
        )
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.note("identity".to_owned());
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "reader".to_owned(),
            workspace: None,
        })
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        _before: Option<&MessageId>,
        _after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.note(format!("fetch_page {channel} limit={limit}"));
        Ok(Vec::new())
    }

    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        Err(ChatError::Refused("this test posts nothing".to_owned()))
    }
}

/// What naming needs, reading through `chat`.
fn deps(chat: Arc<dyn ChatClient>, summarizer: Arc<FakeSummarizer>) -> ThreadDeps {
    ThreadDeps {
        chat,
        store: Arc::new(FakeStore::new()),
        summarizer,
        version: Arc::from("thread-reads-test"),
        sentence_chars: 160,
    }
}

/// The thread list of `channel` as naming sees it from a caller with write scope (`may_write`) or
/// without: annotated, and the due threads queued.
async fn list(
    names: &Arc<ThreadNames>,
    deps: &ThreadDeps,
    plain: &Plain,
    channel: &ChannelId,
    may_write: bool,
) -> TimelinePage {
    let request = TimelineRequest {
        view: TimelineView::Threads,
        limit: 50,
        ..TimelineRequest::default()
    };
    let mut page = plain.list(channel);
    names
        .annotate(deps, channel, &request, &mut page, may_write)
        .await;
    page
}

/// Wait until nothing is queued or being named, and `named` namings have completed in all: up to
/// two hours of tokio's paused clock, long enough for each channel's naming interval and the pause
/// after a failed read (`#286 naming-budget`).
async fn settle(names: &ThreadNames, named: usize) {
    for _ in 0..7_200 {
        if names.pending() == 0 && names.named() >= named {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!(
        "naming never settled at {named}: {} named, {} pending",
        names.named(),
        names.pending()
    );
}

/// Each thread of a page with the name and sentence naming attached to it.
fn shown(page: &TimelinePage) -> Value {
    page.threads
        .iter()
        .map(|thread| json!([thread.id, thread.display_name, thread.summary]))
        .collect()
}

/// Every case, in order on one server: every upstream call, every prompt the summariser was given,
/// and the names a later read of each channel shows.
async fn record() -> Value {
    let plain = Arc::new(Plain::default());
    let summarizer = Arc::new(FakeSummarizer::naming_threads());
    let deps = deps(
        Arc::clone(&plain) as Arc<dyn ChatClient>,
        Arc::clone(&summarizer),
    );
    let names = Arc::new(ThreadNames::new());
    let (a, b, c, d, e) = (
        channel("A0000000001"),
        channel("B0000000002"),
        channel("C0000000003"),
        channel("D0000000004"),
        channel("E0000000005"),
    );
    // Every thread at the summary threshold or past it (`#484 summary-card-threshold`): due when
    // unnamed, as this record's cases say.
    let replies: Vec<u64> = (0..20).map(|n| 2 + n % 4).collect();
    plain.seed(&a, "a", &replies);
    plain.seed(&b, "b", &[3, 2, 4]);
    plain.seed(&c, "c", &[5, 3]);
    plain.seed(&d, "d", &[3, 4, 2]);
    plain.seed(&e, "e", &[2, 3]);
    plain.fail("d01");

    let mut cases = Vec::new();
    let mut prompts_seen = 0;
    let mut case = |name: &str, upstream: Vec<String>, shown: Value| {
        let prompts = summarizer.thread_prompts();
        let fresh = prompts[prompts_seen..].to_vec();
        prompts_seen = prompts.len();
        cases.push(json!({
            "case": name,
            "upstream": upstream,
            "prompts": fresh,
            "shown": shown,
        }));
    };

    list(&names, &deps, &plain, &a, true).await;
    settle(&names, 20).await;
    let upstream = plain.take();
    let after = list(&names, &deps, &plain, &a, false).await;
    case(
        "a channel whose twenty threads are all due",
        upstream,
        shown(&after),
    );

    let again = list(&names, &deps, &plain, &a, true).await;
    settle(&names, 20).await;
    case(
        "the same channel listed again with nothing due",
        plain.take(),
        shown(&again),
    );

    list(&names, &deps, &plain, &b, true).await;
    list(&names, &deps, &plain, &c, true).await;
    settle(&names, 25).await;
    let upstream = plain.take();
    let shown_b = list(&names, &deps, &plain, &b, false).await;
    let shown_c = list(&names, &deps, &plain, &c, false).await;
    case(
        "two channels listed one after the other",
        upstream,
        json!([shown(&shown_b), shown(&shown_c)]),
    );

    // Filed at 3 messages a thread is due again at 4; filed at 5 or 6, at 8 (3, 4, 8, then
    // every 10): a00 was filed at 3, a02 at 5, a03 at 6.
    plain.grow("a00", 3);
    plain.grow("a02", 7);
    plain.grow("a03", 7);
    list(&names, &deps, &plain, &a, true).await;
    settle(&names, 28).await;
    let upstream = plain.take();
    let after = list(&names, &deps, &plain, &a, false).await;
    case(
        "three threads of the first channel grew",
        upstream,
        shown(&after),
    );

    list(&names, &deps, &plain, &d, true).await;
    settle(&names, 30).await;
    let upstream = plain.take();
    let after = list(&names, &deps, &plain, &d, false).await;
    case(
        "a channel where one thread cannot be read",
        upstream,
        shown(&after),
    );

    let read_only = list(&names, &deps, &plain, &e, false).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    settle(&names, 30).await;
    case(
        "a read token lists a channel with due threads",
        plain.take(),
        shown(&read_only),
    );
    Value::Array(cases)
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/thread_reads_default.json")
}

#[tokio::test(start_paused = true)]
async fn a_provider_without_a_read_of_several_threads_gets_todays_names_at_todays_cost() {
    let recorded = record().await;
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(&recorded).expect("serializes")
    );
    let path = fixture_path();
    if std::env::var_os("VIBE_TALK_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("mkdir");
        std::fs::write(&path, &text).expect("write the fixture");
        return;
    }
    let held = std::fs::read_to_string(&path).expect("the recorded fixture");
    let held: Value = serde_json::from_str(&held).expect("the fixture is JSON");
    let (held, recorded) = (
        held.as_array().expect("cases"),
        recorded.as_array().expect("cases"),
    );
    assert_eq!(held.len(), recorded.len(), "the same cases");
    for (was, now) in held.iter().zip(recorded) {
        assert_eq!(
            was, now,
            "{} no longer reads upstream or names threads as it did before reads of several \
             threads existed",
            was["case"]
        );
    }
    // The fixture is not vacuous: every due thread was read once, by the thread view, and named
    // from that read; a channel's threads newest activity first, and two channels turn about, each
    // at its own interval (`#286 naming-budget`).
    let case = |name: &str| -> &Value {
        held.iter()
            .find(|c| c["case"] == name)
            .unwrap_or_else(|| panic!("no case {name}"))
    };
    let upstream = |name: &str| -> Vec<String> {
        case(name)["upstream"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("a line").to_owned())
            .collect()
    };
    let twenty = upstream("a channel whose twenty threads are all due");
    assert_eq!(twenty.len(), 20, "{twenty:?}");
    for (n, line) in twenty.iter().enumerate() {
        assert_eq!(
            *line,
            format!(
                "fetch_timeline A0000000001 view=thread thread=a{:02} limit=50 before=- after=-",
                19 - n
            )
        );
    }
    assert_eq!(
        case("a channel whose twenty threads are all due")["prompts"]
            .as_array()
            .expect("prompts")
            .len(),
        20
    );
    assert!(upstream("the same channel listed again with nothing due").is_empty());
    let two = upstream("two channels listed one after the other");
    assert_eq!(two.len(), 5, "{two:?}");
    for (n, line) in two.iter().enumerate() {
        let channel = if n % 2 == 0 {
            "B0000000002"
        } else {
            "C0000000003"
        };
        assert!(line.contains(&format!(" {channel} ")), "{two:?}");
    }
    assert_eq!(upstream("three threads of the first channel grew").len(), 3);
    let failing = upstream("a channel where one thread cannot be read");
    assert_eq!(failing.len(), 3, "{failing:?}");
    let failed = case("a channel where one thread cannot be read")["shown"].to_string();
    assert!(failed.contains(r#"["d01",null,null]"#), "{failed}");
    assert!(upstream("a read token lists a channel with due threads").is_empty());
}

// --- the method, its default and the router ------------------------------------------------------

/// A provider that reads several threads of a channel itself, in one call: each ask is answered
/// from the pages `plain` holds, as one thread view read of it would be, and only the call is
/// logged, as `fetch_threads <channel> <ids>`.
struct Native {
    plain: Arc<Plain>,
    asked: Mutex<Vec<Vec<String>>>,
}

impl Native {
    fn new(plain: Arc<Plain>) -> Self {
        Self {
            plain,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<Vec<String>> {
        self.asked.lock().expect("asked").clone()
    }
}

#[async_trait]
impl ChatClient for Native {
    fn supports_threading(&self) -> bool {
        true
    }

    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            many_threads: Support::Native,
            ..ReadCaps::default()
        }
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        self.plain.fetch_timeline(channel, request).await
    }

    async fn fetch_threads(&self, channel: &ChannelId, asks: &[ThreadAsk]) -> ThreadAnswers {
        let ids: Vec<String> = asks.iter().map(|ask| ask.id.clone()).collect();
        self.plain
            .note(format!("fetch_threads {channel} {}", ids.join(",")));
        self.asked.lock().expect("asked").push(ids);
        asks.iter()
            .map(|ask| {
                (
                    ask.id.clone(),
                    self.plain
                        .read(channel, &ask.id, ask.limit)
                        .map(|page| newer_than_trimmed(page, ask.newer_than_ms)),
                )
            })
            .collect()
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.plain.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.plain.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.plain.post_message(channel, content, reply_to).await
    }
}

/// `Plain`, answering every thread view read after a pause, and counting how many are in flight.
struct Slow {
    plain: Arc<Plain>,
    in_flight: AtomicUsize,
    most: AtomicUsize,
}

#[async_trait]
impl ChatClient for Slow {
    fn supports_threading(&self) -> bool {
        true
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        let answer = self.plain.fetch_timeline(channel, request).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        answer
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.plain.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.plain.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.plain.post_message(channel, content, reply_to).await
    }
}

fn ids(page: &TimelinePage) -> Vec<String> {
    page.messages.iter().map(|m| m.id.0.clone()).collect()
}

/// The instant reply `n` of thread `n_thread` of a channel seeded by [`Plain::seed`] was created.
fn reply_ms(n_thread: i64, n: i64) -> i64 {
    BASE_MS + n_thread * 10 * MINUTE_MS + n * MINUTE_MS
}

/// remote-ops-audit W14: the default is one thread view read per ask, its own size, two in flight,
/// started in the order asked; a repeated ask is read again, and a thread that cannot be read is
/// an error in its own place while the others are answered.
#[tokio::test(start_paused = true)]
async fn the_default_reads_one_thread_view_per_ask_two_at_a_time_in_order() {
    let plain = Arc::new(Plain::default());
    let a = channel("A0000000001");
    plain.seed(&a, "a", &[0, 1, 2, 3]);
    let slow = Slow {
        plain: Arc::clone(&plain),
        in_flight: AtomicUsize::new(0),
        most: AtomicUsize::new(0),
    };
    let asks = [
        ThreadAsk::newest("a00", 50),
        ThreadAsk::newest("a03", 2),
        ThreadAsk::newest("zz", 50),
        ThreadAsk::newest("a00", 50),
        ThreadAsk::newest("a02", 50),
    ];
    let answers = slow.fetch_threads(&a, &asks).await;

    assert_eq!(
        plain.take(),
        [
            "fetch_timeline A0000000001 view=thread thread=a00 limit=50 before=- after=-",
            "fetch_timeline A0000000001 view=thread thread=a03 limit=2 before=- after=-",
            "fetch_timeline A0000000001 view=thread thread=zz limit=50 before=- after=-",
            "fetch_timeline A0000000001 view=thread thread=a00 limit=50 before=- after=-",
            "fetch_timeline A0000000001 view=thread thread=a02 limit=50 before=- after=-",
        ]
    );
    assert_eq!(slow.most.load(Ordering::SeqCst), THREADS_IN_FLIGHT);
    assert_eq!(THREADS_IN_FLIGHT, 2);
    assert_eq!(
        answers
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["a00", "a03", "zz", "a00", "a02"]
    );
    let page = |at: usize| answers[at].1.as_ref().expect("a page");
    assert_eq!(ids(page(0)), ["a00-root"]);
    assert_eq!(ids(page(1)), ["a03-r2", "a03-r3"], "its own size");
    assert!(
        page(1).has_more,
        "a page is passed on as the provider gave it"
    );
    assert!(matches!(
        answers[2].1,
        Err(ChatError::Status { status: 404, .. })
    ));
    assert_eq!(ids(page(3)), ["a00-root"]);
    assert_eq!(ids(page(4)), ["a02-root", "a02-r1", "a02-r2"]);
    for at in [0, 1, 3, 4] {
        assert_eq!(
            page(at).delta,
            None,
            "no newer_than: the newest page itself"
        );
    }
    // Byte for byte the thread view's own answer.
    let direct = plain
        .fetch_timeline(&a, &ThreadAsk::newest("a03", 2).request())
        .await
        .expect("a page");
    assert_eq!(*page(1), direct);
    plain.take();
    assert!(slow.fetch_threads(&a, &[]).await.is_empty());
    assert!(plain.take().is_empty(), "nothing is read for no asks");
}

/// remote-ops-audit W14's second proof: an answer to `newer_than` holds only what is newer, and is
/// a delta of additions. It never claims to be the whole thread below the instant (no cursor, no
/// `has_more`, `complete` false) and never says a message is gone (`deleted` empty), so nothing
/// can stamp or tombstone on it; it says when it may not hold everything above the instant either.
#[tokio::test]
async fn the_default_answers_newer_than_with_additions_only_and_never_a_whole_thread() {
    let plain = Plain::default();
    let a = channel("A0000000001");
    plain.seed(&a, "a", &[0, 0, 0, 3, 12]);
    let ask = |id: &str, newer_than_ms: Option<i64>, limit: u16| ThreadAsk {
        id: id.to_owned(),
        newer_than_ms,
        limit,
    };
    let answers = plain
        .fetch_threads(
            &a,
            &[
                ask("a03", Some(reply_ms(3, 1)), 50),
                ask("a03", Some(reply_ms(3, 0) - 1), 50),
                ask("a03", Some(reply_ms(3, 3)), 50),
                ask("a04", Some(reply_ms(4, 0) - 1), 5),
                ask("a04", Some(reply_ms(4, 9)), 5),
                ask("a03", None, 50),
            ],
        )
        .await;
    let page = |at: usize| answers[at].1.as_ref().expect("a page");
    let additions_only = |at: usize, more: bool| {
        let page = page(at);
        assert_eq!(
            page.delta,
            Some(TimelineDelta {
                more,
                complete: false,
                deleted: Vec::new(),
                removed_threads: Vec::new(),
            }),
            "{at}"
        );
        assert!(
            !page.has_more,
            "{at}: no step back from an answer to newer_than"
        );
        assert_eq!(page.next_before, None, "{at}");
        assert_eq!(page.next_after, None, "{at}");
        let summary = page.thread.as_ref().expect("the thread's summary is kept");
        assert!(summary.root.is_some(), "{at}: its root too, on the summary");
    };
    assert_eq!(ids(page(0)), ["a03-r2", "a03-r3"]);
    additions_only(0, false);
    assert_eq!(
        ids(page(1)),
        ["a03-root", "a03-r1", "a03-r2", "a03-r3"],
        "a root newer than the instant is news too"
    );
    additions_only(1, false);
    assert!(ids(page(2)).is_empty(), "nothing is newer than the newest");
    additions_only(2, false);
    // Twelve replies read five at a time: all five are newer, and the page did not reach the
    // instant, so newer messages may have been left out.
    assert_eq!(
        ids(page(3)),
        ["a04-r8", "a04-r9", "a04-r10", "a04-r11", "a04-r12"]
    );
    additions_only(3, true);
    // The page reached back past the instant: everything newer is here.
    assert_eq!(ids(page(4)), ["a04-r10", "a04-r11", "a04-r12"]);
    additions_only(4, false);
    assert_eq!(
        *page(5),
        plain
            .read(&a, "a03", 50)
            .expect("the thread view's own page"),
        "no newer_than is the newest page itself"
    );
}

fn routed(id: &str, provider: &str) -> ChannelInfo {
    ChannelInfo {
        id: channel(id),
        label: id.to_owned(),
        writable: false,
        alias: None,
        added: false,
        provider: Some(provider.to_owned()),
        write_source: None,
    }
}

/// remote-ops-audit W14, the router's part: a channel's threads are read by that channel's own
/// provider, in one call when it reads several threads itself and one thread view read each when
/// it does not; a channel no provider holds is refused for every thread and asked of nobody.
#[tokio::test]
async fn the_router_asks_the_channels_own_provider_and_refuses_an_unrouted_channel() {
    let plain_n = Arc::new(Plain::default());
    let plain_p = Arc::new(Plain::default());
    let (n, p) = (channel("N0000000001"), channel("P0000000002"));
    plain_n.seed(&n, "n", &[1, 2, 3]);
    plain_p.seed(&p, "p", &[1, 2]);
    let native = Arc::new(Native::new(Arc::clone(&plain_n)));
    let counting_n = Arc::new(CountingChat::new(Arc::clone(&native) as Arc<dyn ChatClient>));
    let counting_p = Arc::new(CountingChat::new(
        Arc::clone(&plain_p) as Arc<dyn ChatClient>
    ));
    let entry = |key: &str, client: Arc<CountingChat>| ProviderEntry {
        key: key.to_owned(),
        namespace: key.to_owned(),
        client,
        live_poll_seconds: 0,
    };
    let router = ChatRouter::new(
        vec![
            entry("n", Arc::clone(&counting_n)),
            entry("p", Arc::clone(&counting_p)),
        ],
        None,
    );
    router.replace_routes(&[routed(n.as_str(), "n"), routed(p.as_str(), "p")]);
    let asks = |ids: &[&str]| -> Vec<ThreadAsk> {
        ids.iter().map(|id| ThreadAsk::newest(*id, 50)).collect()
    };

    let answers = router
        .fetch_threads(&n, &asks(&["n02", "n00", "n01"]))
        .await;
    assert_eq!(counting_n.count("fetch_threads"), 1);
    assert_eq!(
        counting_n.calls().len(),
        1,
        "one call, and no thread view read"
    );
    assert_eq!(native.asked(), [vec!["n02", "n00", "n01"]]);
    assert_eq!(plain_n.take(), ["fetch_threads N0000000001 n02,n00,n01"]);
    assert_eq!(
        answers
            .iter()
            .map(|(id, page)| (id.as_str(), ids(page.as_ref().expect("a page")).len()))
            .collect::<Vec<_>>(),
        [("n02", 4), ("n00", 2), ("n01", 3)]
    );

    let answers = router.fetch_threads(&p, &asks(&["p01", "p00"])).await;
    assert_eq!(counting_p.count("fetch_threads"), 0);
    assert_eq!(
        counting_p.timeline_reads(&p, TimelineView::Thread),
        2,
        "a provider that does not read several threads is read as it always was"
    );
    assert_eq!(
        answers
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        ["p01", "p00"]
    );

    let before = (counting_n.calls().len(), counting_p.calls().len());
    let answers = router
        .fetch_threads(&channel("NOWHERE"), &asks(&["x", "y"]))
        .await;
    assert_eq!(answers.len(), 2);
    assert!(answers
        .iter()
        .all(|(_, page)| matches!(page, Err(ChatError::Refused(_)))));
    assert_eq!(
        (counting_n.calls().len(), counting_p.calls().len()),
        before,
        "asked of nobody"
    );
}

/// remote-ops-audit W14: a provider that does not read several threads itself is asked the thread
/// view reads the default is made of, and each is timed and counted as the `fetch_timeline` it is;
/// nothing is ever timed as `fetch_threads` for it. One that does is timed once, its rows every
/// message it answered.
#[tokio::test]
async fn an_emulated_read_of_several_threads_is_timed_and_counted_as_the_thread_views_it_is_made_of(
) {
    let plain = Arc::new(Plain::default());
    let a = channel("A0000000001");
    plain.seed(&a, "a", &[0, 2]);
    let asks = [ThreadAsk::newest("a00", 50), ThreadAsk::newest("a01", 50)];
    let operations = |profile: &UpstreamProfile| -> Vec<(&'static str, u64, Option<u64>)> {
        profile.report().providers[0]
            .operations
            .iter()
            .map(|row| (row.name, row.count, row.rows))
            .collect()
    };

    let profile = UpstreamProfile::new();
    let profiled = ProfiledClient::new(
        Arc::clone(&plain) as Arc<dyn ChatClient>,
        profile.provider("plain", "Plain"),
    );
    let answers = profiled.fetch_threads(&a, &asks).await;
    assert!(answers.iter().all(|(_, page)| page.is_ok()));
    let timed = operations(&profile);
    assert_eq!(timed.len(), 1, "{timed:?}");
    assert_eq!((timed[0].0, timed[0].1), ("fetch_timeline", 2));

    let counting = CountingChat::new(Arc::clone(&plain) as Arc<dyn ChatClient>);
    counting.fetch_threads(&a, &asks).await;
    assert_eq!(counting.count("fetch_timeline"), 2);
    assert_eq!(counting.count("fetch_threads"), 0);

    let native = Arc::new(Native::new(Arc::clone(&plain)));
    let profile = UpstreamProfile::new();
    let profiled = ProfiledClient::new(
        Arc::clone(&native) as Arc<dyn ChatClient>,
        profile.provider("native", "Native"),
    );
    profiled.fetch_threads(&a, &asks).await;
    assert_eq!(
        operations(&profile),
        [("fetch_threads", 1, Some(4))],
        "one call; a root, and a root with two replies"
    );
}

/// The whole path, against a loopback bridge configured with `thread_reads`: reading twenty
/// threads of one channel is ONE request, `GET /channels/{id}/threads` naming all twenty, and each
/// thread's answer is the bridge's page for it. Real time, not a paused clock: the client's request
/// timeout must not fire while the loopback server answers.
///
/// Thread naming does not read through this (#287 bulk-reads, rebased onto #286 naming-budget):
/// each thread is named only from a read made at its own turn, one per channel per interval.
#[tokio::test]
async fn reading_twenty_threads_of_a_bridge_channel_is_one_request_to_the_bridge() {
    use axum::http::{Method, StatusCode, Uri};
    use axum::response::IntoResponse as _;

    const SPACE: &str = "spaces/A";
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured = Arc::clone(&seen);
    let app = axum::Router::new().fallback(move |method: Method, uri: Uri| {
        let captured = Arc::clone(&captured);
        async move {
            captured
                .lock()
                .expect("seen")
                .push(format!("{method} {uri}"));
            if uri.path() != "/channels/spaces%2FA/threads" {
                return (StatusCode::NOT_FOUND, axum::Json(json!({}))).into_response();
            }
            let url = reqwest::Url::parse(&format!("http://bridge{uri}")).expect("a url");
            let threads: Vec<Value> = url
                .query_pairs()
                .filter(|(key, _)| key == "ids")
                .map(|(_, id)| {
                    let membership = json!({"id": id, "root_message_id": null, "is_root": false,
                        "reply_count": 1, "reply_count_exact": true});
                    json!({"thread_id": id, "page": {
                        "messages": [{
                            "id": format!("{id}/messages/1"),
                            "channel_id": SPACE,
                            "content": format!("the bridge says hello in {id}"),
                            "timestamp": iso(BASE_MS),
                            "author": {"id": "7", "username": "bridge", "bot": false},
                            "thread": membership,
                        }],
                        "threads": [],
                        "thread": {"id": id, "root": null, "title": id, "reply_count": 1,
                                   "reply_count_exact": true, "updated_at": iso(BASE_MS)},
                        "has_threads": true,
                        "has_more": false,
                        "next_before": null,
                    }})
                })
                .collect();
            axum::Json(json!({ "threads": threads })).into_response()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("server");
    });
    let mut config = vibe_talk::testing::config();
    let discord = config.discord_mut().expect("a discord provider");
    discord.api_base = format!("http://{address}");
    discord.thread_api = vibe_talk::threads::ThreadApi::Bridge;
    discord.thread_reads = true;
    let client = vibe_talk::discord::http::HttpDiscordClient::new(discord).expect("a client");
    let space = channel(SPACE);
    let ids: Vec<String> = (0..20)
        .map(|n| format!("spaces/A/threads/t{n:02}"))
        .collect();
    let asks: Vec<ThreadAsk> = ids
        .iter()
        .map(|id| ThreadAsk::newest(id.clone(), 50))
        .collect();
    let answers = client.fetch_threads(&space, &asks).await;
    server.abort();

    let query: String = ids
        .iter()
        .map(|id| format!("ids={}&", id.replace('/', "%2F")))
        .collect();
    assert_eq!(
        seen.lock().expect("seen").clone(),
        [format!("GET /channels/spaces%2FA/threads?{query}limit=50")]
    );
    assert_eq!(answers.len(), 20);
    for (id, (answered, page)) in ids.iter().zip(&answers) {
        assert_eq!(answered, id);
        let page = page.as_ref().expect("the bridge's page");
        assert_eq!(page.messages.len(), 1, "{id}");
        assert_eq!(
            page.messages[0].content,
            format!("the bridge says hello in {id}")
        );
    }
}

// --- Slack ---------------------------------------------------------------------------------------

const SLACK_CH: &str = "C0SLACK01";

/// A Slack conversation of `roots.len()` threads, root `n` posted at second `100 * n`, each with
/// the replies listed for it (seconds after the root), and one ordinary message between each.
fn slack_threads(roots: &[&[i64]]) -> slack_fake::FakeState {
    let mut state = slack_fake::FakeState::default();
    let mut list = Vec::new();
    for (n, replies) in roots.iter().enumerate() {
        let at = 100 * i64::try_from(n).expect("small");
        let mut root = slack_fake::profiled(
            &slack_fake::ts(at, 0),
            "UADA00001",
            "Ada",
            &format!("root {n} starts here"),
        );
        let replies: Vec<(String, String)> = replies
            .iter()
            .map(|after| {
                (
                    slack_fake::ts(at + after, 0),
                    format!("reply {after} under root {n}"),
                )
            })
            .collect();
        slack_fake::thread(&mut state, SLACK_CH, &mut root, &replies);
        list.push(root);
        list.push(slack_fake::profiled(
            &slack_fake::ts(at + 50, 0),
            "UADA00001",
            "Ada",
            &format!("between {n}"),
        ));
    }
    state.history.insert(SLACK_CH.to_owned(), list);
    state
}

fn slack_root(n: i64) -> String {
    slack_fake::ts(100 * n, 0)
}

fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1000
}

/// remote-ops-audit W14, Slack: a thread's newest page is one `conversations.replies` read where
/// the thread view makes two (its root, then the window), and the answer is the thread view's own,
/// cursors and all. A thread that is not in the conversation fails in its own place.
#[tokio::test]
async fn slack_reads_each_threads_newest_page_in_one_replies_call() {
    let fake =
        slack_fake::Fake::start(slack_threads(&[&[1, 2], &[5], &[1, 2, 3, 4], &[9]]), 200).await;
    let client = fake.client();
    let channel = channel(SLACK_CH);
    assert_eq!(client.read_caps(&channel).many_threads, Support::Native);
    let mut asks: Vec<ThreadAsk> = (0..4)
        .map(|n| ThreadAsk::newest(slack_root(n), 3))
        .collect();
    asks.push(ThreadAsk::newest(slack_fake::ts(77, 0), 3));
    let answers = client.fetch_threads(&channel, &asks).await;
    assert_eq!(fake.calls("conversations.replies"), 5, "one each");
    assert_eq!(fake.calls("conversations.history"), 0);
    assert!(matches!(
        answers[4].1.as_ref().map_err(ChatError::cause),
        Err(ChatError::Status { status: 404, .. })
    ));
    for n in 0..4 {
        let viewed = client
            .fetch_timeline(&channel, &asks[n].request())
            .await
            .expect("the thread view");
        assert_eq!(
            answers[n].1.as_ref().expect("a page"),
            &viewed,
            "thread {n}: exactly the thread view's newest page"
        );
    }
    assert!(
        answers[2].1.as_ref().expect("a page").has_more,
        "five messages, three a page"
    );
    assert_eq!(
        fake.calls("conversations.replies"),
        5 + 8,
        "the four thread view reads it replaces cost two calls each"
    );
}

/// remote-ops-audit W14, Slack's skip-list: after an instant, the roots come from one history page
/// shared by every thread asked about; a thread whose root says no reply came after the instant
/// costs no read of its replies, and one that moved costs one `conversations.replies` read with
/// `oldest` at the instant. Every answer is a delta of additions, never a whole thread.
#[tokio::test]
async fn slack_reads_after_an_instant_only_the_threads_whose_root_moved() {
    // Roots at 0, 100, ..., 500; the instant is second 150. Thread 0's replies are all older;
    // thread 1 has one reply older and two newer; thread 3 is wholly newer; 4 and 5 too, quietly.
    let fake = slack_fake::Fake::start(
        slack_threads(&[&[10, 20], &[10, 60, 70], &[], &[5], &[], &[3]]),
        200,
    )
    .await;
    let client = fake.client();
    let channel = channel(SLACK_CH);
    let newer_than_ms = slack_ms(150);
    let asks: Vec<ThreadAsk> = [0, 1, 3]
        .iter()
        .map(|n| ThreadAsk {
            id: slack_root(*n),
            newer_than_ms: Some(newer_than_ms),
            limit: 50,
        })
        .collect();
    let answers = client.fetch_threads(&channel, &asks).await;

    assert_eq!(
        fake.calls("conversations.history"),
        1,
        "one page finds every root"
    );
    let replies = fake.requests("conversations.replies");
    assert_eq!(
        replies
            .iter()
            .map(|params| (params["ts"].clone(), params.get("oldest").cloned()))
            .collect::<Vec<_>>(),
        [
            (slack_root(1), Some(slack_fake::ts(150, 999))),
            (slack_root(3), Some(slack_fake::ts(150, 999))),
        ],
        "only the threads that moved, each from the instant"
    );
    let contents = |at: usize| -> Vec<String> {
        answers[at]
            .1
            .as_ref()
            .expect("a page")
            .messages
            .iter()
            .map(|m| m.content.clone())
            .collect()
    };
    assert!(
        contents(0).is_empty(),
        "nothing newer: no read of its replies"
    );
    assert_eq!(
        contents(1),
        ["reply 60 under root 1", "reply 70 under root 1"]
    );
    assert_eq!(
        contents(2),
        ["root 3 starts here", "reply 5 under root 3"],
        "a root newer than the instant is news too"
    );
    for (at, _) in answers.iter().enumerate() {
        let page = answers[at].1.as_ref().expect("a page");
        assert_eq!(
            page.delta,
            Some(TimelineDelta {
                more: false,
                complete: false,
                deleted: Vec::new(),
                removed_threads: Vec::new(),
            }),
            "{at}: additions only, never a deletion"
        );
        assert!(!page.has_more && page.next_before.is_none() && page.next_after.is_none());
        let summary = page.thread.as_ref().expect("its summary");
        assert!(
            summary.root.is_some(),
            "{at}: the root came with the history page"
        );
    }

    // One thread alone reads its own root rather than a history page.
    let before = (
        fake.calls("conversations.history"),
        fake.calls("conversations.replies"),
    );
    let alone = client.fetch_threads(&channel, &asks[..1]).await;
    assert!(alone[0].1.as_ref().expect("a page").messages.is_empty());
    assert_eq!(
        (
            fake.calls("conversations.history"),
            fake.calls("conversations.replies")
        ),
        (before.0, before.1 + 1),
        "its root, and nothing more for a thread that did not move"
    );
}

/// The shared history read never costs more than reading each root would: it stops a page short
/// of the number of threads, and a root it did not reach is read on its own.
#[tokio::test]
async fn slack_reads_a_root_the_shared_history_page_did_not_reach_on_its_own() {
    // Two messages a page: the newest four messages are roots 3 and 2 and what is between them.
    let fake = slack_fake::Fake::start(slack_threads(&[&[5], &[5], &[5], &[5]]), 2).await;
    let client = fake.client();
    let channel = channel(SLACK_CH);
    let asks: Vec<ThreadAsk> = [0, 3]
        .iter()
        .map(|n| ThreadAsk {
            id: slack_root(*n),
            newer_than_ms: Some(slack_ms(1_000)),
            limit: 50,
        })
        .collect();
    let answers = client.fetch_threads(&channel, &asks).await;
    assert_eq!(
        fake.calls("conversations.history"),
        1,
        "two threads: one page at most"
    );
    let replies = fake.requests("conversations.replies");
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert_eq!(
        replies[0]["ts"],
        slack_root(0),
        "the root the page did not reach"
    );
    assert!(answers
        .iter()
        .all(|(_, page)| page.as_ref().expect("a page").messages.is_empty()));
}

/// Twenty threads of one Slack conversation read together: one `fetch_threads`, and each thread
/// costs one `conversations.replies` read where the thread view made two, through a server whose
/// pages hold twenty rows, as the Slack shim's do. At naming's limit of fifty.
///
/// Thread naming does not read through this (`#286 naming-budget`): each thread is named only from
/// a read made at its own turn, one per channel per interval.
#[tokio::test]
async fn reading_twenty_slack_threads_reads_each_once_instead_of_twice() {
    let replies: Vec<&[i64]> = (0..20).map(|_| &[1_i64, 2][..]).collect();
    let fake = slack_fake::Fake::start(slack_threads(&replies), 20).await;
    let client = Arc::new(fake.client());
    let counting = CountingChat::new(Arc::clone(&client) as Arc<dyn ChatClient>);
    let channel = channel(SLACK_CH);
    let asks: Vec<ThreadAsk> = (0..20)
        .map(|n| ThreadAsk::newest(slack_root(n), 50))
        .collect();
    let answers = counting.fetch_threads(&channel, &asks).await;
    assert_eq!(counting.count("fetch_threads"), 1);
    assert_eq!(counting.count("fetch_timeline"), 0);
    assert_eq!(
        fake.calls("conversations.replies"),
        20,
        "one replies read a thread, where the thread view made two"
    );
    assert_eq!(fake.calls("conversations.history"), 0, "no history read");
    assert_eq!(answers.len(), 20);
    for (n, (id, page)) in answers.iter().enumerate() {
        assert_eq!(*id, slack_root(i64::try_from(n).expect("small")));
        let texts: Vec<&str> = page
            .as_ref()
            .expect("a page")
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect();
        assert_eq!(
            texts,
            [
                format!("root {n} starts here"),
                format!("reply 1 under root {n}"),
                format!("reply 2 under root {n}"),
            ]
        );
    }
}
