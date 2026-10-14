//! `#208 persistent-cache`, milestone `cache-fill`: whose account the message cache's rows were
//! read as, and what it does when the provider stops letting the server read them.
//!
//! Every test opens a real cache file in its own directory, behind the server's sink, with a
//! hand-set clock placed near its fixture's times. The providers are scripted: each says it is the
//! account the test names, and fails the operations the test names, the way the test names.

mod slack_fake;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use serde_json::Value;
use tower::ServiceExt as _;
use vibe_talk::cache::access::{Dialect, CONFIRM_AFTER};
use vibe_talk::cache::sink::{
    Verification, CREDENTIAL_GRACE, SUSPENDED_RECHECK, VERIFY_RETRY_FIRST,
};
use vibe_talk::cache::{ChanKey, MessageCache, Scope as CacheScope};
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity, RecentRead, UpstreamClearOutcome};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
use vibe_talk::store::fake::FakeStore;
use vibe_talk::store::{StateStore as _, SummaryKey};
use vibe_talk::testing::{
    self, ManualClock, TempDir, TimelineChat, READ_CHANNEL, READ_TOKEN, WRITE_CHANNEL, WRITE_TOKEN,
};
use vibe_talk::threads::{TimelinePage, TimelineRequest, TimelineView};

/// 2026-08-18T12:00:00Z: where the fixtures' messages are written.
const T0: i64 = 1_787_054_400_000;
const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const WAIT: Duration = Duration::from_secs(20);

const FIRST: &str = "first";
const SECOND: &str = "second";

fn id(channel: &str) -> ChannelId {
    ChannelId(channel.to_owned())
}

fn account(user: &str) -> ChatIdentity {
    ChatIdentity {
        id: user.to_owned(),
        username: format!("{user}-name"),
        workspace: None,
    }
}

/// An operation the scripted provider fails, in one thread's scope only when `thread` says.
struct Rule {
    op: &'static str,
    thread: Option<String>,
    error: ChatError,
}

/// A Discord-protocol provider with timelines, read from a [`FakeDiscord`] through
/// [`TimelineChat`], that says it is the account the test sets, holds its answer while the test
/// says, and fails each operation the test names until told to stop.
struct Scripted {
    inner: TimelineChat,
    identity: Mutex<Result<ChatIdentity, ChatError>>,
    gate: tokio::sync::watch::Sender<bool>,
    rules: Mutex<Vec<Rule>>,
    identity_calls: AtomicUsize,
    probes: AtomicUsize,
}

impl Scripted {
    fn new(fake: &Arc<FakeDiscord>, user: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: TimelineChat::new(fake.clone()),
            identity: Mutex::new(Ok(account(user))),
            gate: tokio::sync::watch::Sender::new(true),
            rules: Mutex::new(Vec::new()),
            identity_calls: AtomicUsize::new(0),
            probes: AtomicUsize::new(0),
        })
    }

    /// Say from now on that the credential is `identity`'s, or fail with `error`.
    fn answer(&self, answer: Result<ChatIdentity, ChatError>) {
        *self.identity.lock().expect("identity") = answer;
    }

    /// Hold every identity answer until [`Scripted::release_identity`].
    fn hold_identity(&self) {
        self.gate.send_replace(false);
    }

    fn release_identity(&self) {
        self.gate.send_replace(true);
    }

    fn identity_calls(&self) -> usize {
        self.identity_calls.load(Ordering::SeqCst)
    }

    /// Fail `op` with `error` from now on, until [`Scripted::heal`].
    fn fail(&self, op: &'static str, error: ChatError) {
        self.rules.lock().expect("rules").push(Rule {
            op,
            thread: None,
            error,
        });
    }

    /// Fail `op` with `error`, for thread `thread` only.
    fn fail_thread(&self, op: &'static str, thread: &str, error: ChatError) {
        self.rules.lock().expect("rules").push(Rule {
            op,
            thread: Some(thread.to_owned()),
            error,
        });
    }

    /// Fail every read of a channel's messages with `error`.
    fn fail_channel_reads(&self, error: &ChatError) {
        for op in [
            "fetch_timeline",
            "fetch_page",
            "fetch_recent",
            "fetch_recent_dated",
            "probe_channel",
        ] {
            self.fail(op, error.clone());
        }
    }

    fn heal(&self) {
        self.rules.lock().expect("rules").clear();
    }

    /// The server's own confirming reads so far: a channel's sample, or one message of a thread.
    fn probes(&self) -> usize {
        self.probes.load(Ordering::SeqCst)
    }

    fn check(&self, op: &str, thread: Option<&str>) -> Result<(), ChatError> {
        let rules = self.rules.lock().expect("rules");
        match rules.iter().find(|rule| {
            rule.op == op && (rule.thread.is_none() || rule.thread.as_deref() == thread)
        }) {
            Some(rule) => Err(rule.error.clone()),
            None => Ok(()),
        }
    }
}

fn thread_of(request: &TimelineRequest) -> Option<&str> {
    (request.view == TimelineView::Thread)
        .then_some(request.thread_id.as_deref())
        .flatten()
}

#[async_trait]
impl ChatClient for Scripted {
    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn supports_threading(&self) -> bool {
        true
    }

    fn supports_upstream_clear(&self) -> bool {
        true
    }

    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        self.identity_calls.fetch_add(1, Ordering::SeqCst);
        let mut gate = self.gate.subscribe();
        let _ = gate.wait_for(|open| *open).await;
        self.identity.lock().expect("identity").clone()
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.check("fetch_page", None)?;
        self.inner.fetch_page(channel, limit, before, after).await
    }

    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.check("fetch_recent", None)?;
        self.inner.fetch_recent(channel, limit).await
    }

    async fn fetch_recent_dated(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<RecentRead, ChatError> {
        self.check("fetch_recent_dated", None)?;
        self.inner.fetch_recent_dated(channel, limit).await
    }

    async fn probe_channel(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        self.check("probe_channel", None)?;
        self.inner.fetch_recent(channel, limit).await
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
        if request.view == TimelineView::Thread && request.limit == 1 {
            self.probes.fetch_add(1, Ordering::SeqCst);
        }
        self.check("fetch_timeline", thread_of(request))?;
        self.inner.fetch_timeline(channel, request).await
    }

    async fn fetch_thread_message(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.check("fetch_thread_message", Some(thread_id))?;
        self.inner
            .fetch_thread_message(channel, thread_id, message_id)
            .await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.check("post_message", None)?;
        self.inner.post_message(channel, content, reply_to).await
    }

    async fn clear_upstream_through(
        &self,
        _channel: &ChannelId,
        through: &MessageId,
        dry_run: bool,
    ) -> Result<UpstreamClearOutcome, ChatError> {
        self.check("clear_upstream_through", None)?;
        Ok(UpstreamClearOutcome {
            scope: "space".to_owned(),
            target: through.as_str().to_owned(),
            previous: None,
            advanced: true,
            dry_run,
        })
    }
}

