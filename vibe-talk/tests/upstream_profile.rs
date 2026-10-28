//! `#208 persistent-cache`, milestone `upstream-profile`: every upstream operation is timed, every
//! Slack Web API call is counted as the far end counts it, and none of it carries channel text.
//!
//! The first test is an INVENTORY, checked against `src/chat.rs` in the style of
//! `tests/scope_first.rs`. [`ProfiledClient`] wraps every provider's client, and
//! [`vibe_talk::providers::ChatRouter`] fronts them all; `vibe_talk::testing::CountingChat`
//! counts the calls a test makes through it. A `ChatClient` method any of them leaves to the
//! trait's default body runs that default on the wrapper, which calls the wrapper's other methods
//! and silently skips whatever the provider overrides. So a method added to the trait fails here
//! until all three forward it.

use crate::slack_fake;

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use slack_fake::{profiled, ts, Fake, FakeState, METHODS, TOKEN};
use tower::ServiceExt as _;
use vibe_talk::chat::{
    ChannelHead, ChatClient, ChatError, ChatIdentity, HeadAnswers, LiveRead, Lookup, PostLimit,
    RangeOrder, RangeRead, ReadCaps, ReadScope, RecentAnswers, RecentAsk, RecentRead,
    RegisteredChannel, SourceClaim, Support, ThreadAnswers, ThreadAsk, TimeRange,
    UpstreamClearOutcome, UpstreamReadScope,
};
use vibe_talk::directory::{DirectoryPage, DirectoryRequest};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::providers::ChatRouter;
use vibe_talk::threads::{TimelinePage, TimelineRequest};
use vibe_talk::timeline_forward::{GenericDelta, GenericPosition};
use vibe_talk::upstream_profile::{Outcome, ProfiledClient, UpstreamProfile};

const CHAT_SOURCE: &str = include_str!("../src/chat.rs");
const ROUTER_SOURCE: &str = include_str!("../src/providers.rs");
const PROFILE_SOURCE: &str = include_str!("../src/upstream_profile.rs");
const TESTING_SOURCE: &str = include_str!("../src/testing.rs");

/// The code directly inside the block `header` opens: comments, string and character literals,
/// and every nested block left out.
fn top_level_of(source: &str, header: &str) -> String {
    assert!(header.ends_with('{'), "a header opens a block: {header}");
    let start = source
        .find(header)
        .unwrap_or_else(|| panic!("no `{header}` in the source"))
        + header.len();
    let chars: Vec<char> = source[start..].chars().collect();
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut depth = 0_usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                out.push('\n');
                continue;
            }
            '/' if next == Some('*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                out.push(' ');
                continue;
            }
            'r' if matches!(next, Some('#' | '"')) && (i == 0 || !identifier(chars[i - 1])) => {
                let mut j = i + 1;
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) == Some(&'"') {
                    j += 1;
                    loop {
                        assert!(j < chars.len(), "an unterminated raw string");
                        if chars[j] == '"' && chars[j + 1..].iter().take(hashes).all(|c| *c == '#')
                        {
                            break;
                        }
                        j += 1;
                    }
                    i = j + 1 + hashes;
                    out.push(' ');
                    continue;
                }
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                out.push(' ');
                continue;
            }
            '\'' if next == Some('\\') => {
                i += 2;
                while i < chars.len() && chars[i] != '\'' {
                    i += 1;
                }
                i += 1;
                out.push(' ');
                continue;
            }
            '\'' if chars.get(i + 2) == Some(&'\'') => {
                i += 3;
                out.push(' ');
                continue;
            }
            '{' => {
                depth += 1;
                i += 1;
                continue;
            }
            '}' => {
                if depth == 0 {
                    return out;
                }
                depth -= 1;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth == 0 {
            out.push(c);
        }
        i += 1;
    }
    panic!("the block `{header}` opens is never closed");
}

/// Every `fn` declared in `code`, and the `async` ones among them.
fn fn_names(code: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let identifier = |c: char| c.is_alphanumeric() || c == '_';
    let mut all = BTreeSet::new();
    let mut asynchronous = BTreeSet::new();
    for (at, _) in code.match_indices("fn ") {
        if code[..at].chars().next_back().is_some_and(identifier) {
            continue;
        }
        let name: String = code[at + 3..]
            .trim_start()
            .chars()
            .take_while(|c| identifier(*c))
            .collect();
        assert!(!name.is_empty(), "a `fn` with no name in {code}");
        if code[..at].trim_end().ends_with("async") {
            asynchronous.insert(name.clone());
        }
        all.insert(name);
    }
    (all, asynchronous)
}

/// Every method of `trait ChatClient`, and the `async` ones: those that may reach a provider.
fn trait_methods() -> (BTreeSet<String>, BTreeSet<String>) {
    fn_names(&top_level_of(
        CHAT_SOURCE,
        "pub trait ChatClient: Send + Sync {",
    ))
}

#[test]
fn the_inventory_reader_sees_declarations_and_nothing_else() {
    // The reader is the test's own weak point: one that found nothing would pass everything.
    let sample = r##"
        pub trait T {
            /// `{a, b}` in a comment is not a block, and `fn hidden(` here is not a method.
            fn plain(&self) -> bool { let s = "fn fake() {"; let c = '{'; s.is_empty() && c == '}' }
            async fn read(&self, f: fn(&str) -> u8) -> Result<(), E>;
            fn nested(&self) { fn inner() {} let r = r#"} fn raw() {"#; }
            // fn commented_out(&self);
        }
        fn after_the_block() {}
    "##;
    let (all, asynchronous) = fn_names(&top_level_of(sample, "pub trait T {"));
    assert_eq!(
        all.into_iter().collect::<Vec<_>>(),
        ["nested", "plain", "read"]
    );
    assert_eq!(asynchronous.into_iter().collect::<Vec<_>>(), ["read"]);

    let (all, asynchronous) = trait_methods();
    for known in [
        "provider_name",
        "identity",
        "fetch_page",
        "fetch_timeline_since",
        "clear_upstream_through",
    ] {
        assert!(all.contains(known), "{known} not found in {all:?}");
    }
    assert!(asynchronous.contains("fetch_page") && !asynchronous.contains("provider_name"));
    assert!(
        all.len() >= 27,
        "the trait had 27 methods when this was written: {all:?}"
    );
}

#[test]
fn the_profiled_client_and_the_router_forward_every_chat_client_method() {
    let (methods, _) = trait_methods();
    for (file, source, header) in [
        (
            "src/upstream_profile.rs",
            PROFILE_SOURCE,
            "impl ChatClient for ProfiledClient {",
        ),
        (
            "src/providers.rs",
            ROUTER_SOURCE,
            "impl ChatClient for ChatRouter {",
        ),
        // The counting fixture of the remote-ops audit: a default left on it would count a call
        // the wrapped provider never received, or miss one it did.
        (
            "src/testing.rs",
            TESTING_SOURCE,
            "impl crate::chat::ChatClient for CountingChat {",
        ),
    ] {
        let (defined, _) = fn_names(&top_level_of(source, header));
        let missing: Vec<&String> = methods.difference(&defined).collect();
        assert!(
            missing.is_empty(),
            "`{header}` in {file} leaves {missing:?} to the trait's default body. A default runs on \
             the wrapper and skips the provider's own override, so forward each one explicitly to \
             the wrapped client."
        );
    }
}

/// A client that overrides every `ChatClient` method, records which one ran, and answers with
/// something no default would.
struct Recorder {
    calls: Mutex<Vec<&'static str>>,
    message: Message,
}

impl Recorder {
    async fn new() -> Self {
        let fake = FakeDiscord::new();
        let channel = ChannelId("C1".to_owned());
        fake.seed(&channel, "ada", "words of some length");
        let message = fake.fetch_recent(&channel, 1).await.expect("seeded")[0].clone();
        Self {
            calls: Mutex::new(Vec::new()),
            message,
        }
    }

    fn note(&self, name: &'static str) {
        self.calls.lock().expect("calls").push(name);
    }

    fn take(&self) -> Vec<&'static str> {
        std::mem::take(&mut *self.calls.lock().expect("calls"))
    }

    fn page(&self) -> TimelinePage {
        TimelinePage {
            messages: vec![self.message.clone()],
            ..TimelinePage::default()
        }
    }
}

