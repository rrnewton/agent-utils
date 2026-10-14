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
use vibe_talk::cache::sink::{Verification, VERIFY_RETRY_FIRST};
use vibe_talk::cache::{ChanKey, MessageCache, Scope as CacheScope};
use vibe_talk::chat::{ChatClient, ChatError, ChatIdentity, RecentRead, UpstreamClearOutcome};
use vibe_talk::discord::fake::FakeDiscord;
use vibe_talk::model::{ChannelId, Message, MessageId};
use vibe_talk::providers::{ChatRouter, ProviderEntry};
use vibe_talk::state::AppState;
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
}

impl Scripted {
    fn new(fake: &Arc<FakeDiscord>, user: &str) -> Arc<Self> {
        Arc::new(Self {
            inner: TimelineChat::new(fake.clone()),
            identity: Mutex::new(Ok(account(user))),
            gate: tokio::sync::watch::Sender::new(true),
            rules: Mutex::new(Vec::new()),
            identity_calls: AtomicUsize::new(0),
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
        self.check("probe_channel", None)?;
        self.inner.fetch_recent(channel, limit).await
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &TimelineRequest,
    ) -> Result<TimelinePage, ChatError> {
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

/// The upstream half of a [`Deployment`]: the providers say they are `first_user` and
/// `second_user`. Nothing is behind the sink yet.
fn upstream(count: i64, first_user: &str, second_user: &str) -> Deployment {
    let (mut state, fake, _store) = testing::state_with_store_from_toml(&two_providers_toml());
    for channel in [READ_CHANNEL, WRITE_CHANNEL] {
        for n in 0..count {
            fake.seed_at(
                &id(channel),
                "Ada",
                &format!("message {n}"),
                T0 + n * MINUTE,
            );
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
    let (mut state, _discord, _store) = testing::state_with_store_from_toml(&text);
    let router = ChatRouter::from_config(&state.config).expect("providers build");
    state.replace_providers(Arc::new(router));
    testing::with_message_cache(state, dir.path(), clock.clone()).await
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