/// A server with two providers, `first` owning the read-only channel and `second` the writable
/// one, each answered by its own scripted client over one fake holding `count` messages in each
/// channel, a minute apart from [`T0`].
struct Deployment {
    state: AppState,
    fake: Arc<FakeDiscord>,
    store: Arc<FakeStore>,
    first: Arc<Scripted>,
    second: Arc<Scripted>,
}

fn two_providers_toml() -> String {
    format!(
        r#"
[auth]
read_token = "{READ_TOKEN}"
write_token = "{WRITE_TOKEN}"

[[providers]]
key = "{FIRST}"
kind = "discord"
token = "test-bot-token"

[[providers]]
key = "{SECOND}"
kind = "discord"
token = "test-bot-token-2"

[[channels]]
id = "{READ_CHANNEL}"
label = "build noise"
provider = "{FIRST}"

[[channels]]
id = "{WRITE_CHANNEL}"
label = "lead team"
writable = true
provider = "{SECOND}"
"#
    )
}

/// When the `n`th message of a channel was written: a minute apart from [`T0`], the writable
/// channel's half a minute later than the other's, so no two messages share an id.
fn written_at(channel: &str, n: i64) -> i64 {
    let offset = if channel == WRITE_CHANNEL {
        MINUTE / 2
    } else {
        0
    };
    T0 + n * MINUTE + offset
}

/// The ids of a channel's first ten messages, oldest first.
fn seeded(channel: &str) -> Vec<MessageId> {
    (0..10)
        .map(|n| MessageId::at_time_ms(written_at(channel, n)))
        .collect()
}

/// The upstream half of a [`Deployment`]: the providers say they are `first_user` and
/// `second_user`. Nothing is behind the sink yet.
fn upstream(count: i64, first_user: &str, second_user: &str) -> Deployment {
    let (mut state, fake, store) = testing::state_with_store_from_toml(&two_providers_toml());
    for channel in [READ_CHANNEL, WRITE_CHANNEL] {
        for n in 0..count {
            fake.seed_at(&id(channel), "Ada", "a message", written_at(channel, n));
        }
    }
    let first = Scripted::new(&fake, first_user);
    let second = Scripted::new(&fake, second_user);
    let entry = |key: &str, index: usize, client: Arc<Scripted>| ProviderEntry {
        key: key.to_owned(),
        namespace: state.config.providers[index].namespace(),
        client,
        live_poll_seconds: 0,
    };
    let router = ChatRouter::new(
        vec![
            entry(FIRST, 0, first.clone()),
            entry(SECOND, 1, second.clone()),
        ],
        None,
    );
    state.replace_providers(Arc::new(router));
    Deployment {
        state,
        fake,
        store,
        first,
        second,
    }
}

impl Deployment {
    fn cache(&self) -> Arc<MessageCache> {
        self.state.message_cache.cache().expect("installed")
    }

    /// Close the cache behind the server, so the next start can open the same file.
    fn close(self) {
        let cache = self.cache();
        assert!(cache.quiesce_blocking(WAIT));
        drop(cache);
        drop(self.state.message_cache.uninstall());
    }
}

fn first_chan() -> ChanKey {
    ChanKey::new(FIRST, READ_CHANNEL)
}

fn second_chan() -> ChanKey {
    ChanKey::new(SECOND, WRITE_CHANNEL)
}

/// The messages the file holds for `chan`'s main scope, once the writer has caught up.
fn held(cache: &MessageCache, chan: &ChanKey) -> usize {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache
        .newest(chan, &CacheScope::Main, 1_000)
        .served()
        .expect("served")
        .len()
}

fn stored_account(cache: &MessageCache, provider: &str) -> Option<vibe_talk::cache::Account> {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.account(provider).served().expect("an account read")
}

async fn call(state: &AppState, method: &str, uri: &str, token: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
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

/// Read a channel's newest main page with the write token, which fills the cache.
async fn read_main(state: &AppState, channel: &str) -> StatusCode {
    call(
        state,
        "GET",
        &format!("/api/v1/channels/{channel}/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await
    .0
}

// ---- §6.2: whose account the rows were read as ------------------------------------------------

#[tokio::test]
async fn the_first_start_records_each_account_and_another_account_erases_only_its_provider() {
    let dir = TempDir::new("cache-access-identity");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let d = upstream(10, "UA1", "UB1");
        let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
        let d = Deployment { state, ..d };
        let cache = d.cache();
        assert_eq!(
            stored_account(&cache, FIRST).map(|a| (a.account_id, a.workspace_id)),
            Some(("UA1".to_owned(), None)),
            "the first start writes the account each provider's own client named"
        );
        assert!(stored_account(&cache, SECOND)
            .expect("written")
            .is("UB1", None));
        assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
        assert_eq!(read_main(&d.state, WRITE_CHANNEL).await, StatusCode::OK);
        assert_eq!(held(&cache, &first_chan()), 10);
        assert_eq!(held(&cache, &second_chan()), 10);
        d.close();
    }

    // The next start: the first provider's credential is now another account's.
    let d = upstream(10, "UA2", "UB1");
    let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
    let d = Deployment { state, ..d };
    let cache = d.cache();
    assert_eq!(
        held(&cache, &first_chan()),
        0,
        "what was read as another account is erased"
    );
    assert!(stored_account(&cache, FIRST)
        .expect("the new account")
        .is("UA2", None));
    assert_eq!(
        held(&cache, &second_chan()),
        10,
        "the other provider's account matched: nothing of it is erased"
    );
    assert!(stored_account(&cache, SECOND)
        .expect("kept")
        .is("UB1", None));
    // And the first provider fills again, as its new account.
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 10);
}

#[tokio::test]
async fn no_fill_begins_before_the_providers_account_is_checked() {
    let dir = TempDir::new("cache-access-unverified");
    let clock = ManualClock::at(T0 + HOUR);
    let d = upstream(10, "UA1", "UB1");
    d.first.hold_identity();
    let state = testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
    let d = Deployment { state, ..d };
    let cache = d.cache();
    assert!(
        !d.state
            .message_cache
            .identities_verified(Duration::from_millis(200))
            .await,
        "the first provider has not answered"
    );
    assert!(d.first.identity_calls() >= 1 && d.second.identity_calls() >= 1);
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Unverified
    );
    assert_eq!(
        d.state.message_cache.verification(SECOND),
        Verification::Verified
    );

    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(read_main(&d.state, WRITE_CHANNEL).await, StatusCode::OK);
    assert_eq!(
        held(&cache, &first_chan()),
        0,
        "a read of an unverified provider fills nothing"
    );
    assert_eq!(stored_account(&cache, FIRST), None);
    assert_eq!(
        held(&cache, &second_chan()),
        10,
        "each provider is checked on its own"
    );

    d.first.release_identity();
    assert!(d.state.message_cache.identities_verified(WAIT).await);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 10);
}