#[async_trait]
impl ChatClient for Recorder {
    fn provider_name(&self) -> &str {
        self.note("provider_name");
        "Recorder"
    }
    fn claims_source(&self, _source: &str) -> SourceClaim {
        self.note("claims_source");
        SourceClaim::Certain
    }
    fn self_author_id(&self) -> Option<String> {
        self.note("self_author_id");
        Some("self".to_owned())
    }
    fn owner_author_id(&self) -> Option<String> {
        self.note("owner_author_id");
        Some("owner".to_owned())
    }
    fn upstream_url(
        &self,
        _channel: &ChannelId,
        _id: &MessageId,
        _thread: Option<&vibe_talk::threads::MessageThread>,
    ) -> Option<String> {
        self.note("upstream_url");
        Some("https://example.test/link".to_owned())
    }
    fn upstream_app_url(
        &self,
        _channel: &ChannelId,
        _id: &MessageId,
        _thread: Option<&vibe_talk::threads::MessageThread>,
    ) -> Option<String> {
        self.note("upstream_app_url");
        Some("slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200".to_owned())
    }
    fn supports_threading(&self) -> bool {
        self.note("supports_threading");
        true
    }
    fn replies_in_thread(&self) -> bool {
        self.note("replies_in_thread");
        true
    }
    async fn fetch_timeline(
        &self,
        _channel: &ChannelId,
        _request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        self.note("fetch_timeline");
        Ok(self.page())
    }
    async fn fetch_timeline_since(
        &self,
        _channel: &ChannelId,
        _request: &TimelineRequest,
        since: &GenericPosition,
    ) -> Result<GenericDelta, ChatError> {
        self.note("fetch_timeline_since");
        Ok(GenericDelta {
            page: self.page(),
            position: since.clone(),
        })
    }
    async fn post_in_thread(
        &self,
        _channel: &ChannelId,
        _thread_id: &str,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.note("post_in_thread");
        Ok(self.message.clone())
    }
    async fn fetch_thread_message(
        &self,
        _channel: &ChannelId,
        _thread_id: &str,
        _message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.note("fetch_thread_message");
        Ok(self.message.clone())
    }
    fn supports_channel_registration(&self) -> bool {
        self.note("supports_channel_registration");
        true
    }
    async fn register_channel(
        &self,
        _source: &str,
        _label: &str,
    ) -> Result<RegisteredChannel, ChatError> {
        self.note("register_channel");
        Ok(RegisteredChannel {
            id: ChannelId("C2".to_owned()),
            created: true,
            writable: true,
        })
    }
    async fn unregister_channel(&self, _channel: &ChannelId) -> Result<(), ChatError> {
        self.note("unregister_channel");
        Ok(())
    }
    fn supports_channel_discovery(&self) -> bool {
        self.note("supports_channel_discovery");
        true
    }
    fn forget_channel_directory(&self) {
        self.note("forget_channel_directory");
    }
    fn added_channels_writable(&self) -> Option<bool> {
        self.note("added_channels_writable");
        Some(true)
    }
    async fn discover_channels(
        &self,
        _request: &DirectoryRequest,
    ) -> Result<DirectoryPage, ChatError> {
        self.note("discover_channels");
        Ok(DirectoryPage {
            entries: Vec::new(),
            next_cursor: Some("next".to_owned()),
            truncated: true,
        })
    }
    fn supports_upstream_read_mark(&self) -> bool {
        self.note("supports_upstream_read_mark");
        true
    }
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.note("identity");
        Ok(ChatIdentity {
            id: "U1".to_owned(),
            username: "recorder".to_owned(),
            workspace: None,
        })
    }
    async fn fetch_page(
        &self,
        _channel: &ChannelId,
        _limit: u16,
        _before: Option<&MessageId>,
        _after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.note("fetch_page");
        Ok(vec![self.message.clone()])
    }
    async fn probe_channel(
        &self,
        _channel: &ChannelId,
        _limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.note("probe_channel");
        Ok(vec![self.message.clone()])
    }
    async fn fetch_recent(
        &self,
        _channel: &ChannelId,
        _limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.note("fetch_recent");
        Ok(vec![self.message.clone()])
    }
    async fn fetch_recent_dated(
        &self,
        _channel: &ChannelId,
        _limit: u16,
    ) -> Result<RecentRead, ChatError> {
        self.note("fetch_recent_dated");
        Ok(RecentRead {
            messages: vec![self.message.clone()],
            as_of: Some("2026-10-12T09:00:00Z".to_owned()),
            exhausted: false,
            notice: None,
        })
    }
    fn polls_thread_replies(&self, _channel: &ChannelId) -> bool {
        self.note("polls_thread_replies");
        true
    }
    fn takes_pushed_messages(&self, _channel: &ChannelId) -> bool {
        self.note("takes_pushed_messages");
        true
    }
    async fn pushed_message(
        &self,
        _channel: &ChannelId,
        _raw: Value,
    ) -> Result<Option<Message>, ChatError> {
        self.note("pushed_message");
        Ok(Some(self.message.clone()))
    }
    async fn fetch_live(
        &self,
        _channel: &ChannelId,
        _limit: u16,
        _after: Option<&MessageId>,
    ) -> Result<LiveRead, ChatError> {
        self.note("fetch_live");
        Ok(LiveRead {
            messages: Vec::new(),
            replies: vec![self.message.clone()],
            replies_behind: true,
            changed: Vec::new(),
        })
    }
    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        self.note("read_caps");
        ReadCaps {
            range: Support::Native,
            by_ids: Support::Native,
            heads: Support::Native,
            heads_cover_edits: true,
            many_channels: Support::Native,
            many_threads: Support::Native,
            page_rows: 7,
        }
    }
    async fn fetch_range(
        &self,
        _channel: &ChannelId,
        _scope: &ReadScope,
        _range: TimeRange,
        _order: RangeOrder,
        _limit: u16,
        _cursor: Option<&str>,
    ) -> Result<RangeRead, ChatError> {
        self.note("fetch_range");
        Ok(RangeRead {
            messages: vec![self.message.clone()],
            next: Some("next".to_owned()),
            exhausted: true,
            as_of: None,
            emulated: false,
        })
    }
    async fn fetch_messages(
        &self,
        _channel: &ChannelId,
        _thread: Option<&str>,
        ids: &[MessageId],
    ) -> Result<Vec<Lookup>, ChatError> {
        self.note("fetch_messages");
        Ok(ids
            .iter()
            .enumerate()
            .map(|(n, _)| {
                if n == 0 {
                    Lookup::Found(Box::new(self.message.clone()))
                } else {
                    Lookup::Absent
                }
            })
            .collect())
    }
    async fn channel_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        self.note("channel_heads");
        channels
            .iter()
            .map(|channel| {
                (
                    channel.clone(),
                    Ok(ChannelHead {
                        newest_id: Some(self.message.id.clone()),
                        unchanged_since_ms: Some(7),
                        ..ChannelHead::default()
                    }),
                )
            })
            .collect()
    }
    async fn fetch_recent_many(&self, asks: &[(ChannelId, RecentAsk)]) -> RecentAnswers {
        self.note("fetch_recent_many");
        asks.iter()
            .map(|(channel, _)| {
                (
                    channel.clone(),
                    Ok(RecentRead {
                        messages: vec![self.message.clone()],
                        as_of: Some("2026-10-12T10:00:00Z".to_owned()),
                        exhausted: false,
                        notice: None,
                    }),
                )
            })
            .collect()
    }
    async fn fetch_threads(&self, _channel: &ChannelId, asks: &[ThreadAsk]) -> ThreadAnswers {
        self.note("fetch_threads");
        asks.iter()
            .map(|ask| {
                (
                    ask.id.clone(),
                    Ok(TimelinePage {
                        as_of: Some("2026-10-12T11:00:00Z".to_owned()),
                        ..self.page()
                    }),
                )
            })
            .collect()
    }
    async fn post_message(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.note("post_message");
        Ok(self.message.clone())
    }
    fn supports_idempotent_posts(&self) -> bool {
        self.note("supports_idempotent_posts");
        true
    }
    fn op_key_carrier(&self, _channel: &ChannelId) -> vibe_talk::chat::OpKeyCarrier {
        self.note("op_key_carrier");
        vibe_talk::chat::OpKeyCarrier {
            carried: true,
            dedupes: vibe_talk::chat::OpKeyDedupe::Always,
            readable: true,
        }
    }
    async fn post_message_keyed(
        &self,
        _channel: &ChannelId,
        _content: &str,
        _reply_to: Option<&MessageId>,
        _key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.note("post_message_keyed");
        Ok(self.message.clone())
    }
    async fn post_in_thread_keyed(
        &self,
        _channel: &ChannelId,
        _thread_id: &str,
        _content: &str,
        _reply_to: Option<&MessageId>,
        _key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.note("post_in_thread_keyed");
        Ok(self.message.clone())
    }
    fn post_limit(&self, _channel: &ChannelId) -> PostLimit {
        self.note("post_limit");
        PostLimit::bytes(4321)
    }
    fn defuses_mentions(&self, _channel: &ChannelId) -> bool {
        self.note("defuses_mentions");
        true
    }
    fn continuation_thread(&self, _channel: &ChannelId, _first: &Message) -> Option<String> {
        self.note("continuation_thread");
        Some("t-continued".to_owned())
    }
    async fn mark_read_upstream(
        &self,
        _channel: &ChannelId,
        _through: &MessageId,
    ) -> Result<(), ChatError> {
        self.note("mark_read_upstream");
        Ok(())
    }
    async fn upstream_read_scope(
        &self,
        _channel: &ChannelId,
    ) -> Result<UpstreamReadScope, ChatError> {
        self.note("upstream_read_scope");
        Ok(UpstreamReadScope::Wider)
    }
    fn supports_upstream_clear(&self) -> bool {
        self.note("supports_upstream_clear");
        true
    }
    fn crawl_hold(&self) -> Option<std::time::Duration> {
        self.note("crawl_hold");
        Some(std::time::Duration::from_secs(7))
    }
    fn rate_limit_pressure(&self) -> Option<vibe_talk::chat::RateLimitPressure> {
        self.note("rate_limit_pressure");
        Some(vibe_talk::chat::RateLimitPressure {
            limited_last_10_min: 3,
            ..Default::default()
        })
    }
    async fn clear_upstream_through(
        &self,
        _channel: &ChannelId,
        through: &MessageId,
        dry_run: bool,
    ) -> Result<UpstreamClearOutcome, ChatError> {
        self.note("clear_upstream_through");
        Ok(UpstreamClearOutcome {
            scope: "conversation".to_owned(),
            target: through.0.clone(),
            previous: None,
            advanced: true,
            dry_run,
        })
    }
}

