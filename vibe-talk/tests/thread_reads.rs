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
//! the same order, once naming knows about it. Rewrite it only for a change that is MEANT to alter
//! what such a provider is asked or answers:
//! `VIBE_TALK_UPDATE_FIXTURES=1 cargo test --test thread_reads`.
//!
//! Naming reads each channel at most once per `summaries.thread_naming_interval_seconds`, takes a
//! channel's due threads in its own order, and pauses a channel whose read failed at the provider
//! (`#286 naming-budget`). So the test runs on tokio's paused clock at the default interval, and
//! the fixture records that order and that pause, as the code before `fetch_threads` made them.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity};
use vibe_talk::model::{ChannelId, Message, MessageId, UserId};
use vibe_talk::store::fake::FakeStore;
use vibe_talk::summarize::fake::FakeSummarizer;
use vibe_talk::summarize::threads::{ThreadDeps, ThreadNames};
use vibe_talk::threads::{ThreadSummary, TimelinePage, TimelineRequest, TimelineView};

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
        }
    }

    fn summary(&self, channel: &ChannelId) -> ThreadSummary {
        ThreadSummary {
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
        let id = request.thread_id.clone().unwrap_or_default();
        if self.failing.lock().expect("failing").contains(&id) {
            return Err(ChatError::Status {
                status: 502,
                body: "the thread is unreachable".to_owned(),
            });
        }
        let threads = self.threads.lock().expect("threads");
        threads
            .get(channel.as_str())
            .and_then(|list| list.iter().find(|t| t.id == id))
            .map(|thread| thread.page(channel, request.limit))
            .ok_or_else(|| ChatError::Status {
                status: 404,
                body: "no such thread in this channel".to_owned(),
            })
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.note("identity".to_owned());
        Ok(ChatIdentity {
            id: "7".to_owned(),
            username: "reader".to_owned(),
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
    let replies: Vec<u64> = (0..20).map(|n| n % 4).collect();
    plain.seed(&a, "a", &replies);
    plain.seed(&b, "b", &[1, 0, 2]);
    plain.seed(&c, "c", &[3, 1]);
    plain.seed(&d, "d", &[1, 2, 0]);
    plain.seed(&e, "e", &[0, 1]);
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

    // At 1 message a thread is due again at 2; at 3, at 4; at 4, at 8.
    plain.grow("a00", 1);
    plain.grow("a02", 3);
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