#[tokio::test]
async fn an_account_that_cannot_be_asked_for_neither_erases_nor_fills_and_is_asked_again_later() {
    let dir = TempDir::new("cache-access-unanswered");
    let clock = ManualClock::at(T0 + HOUR);
    {
        let d = upstream(10, "UA1", "UB1");
        let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
        let d = Deployment { state, ..d };
        assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
        assert_eq!(held(&d.cache(), &first_chan()), 10);
        d.close();
    }

    // The next start: the provider cannot be reached to say which account it is, then answers
    // with a 503.
    let d = upstream(10, "UA1", "UB1");
    d.first
        .answer(Err(ChatError::Transport("connection refused".to_owned())));
    let state = testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
    let d = Deployment { state, ..d };
    let cache = d.cache();
    assert!(
        !d.state
            .message_cache
            .identities_verified(Duration::from_millis(200))
            .await
    );
    tokio::time::pause();
    assert_eq!(d.first.identity_calls(), 1);
    assert_eq!(
        held(&cache, &first_chan()),
        10,
        "an unanswered question erases nothing"
    );
    assert!(stored_account(&cache, FIRST).expect("kept").is("UA1", None));

    // A message arrives upstream; reading it fills nothing while the account is unchecked.
    d.fake
        .seed_at(&id(READ_CHANNEL), "Ada", "a new one", T0 + 30 * MINUTE);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 10, "nothing is filled");

    // It is asked again after a wait that doubles: five seconds, then ten.
    d.first.answer(Err(ChatError::Status {
        status: 503,
        body: "unavailable".to_owned(),
    }));
    tokio::time::advance(VERIFY_RETRY_FIRST).await;
    settle_tasks().await;
    assert_eq!(d.first.identity_calls(), 2);
    tokio::time::advance(VERIFY_RETRY_FIRST).await;
    settle_tasks().await;
    assert_eq!(d.first.identity_calls(), 2, "the second wait is longer");
    tokio::time::advance(VERIFY_RETRY_FIRST + Duration::from_secs(1)).await;
    settle_tasks().await;
    assert_eq!(d.first.identity_calls(), 3);
    assert_eq!(
        held(&cache, &first_chan()),
        10,
        "a 5xx erases nothing either"
    );

    // Once it answers with the same account, nothing is erased and filling begins.
    d.first.answer(Ok(account("UA1")));
    tokio::time::advance(VERIFY_RETRY_FIRST * 4).await;
    assert!(d.state.message_cache.identities_verified(WAIT).await);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 11);
}

/// Let every task woken so far run to where it waits again, including work it handed to the
/// blocking pool.
async fn settle_tasks() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
    tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(50)))
        .await
        .expect("the blocking pool runs");
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

const SLACK: &str = "slack";
const SLACK_CH: &str = "C0000MAIN1";

fn slack_ms(seconds: i64) -> i64 {
    (slack_fake::BASE_SECS + seconds) * 1_000
}

/// A Slack server against `fake`, with a cache in `dir`.
async fn slack_deployment(
    fake: &slack_fake::Fake,
    dir: &TempDir,
    clock: &Arc<ManualClock>,
) -> AppState {
    slack_deployment_and_store(fake, dir, clock).await.0
}

/// [`slack_deployment`], and the store behind it.
async fn slack_deployment_and_store(
    fake: &slack_fake::Fake,
    dir: &TempDir,
    clock: &Arc<ManualClock>,
) -> (AppState, Arc<FakeStore>) {
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
id = "{SLACK_CH}"
label = "team"
writable = true
"#,
        base = fake.base,
        token = slack_fake::TOKEN,
    );
    let (mut state, _discord, store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    (
        testing::with_message_cache(state, dir.path(), clock.clone()).await,
        store,
    )
}

fn slack_history() -> slack_fake::FakeState {
    let mut fixture = slack_fake::FakeState::default();
    let list = (0..6)
        .map(|n| {
            slack_fake::profiled(
                &slack_fake::ts(n * 60, 0),
                "UADA00001",
                "Ada",
                &format!("message {n}"),
            )
        })
        .collect();
    fixture.history.insert(SLACK_CH.to_owned(), list);
    fixture
}

#[tokio::test]
async fn slacks_workspace_is_auth_tests_team_id_and_another_workspace_erases_the_provider() {
    let fake = slack_fake::Fake::start(slack_history(), 10).await;
    let dir = TempDir::new("cache-access-slack-team");
    let clock = ManualClock::at(slack_ms(3_600));
    let chan = ChanKey::new(SLACK, SLACK_CH);
    {
        let state = slack_deployment(&fake, &dir, &clock).await;
        let cache = state.message_cache.cache().expect("installed");
        let recorded = stored_account(&cache, SLACK).expect("written");
        assert_eq!(recorded.account_id, slack_fake::SELF_USER);
        assert_eq!(recorded.workspace_id.as_deref(), Some(slack_fake::TEAM));
        assert_eq!(read_main(&state, SLACK_CH).await, StatusCode::OK);
        assert_eq!(held(&cache, &chan), 6);
        assert!(cache.quiesce_blocking(WAIT));
        drop(cache);
        drop(state.message_cache.uninstall());
    }

    // The same user id, in another workspace: another account.
    fake.with(|state| state.team = Some("T0000OTHER".to_owned()));
    let state = slack_deployment(&fake, &dir, &clock).await;
    let cache = state.message_cache.cache().expect("installed");
    assert_eq!(held(&cache, &chan), 0, "read in another workspace: erased");
    let recorded = stored_account(&cache, SLACK).expect("written");
    assert_eq!(recorded.account_id, slack_fake::SELF_USER);
    assert_eq!(recorded.workspace_id.as_deref(), Some("T0000OTHER"));
}

// ---- §6.3: access signals -----------------------------------------------------------------------

/// A little past a deadline: the timer rounds a deadline up to its next millisecond.
const PAST: Duration = Duration::from_millis(5);

/// Discord's own answer for a channel this credential cannot see.
fn unknown_channel() -> ChatError {
    ChatError::Status {
        status: 404,
        body: r#"{"message": "Unknown Channel", "code": 10003}"#.to_owned(),
    }
}

fn missing_access() -> ChatError {
    ChatError::Status {
        status: 403,
        body: r#"{"message": "Missing Access", "code": 50001}"#.to_owned(),
    }
}

fn unauthorized() -> ChatError {
    ChatError::Status {
        status: 401,
        body: r#"{"message": "401: Unauthorized", "code": 0}"#.to_owned(),
    }
}