#[tokio::test]
async fn every_method_reaches_the_wrapped_clients_own_override_and_each_upstream_one_is_timed() {
    let recorder = Arc::new(Recorder::new().await);
    let profile = UpstreamProfile::new();
    let client = ProfiledClient::new(
        Arc::clone(&recorder) as Arc<dyn ChatClient>,
        profile.provider("rec", "Recorder"),
    );
    let channel = ChannelId("C1".to_owned());
    let id = recorder.message.id.clone();
    let request = TimelineRequest::default();
    let mut exercised = BTreeSet::new();
    // Exactly the one override ran: a method left to the trait's default would run the default
    // on the wrapper, which records nothing here or records the method the default calls.
    let mut ran = |name: &'static str| {
        assert_eq!(
            recorder.take(),
            [name],
            "{name} did not reach the wrapped client's own {name}"
        );
        exercised.insert(name.to_owned());
    };

    assert_eq!(client.provider_name(), "Recorder");
    ran("provider_name");
    assert_eq!(client.claims_source("anything"), SourceClaim::Certain);
    ran("claims_source");
    assert_eq!(client.self_author_id().as_deref(), Some("self"));
    ran("self_author_id");
    assert_eq!(client.owner_author_id().as_deref(), Some("owner"));
    ran("owner_author_id");
    assert_eq!(
        client.upstream_url(&channel, &id, None).as_deref(),
        Some("https://example.test/link")
    );
    ran("upstream_url");
    assert_eq!(
        client.upstream_app_url(&channel, &id, None).as_deref(),
        Some("slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200")
    );
    ran("upstream_app_url");
    assert!(client.supports_threading());
    ran("supports_threading");
    assert!(client.replies_in_thread());
    ran("replies_in_thread");
    assert_eq!(
        client
            .fetch_timeline(&channel, &request)
            .await
            .expect("timeline")
            .messages
            .len(),
        1
    );
    ran("fetch_timeline");
    client
        .fetch_timeline_since(&channel, &request, &GenericPosition::origin())
        .await
        .expect("since");
    ran("fetch_timeline_since");
    client
        .post_in_thread(&channel, "t", "hi", None)
        .await
        .expect("post");
    ran("post_in_thread");
    client
        .fetch_thread_message(&channel, "t", &id)
        .await
        .expect("lookup");
    ran("fetch_thread_message");
    assert!(client.supports_channel_registration());
    ran("supports_channel_registration");
    assert!(
        client
            .register_channel("source", "label")
            .await
            .expect("register")
            .created
    );
    ran("register_channel");
    client.unregister_channel(&channel).await.expect("remove");
    ran("unregister_channel");
    assert!(client.supports_channel_discovery());
    ran("supports_channel_discovery");
    let directory = client
        .discover_channels(&DirectoryRequest {
            query: None,
            cursor: None,
            limit: 5,
        })
        .await
        .expect("directory");
    assert!(directory.truncated);
    ran("discover_channels");
    client.forget_channel_directory();
    ran("forget_channel_directory");
    assert_eq!(client.added_channels_writable(), Some(true));
    ran("added_channels_writable");
    assert!(client.supports_upstream_read_mark());
    ran("supports_upstream_read_mark");
    assert_eq!(client.identity().await.expect("identity").id, "U1");
    ran("identity");
    client
        .fetch_page(&channel, 5, None, None)
        .await
        .expect("page");
    ran("fetch_page");
    client.probe_channel(&channel, 5).await.expect("probe");
    ran("probe_channel");
    client.fetch_recent(&channel, 5).await.expect("recent");
    ran("fetch_recent");
    let dated = client
        .fetch_recent_dated(&channel, 5)
        .await
        .expect("dated recent");
    assert_eq!(dated.as_of.as_deref(), Some("2026-10-12T09:00:00Z"));
    assert_eq!(dated.messages.len(), 1);
    ran("fetch_recent_dated");
    assert!(client.polls_thread_replies(&channel));
    ran("polls_thread_replies");
    assert!(client.takes_pushed_messages(&channel));
    ran("takes_pushed_messages");
    let pushed = client
        .pushed_message(&channel, serde_json::json!({"ts": "1.0"}))
        .await
        .expect("pushed");
    assert!(pushed.is_some());
    ran("pushed_message");
    let live = client
        .fetch_live(&channel, 5, Some(&id))
        .await
        .expect("live read");
    assert!(live.replies_behind && live.replies.len() == 1);
    ran("fetch_live");
    assert_eq!(client.read_caps(&channel).page_rows, 7);
    ran("read_caps");
    let range = client
        .fetch_range(
            &channel,
            &ReadScope::Thread("t".to_owned()),
            TimeRange {
                since_ms: 0,
                until_ms: None,
            },
            RangeOrder::NewestFirst,
            5,
            Some("cursor"),
        )
        .await
        .expect("range");
    assert!(range.exhausted && range.next.as_deref() == Some("next"));
    // The profiler asks the provider whether the read is its own, to time it under what it costs.
    {
        let mut calls = recorder.calls.lock().expect("calls");
        assert_eq!(calls.first(), Some(&"read_caps"), "{calls:?}");
        calls.remove(0);
    }
    ran("fetch_range");
    let looked = client
        .fetch_messages(
            &channel,
            Some("t"),
            &[id.clone(), MessageId("gone".to_owned())],
        )
        .await
        .expect("lookup");
    assert_eq!(looked[1], Lookup::Absent);
    // As for a range read: the profiler asks whether the lookup is the provider's own.
    {
        let mut calls = recorder.calls.lock().expect("calls");
        assert_eq!(calls.first(), Some(&"read_caps"), "{calls:?}");
        calls.remove(0);
    }
    ran("fetch_messages");
    let heads = client
        .channel_heads(&[channel.clone(), ChannelId("C9".to_owned())])
        .await;
    assert_eq!(heads.len(), 2);
    assert!(heads
        .iter()
        .all(|(_, head)| head.as_ref().expect("a head").unchanged_since_ms == Some(7)));
    // The profiler asks, channel by channel, whether the provider reads the head itself.
    {
        let mut calls = recorder.calls.lock().expect("calls");
        assert_eq!(calls[..2], ["read_caps", "read_caps"], "{calls:?}");
        calls.drain(..2);
    }
    ran("channel_heads");
    let recent = client
        .fetch_recent_many(&[
            (channel.clone(), RecentAsk::newest(5)),
            (ChannelId("C9".to_owned()), RecentAsk::newest(5)),
        ])
        .await;
    assert_eq!(recent.len(), 2);
    assert!(recent
        .iter()
        .all(|(_, read)| read.as_ref().expect("a read").as_of.as_deref()
            == Some("2026-10-12T10:00:00Z")));
    // The profiler asks, channel by channel, whether the provider reads them together itself.
    {
        let mut calls = recorder.calls.lock().expect("calls");
        assert_eq!(calls[..2], ["read_caps", "read_caps"], "{calls:?}");
        calls.drain(..2);
    }
    ran("fetch_recent_many");
    let threads = client
        .fetch_threads(
            &channel,
            &[ThreadAsk::newest("t1", 5), ThreadAsk::newest("t2", 5)],
        )
        .await;
    assert_eq!(
        threads
            .iter()
            .map(|(id, page)| (id.as_str(), page.as_ref().expect("a page").as_of.as_deref()))
            .collect::<Vec<_>>(),
        [
            ("t1", Some("2026-10-12T11:00:00Z")),
            ("t2", Some("2026-10-12T11:00:00Z"))
        ]
    );
    // As for a lookup: the profiler asks whether the read of several threads is the provider's own.
    {
        let mut calls = recorder.calls.lock().expect("calls");
        assert_eq!(calls.first(), Some(&"read_caps"), "{calls:?}");
        calls.remove(0);
    }
    ran("fetch_threads");
    client
        .post_message(&channel, "hi", None)
        .await
        .expect("post");
    ran("post_message");
    assert!(client.supports_idempotent_posts());
    ran("supports_idempotent_posts");
    assert!(client.op_key_carrier(&channel).readable);
    ran("op_key_carrier");
    client
        .post_message_keyed(&channel, "hi", None, Some("k"))
        .await
        .expect("keyed post");
    ran("post_message_keyed");
    client
        .post_in_thread_keyed(&channel, "t", "hi", None, Some("k"))
        .await
        .expect("keyed thread post");
    ran("post_in_thread_keyed");
    assert_eq!(client.post_limit(&channel), PostLimit::bytes(4321));
    ran("post_limit");
    assert!(client.defuses_mentions(&channel));
    ran("defuses_mentions");
    assert_eq!(
        client
            .continuation_thread(&channel, &recorder.message)
            .as_deref(),
        Some("t-continued")
    );
    ran("continuation_thread");
    client
        .mark_read_upstream(&channel, &id)
        .await
        .expect("mark");
    ran("mark_read_upstream");
    assert_eq!(
        client
            .upstream_read_scope(&channel)
            .await
            .expect("read scope"),
        UpstreamReadScope::Wider
    );
    ran("upstream_read_scope");
    assert!(client.supports_upstream_clear());
    ran("supports_upstream_clear");
    assert!(
        client
            .clear_upstream_through(&channel, &id, true)
            .await
            .expect("clear")
            .dry_run
    );
    ran("clear_upstream_through");
    assert_eq!(client.crawl_hold(), Some(std::time::Duration::from_secs(7)));
    ran("crawl_hold");
    assert_eq!(
        client.rate_limit_pressure().map(|p| p.limited_last_10_min),
        Some(3)
    );
    ran("rate_limit_pressure");

    let (methods, upstream) = trait_methods();
    assert_eq!(
        exercised, methods,
        "every ChatClient method must be called through the wrapper here"
    );

    let report = profile.report();
    let rec = &report.providers[0];
    let timed: BTreeSet<String> = rec
        .operations
        .iter()
        .map(|row| row.name.to_owned())
        .collect();
    assert_eq!(
        timed, upstream,
        "every async method is timed, and only those: a capability question is not an upstream call"
    );
    assert!(rec
        .operations
        .iter()
        .all(|row| row.outcome == Outcome::Ok && row.count == 1 && row.p50_ms >= 0.0));
    let rows = |name: &str| {
        rec.operations
            .iter()
            .find(|row| row.name == name)
            .and_then(|row| row.rows)
    };
    assert_eq!(rows("fetch_recent"), Some(1));
    assert_eq!(rows("fetch_recent_dated"), Some(1));
    assert_eq!(rows("fetch_live"), Some(1), "a reply is a row");
    assert_eq!(
        rows("discover_channels"),
        Some(0),
        "an empty directory page"
    );
    assert_eq!(rows("fetch_range"), Some(1));
    assert_eq!(rows("fetch_messages"), Some(1), "a found message is a row");
    assert_eq!(
        rows("fetch_recent_many"),
        Some(2),
        "one message from each of two channels"
    );
    assert_eq!(
        rows("fetch_threads"),
        Some(2),
        "one message from each of two threads"
    );
    assert_eq!(
        rows("pushed_message"),
        Some(1),
        "a normalized push is a row"
    );
    // Eleven reads each return the one message (a lookup's absent id is no message, a live read's
    // reply is one, a normalized push is one), and the reads of two channels and of two threads
    // return it twice each; posts return ours and are not counted.
    let bytes = recorder.message.content.len() as u64;
    assert_eq!(rec.content_bytes.messages, 15, "{:?}", rec.content_bytes);
    assert_eq!(rec.content_bytes.total, 15 * bytes);
    assert_eq!(rec.content_bytes.max, bytes);
    assert!(
        rec.shim_calls.is_empty(),
        "this client times no calls of its own"
    );
}