/// A Slack failure, as the Slack client writes one.
fn slack_failure(status: u16, method: &str, code: &str) -> ChatError {
    ChatError::Status {
        status,
        body: format!("{method} answered {code}"),
    }
    .with_provider("Slack")
}

/// Every message the file holds, whatever is blocked, once the writer has caught up.
fn total(cache: &MessageCache) -> u64 {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.counts().served().expect("counts").messages
}

fn blocked(cache: &MessageCache, chan: &ChanKey, scope: &CacheScope) -> bool {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.newest(chan, scope, 10) == vibe_talk::cache::Read::Blocked
}

/// A [`Deployment`] with the cache on, both channels read once with the write token: ten rows in
/// each.
async fn filled(dir: &TempDir, clock: &Arc<ManualClock>) -> Deployment {
    let d = upstream(10, "UA1", "UB1");
    let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
    let d = Deployment { state, ..d };
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(read_main(&d.state, WRITE_CHANNEL).await, StatusCode::OK);
    assert_eq!(total(&d.cache()), 20);
    d
}

fn summary_key(state: &AppState, channel: &str, message: &MessageId) -> SummaryKey {
    SummaryKey {
        channel: id(channel),
        message: message.clone(),
        content_hash: vibe_talk::summarize::content_hash("a message"),
        version: state.summary_version.to_string(),
    }
}

async fn file_summary(store: &FakeStore, key: &SummaryKey) {
    store
        .cache_summary(key, "a summary", None)
        .await
        .expect("filed");
}

async fn kept(store: &FakeStore, key: &SummaryKey) -> bool {
    store.cached_summary(key).await.expect("read").is_some()
}

#[tokio::test]
async fn each_excluded_failure_leaves_rows_and_serving_unchanged() {
    let dir = TempDir::new("cache-access-excluded");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    tokio::time::pause();
    let unchanged = |what: &str| {
        assert_eq!(total(&cache), 20, "{what}: nothing erased");
        assert_eq!(held(&cache, &first_chan()), 10, "{what}: still served");
        assert!(
            d.state.providers.access().suspects().is_empty(),
            "{what}: nothing in doubt, so nothing to confirm"
        );
        assert_eq!(
            d.state.message_cache.verification(FIRST),
            Verification::Verified,
            "{what}: not suspended"
        );
    };

    // A 403 from a post.
    d.first.fail(
        "post_message",
        ChatError::Status {
            status: 403,
            body: r#"{"message": "Missing Permissions", "code": 50013}"#.to_owned(),
        },
    );
    assert!(d
        .state
        .chat
        .post_message(&id(READ_CHANNEL), "hello", None)
        .await
        .is_err());
    unchanged("a 403 from a post");
    d.first.heal();

    // A 404 from one message's lookup: the provider no longer holds that one message.
    d.first.fail("fetch_thread_message", unknown_channel());
    assert!(d
        .state
        .chat
        .fetch_thread_message(
            &id(READ_CHANNEL),
            seeded(READ_CHANNEL)[2].as_str(),
            &seeded(READ_CHANNEL)[3]
        )
        .await
        .is_err());
    unchanged("a 404 from one message's lookup");
    d.first.heal();

    // A 404 from moving the provider's read cursor: it no longer holds that one message.
    d.first.fail("clear_upstream_through", unknown_channel());
    let newest = seeded(READ_CHANNEL).pop().expect("a message");
    assert!(d
        .state
        .chat
        .clear_upstream_through(&id(READ_CHANNEL), &newest, false)
        .await
        .is_err());
    unchanged("a 404 from clear_upstream_through");
    d.first.heal();

    // A 400 from a read of the channel.
    d.first.fail(
        "fetch_timeline",
        ChatError::Status {
            status: 400,
            body: r#"{"message": "Invalid Form Body", "code": 50035}"#.to_owned(),
        },
    );
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    unchanged("a 400");
    d.first.heal();

    // And nothing comes of any of it later.
    tokio::time::advance(CONFIRM_AFTER * 2 + PAST).await;
    settle_tasks().await;
    unchanged("twenty minutes on");
    assert_eq!(d.first.probes(), 0, "no confirming read");
}

#[tokio::test]
async fn slacks_excluded_codes_leave_rows_and_serving_unchanged() {
    let mut fixture = slack_history();
    // An author whose profile the page does not carry, and whom users.info does not know.
    fixture
        .history
        .get_mut(SLACK_CH)
        .expect("history")
        .push(slack_fake::plain(
            &slack_fake::ts(400, 0),
            "UNOBODY01",
            "who wrote this",
        ));
    let fake = slack_fake::Fake::start(fixture, 10).await;
    let dir = TempDir::new("cache-access-slack-excluded");
    let clock = ManualClock::at(slack_ms(3_600));
    let state = slack_deployment(&fake, &dir, &clock).await;
    let cache = state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(SLACK, SLACK_CH);

    // A 404 user_not_found from the author lookup inside the read: the read still answers.
    assert_eq!(read_main(&state, SLACK_CH).await, StatusCode::OK);
    assert!(fake.calls("users.info") >= 1, "the author was looked up");
    assert_eq!(held(&cache, &chan), 7, "filled, and served");
    assert!(state.providers.access().suspects().is_empty());

    // A missing_scope 403, and a message_not_found 404, from history: a read of the channel.
    for code in ["missing_scope", "message_not_found"] {
        fake.with(|state| {
            state
                .fail
                .insert("conversations.history".to_owned(), code.to_owned());
        });
        assert_ne!(read_main(&state, SLACK_CH).await, StatusCode::OK);
        assert_eq!(held(&cache, &chan), 7, "{code}: still held, still served");
        assert!(
            state.providers.access().suspects().is_empty(),
            "{code}: nothing in doubt, so nothing to confirm"
        );
        assert_eq!(
            state.message_cache.verification(SLACK),
            Verification::Verified,
            "{code}"
        );
    }
}

/// Plan section 6.3: on Slack only `channel_not_found`, `not_in_channel` and `thread_not_found`
/// say access is lost. An HTTP 404 or 403 that carries no Slack code at all was answered by
/// something in front of Slack (a shim route that is gone or misrouted, an intermediary refusing
/// the destination, a wrong `api_base`). Seen by a READ-token read, and again by anything the
/// server reads ten minutes on, it puts nothing in doubt, erases nothing, and deletes none of the
/// store's summaries.
async fn a_slack_refusal_with_no_slack_code_changes_nothing(status: u16, body: &str) {
    let fake = slack_fake::Fake::start(slack_history(), 10).await;
    let dir = TempDir::new("cache-access-slack-http");
    let clock = ManualClock::at(slack_ms(3_600));
    let (state, store) = slack_deployment_and_store(&fake, &dir, &clock).await;
    let cache = state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(SLACK, SLACK_CH);
    assert_eq!(read_main(&state, SLACK_CH).await, StatusCode::OK);
    assert_eq!(total(&cache), 6, "filled");
    let key = SummaryKey {
        channel: id(SLACK_CH),
        message: MessageId(slack_fake::ts(120, 0)),
        content_hash: vibe_talk::summarize::content_hash("message 2"),
        version: state.summary_version.to_string(),
    };
    file_summary(&store, &key).await;
    tokio::time::pause();

    fake.with(|s| {
        s.http_fail.insert(
            "conversations.history".to_owned(),
            (status, body.to_owned()),
        );
    });
    let (answered, _) = call(
        &state,
        "GET",
        &format!("/api/v1/channels/{SLACK_CH}/timeline?view=main&limit=50"),
        READ_TOKEN,
    )
    .await;
    assert_ne!(answered, StatusCode::OK);
    assert!(
        state.providers.access().suspects().is_empty(),
        "a Slack {status} with no Slack code put the channel in doubt"
    );
    assert!(
        !blocked(&cache, &chan, &CacheScope::Main),
        "a Slack {status} with no Slack code blocked the channel"
    );
    tokio::time::advance(CONFIRM_AFTER + PAST).await;
    settle_tasks().await;
    settle_tasks().await;
    assert_eq!(
        total(&cache),
        6,
        "a Slack {status} with no Slack code erased the channel"
    );
    assert!(
        kept(&store, &key).await,
        "a Slack {status} with no Slack code deleted the store's summaries"
    );
    assert_eq!(
        state.message_cache.verification(SLACK),
        Verification::Verified
    );
}

#[tokio::test]
async fn a_slack_http_404_with_no_slack_code_never_erases_nor_forgets_summaries() {
    a_slack_refusal_with_no_slack_code_changes_nothing(
        404,
        "<html><body><h1>404 Not Found</h1></body></html>",
    )
    .await;
}

#[tokio::test]
async fn a_slack_http_403_with_no_slack_code_never_erases_nor_forgets_summaries() {
    a_slack_refusal_with_no_slack_code_changes_nothing(403, "Forbidden: destination not allowed")
        .await;
}

/// `#307 slack-reply-live`: the live poller reads a Slack conversation through `fetch_live`, which
/// reads its history as `fetch_page` did. Slack's `channel_not_found` there is heard as a read of
/// the channel refused, whoever made the read: the channel is in doubt and blocked, and the
/// poller's next read that succeeds lifts both.
#[tokio::test]
async fn the_slack_pollers_live_read_refused_puts_the_channel_in_doubt_and_its_next_read_lifts_it()
{
    let fake = slack_fake::Fake::start(slack_history(), 10).await;
    let dir = TempDir::new("cache-access-slack-live");
    let clock = ManualClock::at(slack_ms(3_600));
    let state = slack_deployment(&fake, &dir, &clock).await;
    let cache = state.message_cache.cache().expect("installed");
    let chan = ChanKey::new(SLACK, SLACK_CH);
    let channel = id(SLACK_CH);
    let mut cursor = None;
    vibe_talk::live::poll_once(state.chat.as_ref(), &state.live, &channel, 50, &mut cursor)
        .await
        .expect("seeded");
    assert!(state.providers.access().suspects().is_empty());

    fake.with(|s| {
        s.fail.insert(
            "conversations.history".to_owned(),
            "channel_not_found".to_owned(),
        );
    });
    vibe_talk::live::poll_once(state.chat.as_ref(), &state.live, &channel, 50, &mut cursor)
        .await
        .expect_err("refused");
    let suspects = state.providers.access().suspects();
    assert_eq!(suspects.len(), 1, "{suspects:?}");
    assert_eq!(suspects[0].provider_key, SLACK);
    assert_eq!(suspects[0].channel, channel);
    assert_eq!(suspects[0].thread, None);
    assert!(blocked(&cache, &chan, &CacheScope::Main));

    fake.with(|s| s.fail.clear());
    vibe_talk::live::poll_once(state.chat.as_ref(), &state.live, &channel, 50, &mut cursor)
        .await
        .expect("read again");
    assert!(
        state.providers.access().suspects().is_empty(),
        "the poller's read that succeeded cleared the doubt"
    );
    assert!(!blocked(&cache, &chan, &CacheScope::Main));
    assert_eq!(total(&cache), 6, "nothing erased");
}

#[tokio::test]
async fn one_refusal_blocks_without_erasing_and_the_servers_own_read_ten_minutes_on_erases() {
    let dir = TempDir::new("cache-access-confirmed");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    let lost = summary_key(&d.state, READ_CHANNEL, &seeded(READ_CHANNEL)[2]);
    let other = summary_key(&d.state, WRITE_CHANNEL, &seeded(WRITE_CHANNEL)[2]);
    file_summary(&d.store, &lost).await;
    file_summary(&d.store, &other).await;
    tokio::time::pause();

    d.first.fail_channel_reads(&unknown_channel());
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(
        blocked(&cache, &first_chan(), &CacheScope::Main),
        "blocked at once"
    );
    assert_eq!(total(&cache), 20, "but nothing is erased on one refusal");
    assert_eq!(
        held(&cache, &second_chan()),
        10,
        "another channel is served"
    );

    // Not before ten minutes.
    tokio::time::advance(CONFIRM_AFTER - Duration::from_secs(1)).await;
    settle_tasks().await;
    assert_eq!(d.first.probes(), 0);
    assert_eq!(total(&cache), 20);

    // Then the server reads it itself, is refused the same way, and erases the channel and the
    // store's summaries of it.
    tokio::time::advance(Duration::from_secs(1) + PAST).await;
    settle_tasks().await;
    assert_eq!(
        d.first.probes(),
        1,
        "one confirming read, on the server's account"
    );
    assert_eq!(total(&cache), 10, "the channel is erased");
    assert_eq!(held(&cache, &first_chan()), 0, "and holds nothing to serve");
    settle_tasks().await;
    assert!(!kept(&d.store, &lost).await, "its summaries are forgotten");
    assert!(kept(&d.store, &other).await, "another channel's are kept");
    assert_eq!(held(&cache, &second_chan()), 10);

    // Access comes back: a read fills it again.
    d.first.heal();
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 10);
}

#[tokio::test]
async fn a_successful_read_in_between_lifts_the_block_and_nothing_is_erased() {
    let dir = TempDir::new("cache-access-cleared");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    tokio::time::pause();

    d.first.fail("fetch_timeline", missing_access());
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(blocked(&cache, &first_chan(), &CacheScope::Main));
    d.first.heal();
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 10, "served again");

    tokio::time::advance(CONFIRM_AFTER * 2 + PAST).await;
    settle_tasks().await;
    assert_eq!(d.first.probes(), 0, "nothing left to confirm");
    assert_eq!(total(&cache), 20, "nothing erased");
}