#[tokio::test]
async fn a_failure_is_timed_under_its_outcome_and_returned_unchanged() {
    let fake = Arc::new(FakeDiscord::new());
    fake.revoke_token();
    let router = ChatRouter::single(fake, "ns", 0);
    let error = router.identity().await.expect_err("revoked");
    assert!(matches!(
        error.cause(),
        ChatError::Status { status: 401, .. }
    ));
    let report = router.profile().report();
    let row = report.providers[0]
        .operations
        .iter()
        .find(|row| row.name == "identity")
        .expect("identity timed");
    assert_eq!(row.outcome, Outcome::Unauthorized);
    assert_eq!(row.count, 1);
    assert_eq!(row.rows, Some(0));
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(
                    "authorization",
                    format!("Bearer {}", vibe_talk::testing::READ_TOKEN),
                )
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn diagnostics_show_each_providers_timings_and_sizes_and_no_channel_text() {
    let (state, discord) = vibe_talk::testing::state();
    let channel = ChannelId(vibe_talk::testing::READ_CHANNEL.to_owned());
    let words = "a-sentence-only-the-channel-should-ever-show";
    discord.seed(&channel, "Grace Hopper", words);
    discord.seed(&channel, "Ada Lovelace", "short");
    let router = vibe_talk::http::router(state);
    let (status, _) = get(&router, &format!("/api/v1/channels/{}/messages", channel.0)).await;
    assert_eq!(status, StatusCode::OK);

    let (status, report) = get(&router, "/api/v1/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    let upstream = &report["upstream"];
    assert!(upstream["since"]
        .as_str()
        .is_some_and(|since| since.ends_with('Z')));
    let providers = upstream["providers"].as_array().expect("providers");
    assert_eq!(providers.len(), 1, "{upstream:#}");
    let discord_profile = &providers[0];
    assert_eq!(discord_profile["provider"], "discord", "{upstream:#}");
    assert_eq!(discord_profile["name"], "Discord");
    let operations = discord_profile["operations"]
        .as_array()
        .expect("operations");
    let ok_reads: u64 = operations
        .iter()
        .filter(|row| row["outcome"] == "ok" && row["rows"].as_u64().unwrap_or(0) > 0)
        .map(|row| row["count"].as_u64().unwrap_or(0))
        .sum();
    assert!(ok_reads >= 1, "the read was not timed: {upstream:#}");
    assert!(
        operations.iter().any(|row| row["name"] == "identity"),
        "the report's own token check is counted too: {upstream:#}"
    );
    for row in operations {
        for field in ["count", "mean_ms", "p50_ms", "p90_ms", "p99_ms", "max_ms"] {
            assert!(row[field].is_number(), "{field} in {row}");
        }
        assert!(row["octaves_ms"].is_array(), "{row}");
    }
    assert_eq!(discord_profile["shim_calls"], Value::Array(Vec::new()));
    let sizes = &discord_profile["content_bytes"];
    assert!(
        sizes["messages"].as_u64().is_some_and(|n| n >= 2),
        "{upstream:#}"
    );
    assert!(
        sizes["max"]
            .as_u64()
            .is_some_and(|max| max >= words.len() as u64),
        "{upstream:#}"
    );

    let text = upstream.to_string();
    for private in [
        words,
        "short",
        "Grace Hopper",
        "Ada Lovelace",
        channel.0.as_str(),
        vibe_talk::testing::WRITE_CHANNEL,
    ] {
        assert!(
            !text.contains(private),
            "the upstream section carries {private:?}: {text}"
        );
    }
}

const TEAM: &str = "C0000MAIN1";

/// A Slack deployment built from configuration exactly as `main` builds one.
async fn slack_deployment(fake: &Fake) -> vibe_talk::state::AppState {
    let text = format!(
        r#"
[auth]
read_token = "{read}"
write_token = "{write}"

[[providers]]
key = "slack"
kind = "slack"
api_base = "{base}"
token = "{TOKEN}"

[[channels]]
id = "{TEAM}"
label = "team"
writable = true
"#,
        read = vibe_talk::testing::READ_TOKEN,
        write = vibe_talk::testing::WRITE_TOKEN,
        base = fake.base,
    );
    let (mut state, _discord, _store) = vibe_talk::testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    state
}

#[tokio::test]
async fn slack_web_api_calls_are_counted_as_the_far_end_counts_them() {
    let mut fixture = FakeState::default();
    fixture.history.insert(
        TEAM.to_owned(),
        vec![
            profiled(&ts(20, 0), "UADA00001", "Ada", "second message"),
            profiled(&ts(10, 0), "UGRACE001", "Grace", "first"),
        ],
    );
    let fake = Fake::start(fixture, 20).await;
    let state = slack_deployment(&fake).await;
    let router = vibe_talk::http::router(state.clone());
    let (status, body) = get(&router, &format!("/api/v1/channels/{TEAM}/messages")).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let count = |report: &vibe_talk::upstream_profile::UpstreamReport,
                 shim: bool,
                 name: &str,
                 outcome: Outcome| {
        let provider = &report.providers[0];
        let rows = if shim {
            &provider.shim_calls
        } else {
            &provider.operations
        };
        rows.iter()
            .find(|row| row.name == name && row.outcome == outcome)
            .map_or(0, |row| row.count)
    };
    let before = state.providers.profile().report();
    // The page's read of a channel's newest messages asks for them dated, whether or not the
    // provider dates them.
    assert!(
        count(&before, false, "fetch_recent_dated", Outcome::Ok) >= 1,
        "{before:#?}"
    );

    // One 429, waited out: two requests at the far end, two calls here, one operation.
    fake.with(|state| state.rate_limit_once = Some(("conversations.history".to_owned(), 0)));
    let read = state
        .chat
        .fetch_recent(&ChannelId(TEAM.to_owned()), 10)
        .await
        .expect("read after a rate limit");
    assert_eq!(read.len(), 2);
    let _ = state.chat.identity().await.expect("auth.test");

    let report = state.providers.profile().report();
    assert_eq!(report.providers.len(), 1);
    let slack = &report.providers[0];
    assert_eq!(
        (slack.provider.as_str(), slack.name.as_str()),
        ("slack", "Slack")
    );
    for method in METHODS {
        let counted: u64 = slack
            .shim_calls
            .iter()
            .filter(|row| row.name == method)
            .map(|row| row.count)
            .sum();
        assert_eq!(
            counted,
            fake.calls(method) as u64,
            "{method}: the far end saw {} requests and the profile counted {counted}: {:#?}",
            fake.calls(method),
            slack.shim_calls
        );
    }
    let history = |outcome: Outcome| {
        slack
            .shim_calls
            .iter()
            .find(|row| row.name == "conversations.history" && row.outcome == outcome)
            .map_or(0, |row| row.count)
    };
    assert_eq!(history(Outcome::RateLimited), 1, "{:#?}", slack.shim_calls);
    assert!(history(Outcome::Ok) >= 2, "{:#?}", slack.shim_calls);
    assert!(
        slack.shim_calls.iter().all(|row| row.rows.is_none()),
        "a call has no rows"
    );
    assert_eq!(
        count(&report, false, "fetch_recent", Outcome::Ok),
        count(&before, false, "fetch_recent", Outcome::Ok) + 1,
        "one operation, however many calls it took"
    );
    assert_eq!(
        count(&report, true, "conversations.history", Outcome::Ok)
            + count(&report, true, "conversations.history", Outcome::RateLimited),
        count(&before, true, "conversations.history", Outcome::Ok) + 2,
        "{:#?}",
        slack.shim_calls
    );
    assert!(
        slack.content_bytes.messages >= 4,
        "{:?}",
        slack.content_bytes
    );

    // A refusal from Slack is a call with an outcome, and the operation fails with the same one.
    fake.with(|state| {
        state.fail.insert(
            "conversations.history".to_owned(),
            "channel_not_found".to_owned(),
        )
    });
    let _ = state
        .chat
        .fetch_recent(&ChannelId(TEAM.to_owned()), 10)
        .await
        .expect_err("refused");
    let report = state.providers.profile().report();
    let slack = &report.providers[0];
    assert!(slack
        .shim_calls
        .iter()
        .any(|row| row.name == "conversations.history" && row.outcome == Outcome::NotFound));
    assert!(slack
        .operations
        .iter()
        .any(|row| row.name == "fetch_recent" && row.outcome == Outcome::NotFound));
}

#[tokio::test]
async fn diagnostics_count_each_read_under_the_reason_it_was_made_for() {
    // `remote-ops-audit`, read reason tags. A read the page asked for counts under the page's word;
    // one the server made for itself — a catch-up's refresh in its own task, the reachability probe —
    // under the server's; one nobody said anything about, as untagged.
    let (state, discord) = vibe_talk::testing::state();
    let channel = ChannelId(vibe_talk::testing::READ_CHANNEL.to_owned());
    discord.seed(&channel, "Grace Hopper", "words");
    let router = vibe_talk::http::router(state.clone());
    let messages = format!("/api/v1/channels/{}/messages", channel.0);
    assert_eq!(
        get(&router, &format!("{messages}?reason=visible")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        get(&router, &format!("{messages}?limit=5")).await.0,
        StatusCode::OK
    );
    let mut refreshed = vibe_talk::recent::refresh(&state, &channel, 10, None);
    refreshed.wait_for(|over| *over).await.expect("finished");
    let channels = state.all_channels();
    vibe_talk::probe::probe_channels(state.chat.as_ref(), &channels).await;

    let (status, report) = get(&router, "/api/v1/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    let rows = report["upstream"]["providers"][0]["reasons"]
        .as_array()
        .expect("reasons")
        .clone();
    let count = |name: &str, reason: &str| -> u64 {
        rows.iter()
            .find(|row| row["name"] == name && row["reason"] == reason)
            .and_then(|row| row["count"].as_u64())
            .unwrap_or(0)
    };
    assert_eq!(count("fetch_recent_dated", "visible"), 1, "{rows:?}");
    assert_eq!(count("fetch_recent_dated", "untagged"), 1, "{rows:?}");
    assert_eq!(count("fetch_recent_dated", "recent"), 1, "{rows:?}");
    assert!(
        count("probe_channel", "probe") >= u64::try_from(channels.len()).unwrap(),
        "{rows:?}"
    );
    assert!(
        rows.iter().all(|row| row["name"] != "identity"),
        "only reads are counted by reason: {rows:?}"
    );
}

#[tokio::test]
async fn a_read_reason_changes_nothing_but_the_figures() {
    // `remote-ops-audit`: the reason is attribution only. The same read with any word, an unknown
    // word or none answers the same and costs the provider the same one read each time: no reason
    // shares, skips, keeps or freshens anything.
    let (state, discord) = vibe_talk::testing::state();
    let channel = ChannelId(vibe_talk::testing::READ_CHANNEL.to_owned());
    discord.seed(&channel, "Grace Hopper", "words");
    let router = vibe_talk::http::router(state.clone());
    let page = format!("/api/v1/channels/{}/page?limit=5", channel.0);
    let reads = || {
        state
            .providers
            .profile()
            .report()
            .providers
            .iter()
            .flat_map(|provider| provider.reasons.iter().map(|row| row.count))
            .sum::<u64>()
    };
    let (status, first) = get(&router, &page).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    for reason in ["poll", "pull", "enter", "side", "naming", "anything"] {
        let before = reads();
        let (status, body) = get(&router, &format!("{page}&reason={reason}")).await;
        assert_eq!(status, StatusCode::OK, "{reason}: {body}");
        assert_eq!(body, first, "reason={reason} changed the answer");
        assert_eq!(reads() - before, 1, "reason={reason} changed what was read");
    }
}

/// remote-ops-audit W11 `#287 bulk-reads`: a lookup by id on a provider without one of its own is
/// the reads the trait's default is made of, and both wrappers record exactly those, never a
/// `fetch_messages` the provider did not answer itself, so its diagnostics read as before.
#[tokio::test]
async fn an_emulated_lookup_by_id_is_timed_and_counted_as_the_reads_it_is_made_of() {
    let fake = Arc::new(FakeDiscord::new());
    let channel = ChannelId("C1".to_owned());
    let kept = fake.seed(&channel, "ada", "words of some length");
    let profile = UpstreamProfile::new();
    let profiled = ProfiledClient::new(
        Arc::clone(&fake) as Arc<dyn ChatClient>,
        profile.provider("fake", "Fake"),
    );
    let looked = profiled
        .fetch_messages(&channel, None, &[kept.clone(), MessageId("404".to_owned())])
        .await
        .expect("lookup");
    assert!(matches!(&looked[0], Lookup::Found(m) if m.id == kept));
    assert_eq!(looked[1], Lookup::Unseen, "a window proves nothing");
    let report = profile.report();
    let names: Vec<(&str, u64)> = report.providers[0]
        .operations
        .iter()
        .map(|row| (row.name, row.count))
        .collect();
    assert_eq!(names, [("fetch_recent_dated", 1)], "{names:?}");

    let counting = vibe_talk::testing::CountingChat::new(Arc::clone(&fake) as Arc<dyn ChatClient>);
    counting
        .fetch_messages(&channel, None, std::slice::from_ref(&kept))
        .await
        .expect("lookup");
    assert_eq!(counting.count("fetch_recent_dated"), 1);
    assert_eq!(counting.count("fetch_messages"), 0);
}

/// remote-ops-audit W12 `#287 bulk-reads`: a provider that does not read heads itself is asked the
/// one-message reads the default is made of, and each is timed and counted as itself; nothing is
/// ever timed as `channel_heads` for it.
#[tokio::test]
async fn an_emulated_head_read_is_timed_and_counted_as_the_reads_it_is_made_of() {
    let fake = Arc::new(FakeDiscord::new());
    let channels = [ChannelId("C1".to_owned()), ChannelId("C2".to_owned())];
    let newest = fake.seed(&channels[0], "ada", "words of some length");
    fake.register_channel(&channels[1]);
    let profile = UpstreamProfile::new();
    let profiled = ProfiledClient::new(
        Arc::clone(&fake) as Arc<dyn ChatClient>,
        profile.provider("fake", "Fake"),
    );
    let heads = profiled.channel_heads(&channels).await;
    assert_eq!(
        heads[0].1.as_ref().expect("a head").newest_id.as_ref(),
        Some(&newest)
    );
    assert_eq!(heads[1].1.as_ref().expect("a head").newest_id, None);
    let report = profile.report();
    let names: Vec<(&str, u64)> = report.providers[0]
        .operations
        .iter()
        .map(|row| (row.name, row.count))
        .collect();
    assert_eq!(names, [("fetch_recent", 2)], "{names:?}");

    let counting = vibe_talk::testing::CountingChat::new(Arc::clone(&fake) as Arc<dyn ChatClient>);
    counting.channel_heads(&channels).await;
    assert_eq!(counting.count("fetch_recent"), 2);
    assert_eq!(counting.count("channel_heads"), 0);
}

/// remote-ops-audit W13 `#287 bulk-reads`: a provider that does not read several channels together
/// itself is asked the recent reads the default is made of, and each is timed and counted as
/// itself; nothing is ever timed as `fetch_recent_many` for it.
#[tokio::test]
async fn an_emulated_read_of_several_channels_is_timed_and_counted_as_the_reads_it_is_made_of() {
    let fake = Arc::new(FakeDiscord::new());
    let channels = [ChannelId("C1".to_owned()), ChannelId("C2".to_owned())];
    let newest = fake.seed(&channels[0], "ada", "words of some length");
    fake.register_channel(&channels[1]);
    let asks: Vec<(ChannelId, RecentAsk)> = channels
        .iter()
        .map(|channel| (channel.clone(), RecentAsk::newest(5)))
        .collect();
    let profile = UpstreamProfile::new();
    let profiled = ProfiledClient::new(
        Arc::clone(&fake) as Arc<dyn ChatClient>,
        profile.provider("fake", "Fake"),
    );
    let answers = profiled.fetch_recent_many(&asks).await;
    let read = |at: usize| answers[at].1.as_ref().expect("a read");
    assert_eq!(
        read(0)
            .messages
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>(),
        [newest]
    );
    assert!(read(1).messages.is_empty());
    let report = profile.report();
    let names: Vec<(&str, u64)> = report.providers[0]
        .operations
        .iter()
        .map(|row| (row.name, row.count))
        .collect();
    assert_eq!(names, [("fetch_recent_dated", 2)], "{names:?}");

    let counting = vibe_talk::testing::CountingChat::new(Arc::clone(&fake) as Arc<dyn ChatClient>);
    counting.fetch_recent_many(&asks).await;
    assert_eq!(counting.count("fetch_recent_dated"), 2);
    assert_eq!(counting.count("fetch_recent_many"), 0);
}

/// A provider that sends one upstream request per page it reads ([`FakeDiscord`] underneath), and,
/// when `native_heads`, reads the heads of every channel asked about in one request more.
struct Billed {
    fake: Arc<FakeDiscord>,
    native_heads: bool,
}

#[async_trait]
impl ChatClient for Billed {
    fn provider_name(&self) -> &str {
        "Billed"
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        vibe_talk::upstream_profile::count_call();
        self.fake.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        vibe_talk::upstream_profile::count_call();
        self.fake.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        vibe_talk::upstream_profile::count_call();
        self.fake.post_message(channel, content, reply_to).await
    }

    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        ReadCaps {
            heads: if self.native_heads {
                Support::Native
            } else {
                Support::Emulated
            },
            ..ReadCaps::default()
        }
    }

    async fn channel_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        if !self.native_heads {
            return vibe_talk::chat::emulated_heads(self, channels).await;
        }
        vibe_talk::upstream_profile::count_call();
        let mut answers = Vec::new();
        for channel in channels {
            let newest = self.fake.fetch_recent(channel, 1).await.map(|messages| {
                let newest = messages.last();
                ChannelHead {
                    newest_id: newest.map(|message| message.id.clone()),
                    newest_at_ms: newest.and_then(|message| message.id.created_at_ms()),
                    ..ChannelHead::default()
                }
            });
            answers.push((channel.clone(), newest));
        }
        answers
    }
}

/// remote-ops-audit §4.1 `#287 bulk-reads`: diagnostics tell a native bulk read from its emulation.
/// The same heads of three channels are one `channel_heads` operation and one upstream request
/// from a provider that reads heads itself, and three `fetch_recent` operations of a request each
/// from one that does not; each operation's row says how many requests it sent.
#[tokio::test]
async fn a_native_bulk_read_and_its_emulation_differ_in_operations_and_upstream_calls() {
    let fake = Arc::new(FakeDiscord::new());
    let channels: Vec<ChannelId> = ["C1", "C2", "C3"]
        .iter()
        .map(|id| ChannelId((*id).to_owned()))
        .collect();
    for channel in &channels {
        fake.seed(channel, "ada", "words");
    }
    let mut answers = Vec::new();
    let mut rows = Vec::new();
    for native_heads in [true, false] {
        let profile = UpstreamProfile::new();
        let profiled = ProfiledClient::new(
            Arc::new(Billed {
                fake: Arc::clone(&fake),
                native_heads,
            }),
            profile.provider("billed", "Billed"),
        );
        let heads = profiled.channel_heads(&channels).await;
        answers.push(
            heads
                .into_iter()
                .map(|(channel, head)| (channel, head.expect("a head").newest_id))
                .collect::<Vec<_>>(),
        );
        rows.push(
            profile.report().providers[0]
                .operations
                .iter()
                .map(|row| (row.name, row.count, row.calls))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(answers[0], answers[1], "the same heads either way");
    assert_eq!(rows[0], [("channel_heads", 1, Some(1))], "native");
    assert_eq!(rows[1], [("fetch_recent", 3, Some(3))], "emulated");
}

/// A provider that reads heads, several channels and several threads itself when `native`, each
/// answered from [`FakeDiscord`] in one operation; otherwise none of them.
struct Bulk {
    fake: Arc<FakeDiscord>,
    native: bool,
}

#[async_trait]
impl ChatClient for Bulk {
    fn provider_name(&self) -> &str {
        "Bulk"
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.fake.identity().await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.fake.fetch_page(channel, limit, before, after).await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.fake.post_message(channel, content, reply_to).await
    }

    fn read_caps(&self, _channel: &ChannelId) -> ReadCaps {
        let support = if self.native {
            Support::Native
        } else {
            Support::Emulated
        };
        ReadCaps {
            heads: support,
            many_channels: support,
            many_threads: support,
            ..ReadCaps::default()
        }
    }

    async fn channel_heads(&self, channels: &[ChannelId]) -> HeadAnswers {
        vibe_talk::chat::emulated_heads(self.fake.as_ref(), channels).await
    }

    async fn fetch_recent_many(&self, asks: &[(ChannelId, RecentAsk)]) -> RecentAnswers {
        vibe_talk::chat::emulated_recent_many(self.fake.as_ref(), asks).await
    }

    async fn fetch_threads(&self, channel: &ChannelId, asks: &[ThreadAsk]) -> ThreadAnswers {
        vibe_talk::chat::emulated_threads(self.fake.as_ref(), channel, asks).await
    }
}

/// `#287 bulk-reads` with `remote-ops-audit`'s read reasons: a native read of heads, of several
/// channels or of several threads is counted once, under the reason it was asked for, as every
/// other read is; and its emulation counts each read it is made of under that same reason.
#[tokio::test]
async fn a_bulk_read_counts_under_the_reason_it_was_asked_for_native_or_emulated() {
    use vibe_talk::read_reason::ReadReason;

    let fake = Arc::new(FakeDiscord::new());
    let channels: Vec<ChannelId> = ["C1", "C2", "C3"]
        .iter()
        .map(|id| ChannelId((*id).to_owned()))
        .collect();
    for channel in &channels {
        fake.seed(channel, "ada", "words");
    }
    let asks: Vec<(ChannelId, RecentAsk)> = channels
        .iter()
        .map(|channel| (channel.clone(), RecentAsk::newest(5)))
        .collect();
    let threads = [ThreadAsk::newest("t1", 50), ThreadAsk::newest("t2", 50)];
    let mut counted = Vec::new();
    for native in [true, false] {
        let profile = UpstreamProfile::new();
        let profiled = ProfiledClient::new(
            Arc::new(Bulk {
                fake: Arc::clone(&fake),
                native,
            }),
            profile.provider("bulk", "Bulk"),
        );
        ReadReason::Recent
            .scope(async {
                profiled.channel_heads(&channels).await;
                profiled.fetch_recent_many(&asks).await;
            })
            .await;
        ReadReason::Tool
            .scope(profiled.fetch_threads(&channels[0], &threads))
            .await;
        counted.push(
            profile.report().providers[0]
                .reasons
                .iter()
                .map(|row| (row.name, row.reason, row.count))
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(
        counted[0],
        [
            ("channel_heads", ReadReason::Recent, 1),
            ("fetch_recent_many", ReadReason::Recent, 1),
            ("fetch_threads", ReadReason::Tool, 1),
        ],
        "native"
    );
    assert_eq!(
        counted[1],
        [
            ("fetch_recent", ReadReason::Recent, 3),
            ("fetch_recent_dated", ReadReason::Recent, 3),
            ("fetch_timeline", ReadReason::Tool, 2),
        ],
        "emulated"
    );
}

/// Slack's requests are counted against the operation that sent them: a `/count` since an
/// instant with 150 messages after it is one native `fetch_range` of eight
/// `conversations.history` calls; a read that met one 429 is one operation of two calls; and
/// every request the far end saw is on exactly one operation.
#[tokio::test]
async fn each_slack_request_is_counted_against_the_operation_that_sent_it() {
    let mut fixture = FakeState::default();
    let mut history = Vec::new();
    for i in 0..400 {
        history.push(profiled(&ts(i, 0), "UADA00001", "Ada", &format!("old {i}")));
    }
    for i in 0..150 {
        history.push(profiled(
            &ts(10_000 + i, 0),
            "UADA00001",
            "Ada",
            &format!("in {i}"),
        ));
    }
    fixture.history.insert(TEAM.to_owned(), history);
    let fake = Fake::start(fixture, 20).await;
    let state = slack_deployment(&fake).await;
    let router = vibe_talk::http::router(state.clone());
    let since =
        vibe_talk::clock::iso_from_ms((slack_fake::BASE_SECS + 10_000) * 1000).replace(':', "%3A");
    let (status, body) = get(
        &router,
        &format!("/api/v1/channels/{TEAM}/count?since={since}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["counted"], 150, "{body}");
    assert_eq!(fake.calls("conversations.history"), 8);
    let row = |name: &str| {
        let report = state.providers.profile().report();
        report.providers[0]
            .operations
            .iter()
            .find(|row| row.name == name && row.outcome == Outcome::Ok)
            .map(|row| (row.count, row.calls))
    };
    assert_eq!(
        row("fetch_range"),
        Some((1, Some(8))),
        "one operation, eight calls"
    );

    fake.with(|state| state.rate_limit_once = Some(("conversations.history".to_owned(), 0)));
    state
        .chat
        .fetch_recent(&ChannelId(TEAM.to_owned()), 10)
        .await
        .expect("read after a rate limit");
    assert_eq!(
        row("fetch_recent"),
        Some((1, Some(2))),
        "the 429 and the retry"
    );

    let report = state.providers.profile().report();
    let slack = &report.providers[0];
    let on_operations: u64 = slack.operations.iter().filter_map(|row| row.calls).sum();
    let at_the_far_end: usize = METHODS.iter().map(|method| fake.calls(method)).sum();
    assert_eq!(
        on_operations, at_the_far_end as u64,
        "every request is on one operation: {:#?}",
        slack.operations
    );
}

/// remote-ops-audit §4.1 `#287 bulk-reads`: diagnostics say, per provider, which bulk reads it
/// answers itself, under the names their operations are timed as, so a reader can tell which rows
/// are native answers. A provider with no bulk read of its own says every one is emulated; one that
/// reads heads natively says that one is native; the two are told apart in one deployment.
#[tokio::test]
async fn diagnostics_say_which_bulk_reads_each_provider_answers_itself() {
    const OTHER: &str = "C0OTHER001";
    let toml = format!(
        "[[providers]]\nkey = \"headed\"\nkind = \"slack\"\ntoken = \"xoxb-test\"\n\n\
         [[channels]]\nid = \"{OTHER}\"\nlabel = \"other\"\nwritable = false\n\
         provider = \"headed\"\n\n{}",
        vibe_talk::testing::config_toml()
    );
    let (mut state, discord, _elevenlabs) = vibe_talk::testing::state_from_toml(&toml);
    let headed = Arc::new(FakeDiscord::new());
    headed.seed(&ChannelId(OTHER.to_owned()), "ada", "words");
    let entries = vec![
        vibe_talk::providers::ProviderEntry {
            key: "discord".to_owned(),
            namespace: state.config.providers[0].namespace(),
            client: Arc::clone(&discord) as Arc<dyn ChatClient>,
            live_poll_seconds: 0,
        },
        vibe_talk::providers::ProviderEntry {
            key: "headed".to_owned(),
            namespace: state.config.providers[1].namespace(),
            client: Arc::new(Billed {
                fake: headed,
                native_heads: true,
            }),
            live_poll_seconds: 0,
        },
    ];
    let router = ChatRouter::new(entries, state.config.default_provider_key());
    state.replace_providers(Arc::new(router));
    let (status, report) = get(&vibe_talk::http::router(state), "/api/v1/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    let reads = |key: &str| -> Value {
        report["upstream"]["providers"]
            .as_array()
            .expect("providers")
            .iter()
            .find(|provider| provider["provider"] == key)
            .map(|provider| provider["reads"].clone())
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        reads("discord"),
        serde_json::json!({
            "fetch_range": "emulated", "fetch_messages": "emulated",
            "channel_heads": "emulated", "heads_cover_edits": false,
            "fetch_recent_many": "emulated", "fetch_threads": "emulated", "page_rows": 100
        }),
        "{report:#}"
    );
    assert_eq!(
        reads("headed"),
        serde_json::json!({
            "fetch_range": "emulated", "fetch_messages": "emulated",
            "channel_heads": "native", "heads_cover_edits": false,
            "fetch_recent_many": "emulated", "fetch_threads": "emulated", "page_rows": 100
        }),
        "{report:#}"
    );
}

/// The Slack client, built from configuration as `main` builds it, says in diagnostics what it
/// reads itself: a channel by time, messages by id and several threads, twenty rows a call; never
/// heads or several channels.
#[tokio::test]
async fn diagnostics_say_what_the_slack_client_reads_itself() {
    let fake = Fake::start(FakeState::default(), 20).await;
    let state = slack_deployment(&fake).await;
    let (status, report) = get(&vibe_talk::http::router(state), "/api/v1/diagnostics").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        report["upstream"]["providers"][0]["reads"],
        serde_json::json!({
            "fetch_range": "native", "fetch_messages": "native",
            "channel_heads": "emulated", "heads_cover_edits": false,
            "fetch_recent_many": "emulated", "fetch_threads": "native", "page_rows": 20
        }),
        "{report:#}"
    );
}