#[tokio::test]
async fn thread_not_found_doubts_only_that_thread_and_confirming_it_erases_only_the_thread() {
    let dir = TempDir::new("cache-access-thread");
    let clock = ManualClock::at(T0 + HOUR);
    let d = upstream(10, "UA1", "UB1");
    let ids = seeded(READ_CHANNEL);
    let root = ids[5].clone();
    for n in [7, 9] {
        d.fake.set_reply_to(&ids[n], &root);
    }
    let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
    let d = Deployment { state, ..d };
    // The first provider fails the way Slack's client does, and is read as Slack.
    d.state
        .providers
        .access()
        .set_dialect(FIRST, Dialect::Slack);
    let cache = d.cache();
    let thread = CacheScope::Thread(root.as_str().to_owned());
    let thread_uri = format!(
        "/api/v1/channels/{READ_CHANNEL}/timeline?view=thread&thread_id={}",
        root.as_str()
    );
    // The thread first, so its replies' home is the thread.
    assert_eq!(
        call(&d.state, "GET", &thread_uri, WRITE_TOKEN).await.0,
        StatusCode::OK
    );
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(total(&cache), 10);
    assert_eq!(
        cache
            .newest(&first_chan(), &thread, 10)
            .served()
            .expect("served")
            .len(),
        3,
        "the root and its two replies"
    );
    let reply = summary_key(&d.state, READ_CHANNEL, &ids[7]);
    let elsewhere = summary_key(&d.state, READ_CHANNEL, &ids[2]);
    file_summary(&d.store, &reply).await;
    file_summary(&d.store, &elsewhere).await;
    tokio::time::pause();

    d.first.fail_thread(
        "fetch_timeline",
        root.as_str(),
        slack_failure(404, "conversations.replies", "thread_not_found"),
    );
    assert_ne!(
        call(&d.state, "GET", &thread_uri, WRITE_TOKEN).await.0,
        StatusCode::OK
    );
    assert!(
        blocked(&cache, &first_chan(), &thread),
        "the thread is blocked"
    );
    assert!(
        !blocked(&cache, &first_chan(), &CacheScope::Main),
        "the channel is not"
    );
    assert_eq!(total(&cache), 10, "nothing erased yet");

    tokio::time::advance(CONFIRM_AFTER + PAST).await;
    settle_tasks().await;
    assert_eq!(d.first.probes(), 1);
    assert_eq!(total(&cache), 8, "the thread's two replies are erased");
    assert!(cache
        .newest(&first_chan(), &thread, 10)
        .served()
        .expect("served again, and empty")
        .is_empty());
    assert_eq!(
        held(&cache, &first_chan()),
        8,
        "the channel's other messages stay"
    );
    assert!(d.state.message_cache.orphans_settled(WAIT).await);
    assert!(
        !kept(&d.store, &reply).await,
        "a reply's summary is forgotten"
    );
    assert!(
        kept(&d.store, &elsewhere).await,
        "the rest of the channel's are kept"
    );
}

#[tokio::test]
async fn a_refused_credential_suspends_the_provider_until_its_account_matches_again() {
    let dir = TempDir::new("cache-access-suspended");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    tokio::time::pause();

    d.first.fail("fetch_timeline", unauthorized());
    d.first.answer(Err(unauthorized()));
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Suspended
    );
    assert!(
        blocked(&cache, &first_chan(), &CacheScope::Main),
        "nothing served"
    );
    assert_eq!(
        held(&cache, &second_chan()),
        10,
        "another provider is served"
    );
    assert_eq!(total(&cache), 20, "nor erased");

    // Fills stop too: the provider answers reads again, but its account is not checked.
    d.first.heal();
    d.fake
        .seed_at(&id(READ_CHANNEL), "Ada", "a new one", T0 + 30 * MINUTE);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(total(&cache), 20, "nothing filled while suspended");

    // It is asked every ten minutes; a refusal keeps it suspended.
    let asked = d.first.identity_calls();
    tokio::time::advance(SUSPENDED_RECHECK + PAST).await;
    settle_tasks().await;
    assert_eq!(d.first.identity_calls(), asked + 1);
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Suspended
    );

    // The account answers again, and matches: served and filled again, nothing erased.
    d.first.answer(Ok(account("UA1")));
    tokio::time::advance(SUSPENDED_RECHECK + PAST).await;
    assert!(d.state.message_cache.identities_verified(WAIT).await);
    assert_eq!(held(&cache, &first_chan()), 10);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &first_chan()), 11);
}

#[tokio::test]
async fn a_revoked_credential_erases_at_once_and_a_refusal_lasting_a_day_erases() {
    let dir = TempDir::new("cache-access-revoked");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    tokio::time::pause();

    // Revoked: erased at once, its account with it. The first provider fails the way Slack's
    // client does, and is read as Slack: only Slack says a credential is gone for good.
    d.state
        .providers
        .access()
        .set_dialect(FIRST, Dialect::Slack);
    let revoked = slack_failure(401, "conversations.history", "token_revoked");
    d.first.fail("fetch_timeline", revoked.clone());
    d.first.answer(Err(revoked));
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(total(&cache), 10, "the provider's rows are erased at once");
    assert_eq!(stored_account(&cache, FIRST), None);
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Suspended
    );

    // Refused but not revoked: kept, until the refusal has lasted a day.
    d.second.fail("fetch_timeline", unauthorized());
    d.second.answer(Err(unauthorized()));
    assert_ne!(read_main(&d.state, WRITE_CHANNEL).await, StatusCode::OK);
    assert!(blocked(&cache, &second_chan(), &CacheScope::Main));
    tokio::time::advance(SUSPENDED_RECHECK + PAST).await;
    settle_tasks().await;
    assert_eq!(total(&cache), 10, "ten minutes of refusals erase nothing");
    tokio::time::advance(CREDENTIAL_GRACE).await;
    settle_tasks().await;
    assert_eq!(total(&cache), 0, "a day of them does");

    // A credential that is accepted again starts over, with nothing held.
    d.second.heal();
    d.second.answer(Ok(account("UB1")));
    tokio::time::advance(SUSPENDED_RECHECK + PAST).await;
    settle_tasks().await;
    assert_eq!(
        d.state.message_cache.verification(SECOND),
        Verification::Verified
    );
    assert!(stored_account(&cache, SECOND)
        .expect("recorded again")
        .is("UB1", None));
    assert_eq!(read_main(&d.state, WRITE_CHANNEL).await, StatusCode::OK);
    assert_eq!(held(&cache, &second_chan()), 10);
}

// ---- §3.4: what reaches the file while a provider's account is not checked against it ---------

use vibe_talk::live::LiveKind;

/// The text the file holds for `message` of the read-only channel, once the writer has caught up.
fn text_of(cache: &MessageCache, message: &MessageId) -> Option<String> {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache
        .message(&first_chan(), message)
        .served()
        .expect("a read")
        .map(|held| held.message.content)
}

/// Whether the read-only channel is suspect, once the writer has applied everything offered: a
/// read that lifts the suspicion does so when the writer applies it.
fn suspect(cache: &MessageCache) -> bool {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.is_suspect(&first_chan())
}

/// The tombstones the file holds, once the writer has caught up. Counted file-wide, so a provider
/// whose serving is suspended can be asked too.
fn tombstones(cache: &MessageCache) -> u64 {
    assert!(cache.quiesce_blocking(WAIT), "the writer caught up");
    cache.counts().served().expect("counts").tombstones
}

/// Publish one change to the read-only channel as an adapter's pushed event, the message as the
/// fake holds it (with `content` instead, when given).
fn push(d: &Deployment, event: &str, kind: LiveKind, message: &MessageId, content: Option<&str>) {
    let mut message = d.fake.message(message).expect("the fake holds it");
    if let Some(content) = content {
        message.content = content.to_owned();
    }
    assert!(d.state.live.publish_ingested(
        event.to_owned(),
        kind,
        false,
        &id(READ_CHANNEL),
        message,
    ));
}

/// Read the read-only channel's newest Main page with the write token: its forward cursor.
async fn newest_cursor(state: &AppState) -> String {
    let (status, page) = call(
        state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    page["next_after"]
        .as_str()
        .expect("a forward cursor")
        .to_owned()
}

/// The provider's deltas say from now on, completely, that `gone` was deleted; read the one since
/// `cursor` with the write token.
async fn delta_deleting(d: &Deployment, cursor: &str, gone: &MessageId) {
    d.fake.delete(gone);
    d.first.inner.set_delta(vibe_talk::threads::TimelineDelta {
        more: false,
        complete: true,
        deleted: vec![gone.clone()],
        removed_threads: Vec::new(),
    });
    let (status, page) = call(
        &d.state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=50&after={cursor}"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(page["delta"].is_object(), "{page}");
}

/// What the shadow has counted for one consumer.
fn shadowed(state: &AppState, consumer: &str) -> vibe_talk::cache::shadow::ConsumerCounts {
    state
        .message_cache
        .shadow_counts()
        .into_iter()
        .find(|counts| counts.consumer == consumer)
        .expect("a consumer the shadow counts")
}

#[tokio::test]
async fn a_deletion_reaches_the_file_before_the_account_is_checked_and_an_edit_marks_the_channel_suspect(
) {
    let dir = TempDir::new("cache-access-unchecked-changes");
    let clock = ManualClock::at(T0 + HOUR);
    filled(&dir, &clock).await.close();

    // The next start: the same account, but its provider is slow to say so.
    let d = upstream(10, "UA1", "UB1");
    d.first.hold_identity();
    let state = testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
    let d = Deployment { state, ..d };
    let cache = d.cache();
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Unverified
    );
    let ids = seeded(READ_CHANNEL);

    // A deletion from the live feed takes the message out at once, and leaves nothing in doubt.
    clock.advance(MINUTE);
    push(&d, "e1", LiveKind::Delete, &ids[4], None);
    d.fake.delete(&ids[4]);
    assert_eq!(held(&cache, &first_chan()), 9);
    assert!(!suspect(&cache));

    // An edit waits for the account, and the channel is suspect until a later read lands.
    d.fake.edit(&ids[5], "edited upstream");
    push(&d, "e2", LiveKind::Update, &ids[5], None);
    assert_eq!(text_of(&cache, &ids[5]).as_deref(), Some("a message"));
    assert!(suspect(&cache), "the edit was left out");

    // A complete delta's deletion is applied too; the rest of what reads see is not.
    let cursor = newest_cursor(&d.state).await;
    delta_deleting(&d, &cursor, &ids[6]).await;
    assert_eq!(held(&cache, &first_chan()), 8);
    assert_eq!(tombstones(&cache), 2);

    // The account answers, and matches: what was deleted stays deleted, and the channel stays
    // suspect, so the message that may have been edited is not one the cache would serve.
    d.first.release_identity();
    assert!(d.state.message_cache.identities_verified(WAIT).await);
    assert_eq!(held(&cache, &first_chan()), 8);
    assert!(suspect(&cache));
    clock.advance(MINUTE);
    let (status, _) = call(
        &d.state,
        "GET",
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{}",
            ids[5].as_str()
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(d.state.message_cache.shadow_settled(WAIT).await);
    let counted = shadowed(&d.state, "message_by_id");
    assert_eq!(
        (counted.asked, counted.would_serve, counted.content_differs),
        (1, 0, 0),
        "{counted:?}"
    );

    // A read that began after the edit was left out lands: the head is trusted, the edit is in.
    clock.advance(MINUTE);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(!suspect(&cache));
    assert_eq!(text_of(&cache, &ids[5]).as_deref(), Some("edited upstream"));
    assert_eq!(held(&cache, &first_chan()), 8);
}

#[tokio::test]
async fn a_deletion_reaches_the_file_while_the_provider_is_suspended() {
    let dir = TempDir::new("cache-access-suspended-changes");
    let clock = ManualClock::at(T0 + HOUR);
    let d = filled(&dir, &clock).await;
    let cache = d.cache();
    tokio::time::pause();
    let ids = seeded(READ_CHANNEL);

    d.first.fail("fetch_timeline", unauthorized());
    d.first.answer(Err(unauthorized()));
    assert_ne!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert_eq!(
        d.state.message_cache.verification(FIRST),
        Verification::Suspended
    );

    // The adapter, which does not use the refused credential, pushes a deletion and an edit.
    clock.advance(MINUTE);
    push(&d, "e1", LiveKind::Delete, &ids[1], None);
    d.fake.delete(&ids[1]);
    assert_eq!(total(&cache), 19, "the deletion is applied at once");
    assert_eq!(tombstones(&cache), 1);
    assert!(!suspect(&cache));
    d.fake.edit(&ids[2], "edited upstream");
    push(&d, "e2", LiveKind::Update, &ids[2], None);
    assert!(suspect(&cache), "the edit was left out");

    // The credential works again, but is not checked yet: a complete delta's deletion is applied.
    d.first.heal();
    let cursor = newest_cursor(&d.state).await;
    delta_deleting(&d, &cursor, &ids[3]).await;
    assert_eq!(total(&cache), 18);
    assert_eq!(tombstones(&cache), 2);

    // Checked again, and matching: nothing deleted comes back, and the next read takes the edit.
    d.first.answer(Ok(account("UA1")));
    tokio::time::advance(SUSPENDED_RECHECK + PAST).await;
    assert!(d.state.message_cache.identities_verified(WAIT).await);
    assert_eq!(held(&cache, &first_chan()), 8);
    assert!(suspect(&cache), "nothing has read it since");
    clock.advance(MINUTE);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(!suspect(&cache));
    assert_eq!(text_of(&cache, &ids[2]).as_deref(), Some("edited upstream"));
    assert_eq!(held(&cache, &first_chan()), 8);
}

// ---- a left-out change is remembered per message, and across a restart -------------------------

/// What the shadow has counted for `message_by_id` once a write-token read of `message` by id has
/// answered `expected` upstream.
async fn by_id(
    state: &AppState,
    message: &MessageId,
    expected: StatusCode,
) -> vibe_talk::cache::shadow::ConsumerCounts {
    let (status, _) = call(
        state,
        "GET",
        &format!(
            "/api/v1/channels/{READ_CHANNEL}/messages/{}",
            message.as_str()
        ),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, expected);
    assert!(state.message_cache.shadow_settled(WAIT).await);
    shadowed(state, "message_by_id")
}

/// The next start of a filled cache, its provider slow to say which account it is. The message is
/// deleted upstream meanwhile, and the write-scope read that would have shown it gone is left out.
/// Once the account answers, a two-row newest page re-reads only the newest messages: the deleted
/// message, still in the file, must not be one the cache would serve.
#[tokio::test]
async fn a_deletion_left_out_while_the_account_was_unchecked_is_never_vouched_for_by_a_short_page()
{
    let dir = TempDir::new("cache-access-missed-deletion");
    let clock = ManualClock::at(T0 + HOUR);
    filled(&dir, &clock).await.close();
    let ids = seeded(READ_CHANNEL);
    let d = upstream(10, "UA1", "UB1");
    d.first.hold_identity();
    let state = testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
    let d = Deployment { state, ..d };
    let cache = d.cache();

    clock.advance(MINUTE);
    d.fake.delete(&ids[3]);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(suspect(&cache), "the read was left out");
    d.first.release_identity();
    assert!(d.state.message_cache.identities_verified(WAIT).await);

    clock.advance(MINUTE);
    let (status, _) = call(
        &d.state,
        "GET",
        &format!("/api/v1/channels/{READ_CHANNEL}/timeline?view=main&limit=2"),
        WRITE_TOKEN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        text_of(&cache, &ids[3]).is_some(),
        "the short page did not reach it"
    );
    assert!(
        suspect(&cache),
        "nothing re-read the deleted message's stretch"
    );
    let counted = by_id(&d.state, &ids[3], StatusCode::NOT_FOUND).await;
    assert_eq!(
        (
            counted.would_serve,
            counted.extra_rows,
            counted.misses.suspect
        ),
        (0, 0, 1),
        "the shadow would serve a deleted message: {counted:?}"
    );

    // A read of the whole of Main that began after it was left out takes the deletion in.
    clock.advance(MINUTE);
    assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
    assert!(text_of(&cache, &ids[3]).is_none());
    assert!(!suspect(&cache));
}

/// A left-out edit of a Main message leaves the channel in doubt; a read of one of its threads,
/// which never carries that message, must not lift the doubt about it.
#[tokio::test]
async fn a_threads_listing_never_vouches_for_a_main_message_whose_edit_was_left_out() {
    let dir = TempDir::new("cache-access-thread-clears");
    let clock = ManualClock::at(T0 + HOUR);
    let ids = seeded(READ_CHANNEL);
    let root = ids[5].clone();
    let thread = format!(
        "/api/v1/channels/{READ_CHANNEL}/timeline?view=thread&thread_id={}",
        root.as_str()
    );
    let threaded = || {
        let d = upstream(10, "UA1", "UB1");
        for n in [7, 9] {
            d.fake.set_reply_to(&ids[n], &root);
        }
        d
    };
    {
        let d = threaded();
        let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
        let d = Deployment { state, ..d };
        assert_eq!(
            call(&d.state, "GET", &thread, WRITE_TOKEN).await.0,
            StatusCode::OK
        );
        assert_eq!(read_main(&d.state, READ_CHANNEL).await, StatusCode::OK);
        d.close();
    }
    let d = threaded();
    d.first.hold_identity();
    let state = testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
    let d = Deployment { state, ..d };
    let cache = d.cache();
    clock.advance(MINUTE);
    d.fake.edit(&ids[2], "edited upstream");
    push(&d, "left-out", LiveKind::Update, &ids[2], None);
    assert!(suspect(&cache), "the edit was left out");
    d.first.release_identity();
    assert!(d.state.message_cache.identities_verified(WAIT).await);

    // A write-scope read of the thread, begun after the edit was left out, lands.
    clock.advance(MINUTE);
    assert_eq!(
        call(&d.state, "GET", &thread, WRITE_TOKEN).await.0,
        StatusCode::OK
    );
    assert_eq!(text_of(&cache, &ids[2]).as_deref(), Some("a message"));
    assert!(
        suspect(&cache),
        "a thread's listing lifted the doubt about a Main message"
    );
    let counted = by_id(&d.state, &ids[2], StatusCode::OK).await;
    assert_eq!(
        (
            counted.would_serve,
            counted.content_differs,
            counted.misses.suspect
        ),
        (0, 0, 1),
        "{counted:?}"
    );
}

/// What a left-out edit leaves in doubt is written to the file: a restart, whose provider answers
/// at once, must still not vouch for the message whose edit the file lacks.
#[tokio::test]
async fn a_left_out_edit_is_still_distrusted_after_a_restart() {
    let dir = TempDir::new("cache-access-restart-forgets");
    let clock = ManualClock::at(T0 + HOUR);
    filled(&dir, &clock).await.close();
    let ids = seeded(READ_CHANNEL);
    {
        let d = upstream(10, "UA1", "UB1");
        d.first.hold_identity();
        let state =
            testing::with_message_cache_unverified(d.state.clone(), dir.path(), clock.clone());
        let d = Deployment { state, ..d };
        let cache = d.cache();
        clock.advance(MINUTE);
        d.fake.edit(&ids[2], "edited upstream");
        push(&d, "left-out", LiveKind::Update, &ids[2], None);
        assert!(suspect(&cache), "the edit was left out");
        drop(cache);
        d.first.release_identity();
        assert!(d.state.message_cache.identities_verified(WAIT).await);
        d.close();
    }

    // The next start: the account answers at once, and nothing has read the channel since.
    let d = upstream(10, "UA1", "UB1");
    d.fake.edit(&ids[2], "edited upstream");
    let state = testing::with_message_cache(d.state.clone(), dir.path(), clock.clone()).await;
    let d = Deployment { state, ..d };
    let cache = d.cache();
    assert_eq!(text_of(&cache, &ids[2]).as_deref(), Some("a message"));
    assert!(suspect(&cache), "the restart forgot what was left out");
    clock.advance(MINUTE);
    let counted = by_id(&d.state, &ids[2], StatusCode::OK).await;
    assert_eq!(
        (
            counted.would_serve,
            counted.content_differs,
            counted.misses.suspect
        ),
        (0, 0, 1),
        "{counted:?}"
    );
}
