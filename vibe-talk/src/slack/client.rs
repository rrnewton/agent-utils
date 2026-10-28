//! The Slack Web API client: transport, error mapping, author names, and message mapping.
//!
//! Reads, timelines and posts are built on top of this in [`super::history`] and
//! [`super::timeline`]; this file owns the one function that talks to the network, so the token,
//! the rate-limit budget and the translation of Slack's `{ok:false}` answers live in one place.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::StreamExt as _;
use serde_json::Value;

use super::ids::{self, ChannelRef};
use super::SlackConfig;
use crate::chat::{
    ChatClient, ChatError, ChatIdentity, RateLimitExhausted, RegisteredChannel, SourceClaim,
};
use crate::config::Secret;
use crate::model::{ChannelId, Message, MessageId, UserId};
use crate::threads::MessageThread;
use crate::upstream_profile::{self, ProviderProfile, Series};

/// Slack's own ceiling on `chat.postMessage` text: "Slack will truncate messages containing more
/// than 40,000 characters", <https://docs.slack.dev/reference/methods/chat.postMessage>, read
/// 2026-10-12. Longer text would be cut short upstream, so it is split or refused here instead.
pub const SLACK_MAX_TEXT_CHARS: usize = 40_000;
/// The longest one request may spend waiting out rate limits, across all its attempts.
pub const RATE_LIMIT_BUDGET: Duration = Duration::from_secs(30);
/// How many times one logical request may be sent before a rate limit is declared unclearable.
const MAX_RATE_LIMIT_ATTEMPTS: u32 = 5;
/// The wait assumed when a rate-limited answer carries no readable `Retry-After`.
const DEFAULT_RETRY_AFTER: Duration = Duration::from_secs(1);
/// The floor on one retry wait, so "retry now" is never spent in the same millisecond.
const MIN_RETRY_WAIT: Duration = Duration::from_millis(50);

/// The methods background crawling reads: a rate limit on either holds it off. `#602
/// history-settings-and-limits`.
const CRAWL_METHODS: [&str; 2] = ["conversations.history", "conversations.replies"];

/// An `X-RateLimit-Remaining` at or below which crawling holds off until the reset.
const LOW_REMAINING: u64 = 2;

/// What Slack has said lately of one method's rate limit. `#602 history-settings-and-limits`.
#[derive(Debug, Default)]
struct MethodPressure {
    /// When it answered a rate limit, inside [`crate::chat::PRESSURE_WINDOW`], oldest first.
    limited: std::collections::VecDeque<tokio::time::Instant>,
    /// The end of the latest `Retry-After`.
    retry_until: Option<tokio::time::Instant>,
    /// The remaining quota last reported, and until when a low one holds crawling.
    remaining: Option<u64>,
    low_until: Option<tokio::time::Instant>,
}

impl MethodPressure {
    fn prune(&mut self, now: tokio::time::Instant) {
        while self
            .limited
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= crate::chat::PRESSURE_WINDOW)
        {
            self.limited.pop_front();
        }
    }

    /// Until when crawling holds off this method: the latest `Retry-After`, a quiet spell after
    /// the latest rate limit, and a low reported quota's reset, whichever is latest.
    fn hold_until(&mut self, now: tokio::time::Instant) -> Option<tokio::time::Instant> {
        self.prune(now);
        let quiet = self
            .limited
            .back()
            .map(|at| *at + crate::chat::CRAWL_QUIET_AFTER_LIMIT);
        [self.retry_until, quiet, self.low_until]
            .into_iter()
            .flatten()
            .filter(|until| *until > now)
            .max()
    }
}
/// The largest `Retry-After` believed, in seconds; anything larger blows the budget anyway.
const MAX_PARSED_RETRY_AFTER_SECONDS: f64 = 3600.0;
/// Most authors whose `users.info` answer is remembered.
const USER_CACHE_MAX: usize = 2000;
/// How long a resolved author name is trusted.
const USER_CACHE_TTL: Duration = Duration::from_secs(3600);
/// How long a failed lookup is remembered, so a missing scope does not cost a call per message.
const USER_FAILURE_TTL: Duration = Duration::from_secs(300);
/// Most conversations remembered as having threads.
const THREADED_MAX: usize = 1000;

/// Slackbot's user id, the same in every workspace. `users.info` reports it with `is_bot: false`.
const SLACKBOT_USER_ID: &str = "USLACKBOT";

/// Message subtypes that are somebody's words. Every other subtype is a system event (a join, a
/// topic change, a rename, …) and is omitted from reads.
const KEPT_SUBTYPES: [&str; 4] = [
    "bot_message",
    "thread_broadcast",
    "file_share",
    "me_message",
];

/// Slack `error` strings that mean the credential itself was refused.
const UNAUTHORIZED: [&str; 5] = [
    "not_authed",
    "invalid_auth",
    "account_inactive",
    "token_revoked",
    "token_expired",
];
/// Slack `error` strings that mean the credential is good but may not do this.
const FORBIDDEN: [&str; 8] = [
    "missing_scope",
    "not_in_channel",
    "access_denied",
    "restricted_action",
    "is_archived",
    "no_permission",
    "not_allowed_token_type",
    "team_access_not_granted",
];
/// Slack `error` strings that mean the named thing does not exist for this credential.
const NOT_FOUND: [&str; 4] = [
    "channel_not_found",
    "thread_not_found",
    "message_not_found",
    "user_not_found",
];

/// Whether a request may wait out a rate limit or should give up at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Patience {
    /// Wait within [`RATE_LIMIT_BUDGET`]: the caller needs this answer.
    Budgeted,
    /// Give up at the first rate limit: the answer is cosmetic, such as an author's name.
    BestEffort,
}

/// One answer from the transport.
enum Outcome {
    Done(Value),
    Limited(Duration),
}

/// What is remembered about one author.
#[derive(Clone, Debug)]
struct CachedUser {
    name: Option<String>,
    is_bot: bool,
    at: Instant,
}

/// What is remembered of each group DM's members, by conversation id. `#317 slack-group-dm-names`.
pub(super) type GroupMembers = Mutex<HashMap<String, (Option<Vec<String>>, Instant)>>;

/// A live Slack client.
#[derive(Debug)]
pub struct HttpSlackClient {
    pub(super) provider_name: String,
    api_base: String,
    token: Secret,
    owner_user_id: Option<String>,
    client: reqwest::Client,
    pub(super) channel_registration: bool,
    pub(super) registered_writable: bool,
    pub(super) channel_discovery: bool,
    /// Whether the server may move this account's Slack read cursor; see `super::clear`.
    pub(super) upstream_clear: bool,
    /// The account this client posts as, learned from `auth.test` or from its first post.
    self_user_id: Mutex<Option<String>>,
    /// The workspace's own origin (`https://acme.slack.com`), as configured or as `auth.test`
    /// reported it, where a message's permalink lives. `#408 copy-upstream-link`.
    workspace_url: Mutex<Option<String>>,
    /// The workspace's team id as `auth.test` reported it, where a message's address in
    /// Slack's own app names it. `#481 slack-app-deep-link`.
    team_id: Mutex<Option<String>>,
    users: Mutex<HashMap<String, CachedUser>>,
    /// Each group DM's member ids, or `None` where Slack would not list them, and when that was
    /// learned, so the channel browser names it by its members without asking again. `#317
    /// slack-group-dm-names`; see `super::groups`.
    pub(super) group_members: GroupMembers,
    /// The names of channels messages mention without naming them. `#318 slack-mention-names`;
    /// see `super::mentions`.
    pub(super) channel_names: super::mentions::ChannelNames,
    /// Conversations in which a thread root has been seen, so `has_threads` does not flicker
    /// between pages that happen to contain none.
    threaded: Mutex<HashSet<String>>,
    /// Stretches after a cursor proven to hold no real messages, so a walk that ran out of
    /// requests while crossing system events resumes rather than restarts. See `super::history`.
    pub(super) proven: Mutex<HashMap<(String, i64), (i64, Instant)>>,
    /// The replies each thread's last Flat read brought, by conversation and root `ts`, and what
    /// the root said then, so the next Flat read reads only the threads that moved; and when a
    /// read of each thread, Flat or the thread view, last finished. `#282 slack-flat-skip`; see
    /// `super::timeline`.
    pub(super) flat_memory: Mutex<super::timeline::FlatMemory>,
    /// How long those replies are trusted before a Flat read reads their thread in full again.
    pub(super) flat_reconcile: Duration,
    /// Where the live poller stands in each thread of each conversation it polls, so each of its
    /// reads reads only the threads that moved, and only after the newest reply it has had.
    /// `#307 slack-reply-live`; see `super::live`.
    pub(super) live_threads: Mutex<HashMap<String, super::live::LiveThreads>>,
    /// The most one post may carry, as Slack is sent it; see [`SlackConfig::post_limit`].
    pub(super) post_limit: crate::chat::PostLimit,
    /// Where each Web API request is timed, by method and outcome, when the client was built with
    /// one. `#208 persistent-cache`; see [`crate::upstream_profile`].
    shim_profile: Option<Arc<ProviderProfile>>,
    /// The `users.conversations` pages the channel browser read, by cursor, so searches typed one
    /// after another walk the same listing without asking Slack again. `#371 channel-search-hash`.
    pub(super) directory_pages: crate::directory::Kept<String, Value>,
    /// What Slack has said lately of each method's rate limit, whichever request it answered:
    /// what background crawling holds off for ([`ChatClient::crawl_hold`]) and the diagnostics
    /// report. `#602 history-settings-and-limits`.
    pressure: Mutex<HashMap<String, MethodPressure>>,
    /// The threads the first parts of long posts just went into or started, by conversation and
    /// root `ts`, and when: proven to be there by Slack's own answer to that part, so the rest of
    /// the post goes in without reading the thread first. `#448 send-dedupe`; see
    /// `super::timeline::ROOTS_KEEP`.
    pub(super) posted_roots: Mutex<HashMap<(String, String), Instant>>,
    /// Whether Slack, as this client reaches it, has handed back an operation key on a message:
    /// the proof that the transport passes `metadata` through both ways. Until then reads are not
    /// promised to return one. `#465 replication-c6-op-key-marker`.
    op_keys_returned: std::sync::atomic::AtomicBool,
}

impl HttpSlackClient {
    /// Build a client from one Slack provider's configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ChatError::Transport`] when the HTTP client cannot be built, and
    /// [`ChatError::Refused`] when the API base is not an `http(s)` URL.
    pub fn new(config: &SlackConfig) -> Result<Self, ChatError> {
        let api_base = config.api_base.trim().trim_end_matches('/').to_owned();
        let parsed = reqwest::Url::parse(&api_base).map_err(|_| {
            ChatError::Refused("the Slack API base is not a URL".to_owned())
                .with_provider(&config.provider_name)
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(
                ChatError::Refused("the Slack API base must be an http(s) URL".to_owned())
                    .with_provider(&config.provider_name),
            );
        }
        let client = crate::egress::client_builder()
            .user_agent(concat!(
                "vibe-talk (https://github.com/rrnewton/agent-utils, ",
                env!("CARGO_PKG_VERSION"),
                ")"
            ))
            .timeout(Duration::from_secs(config.request_timeout_seconds.max(1)))
            .build()
            .map_err(|e| {
                ChatError::Transport(e.to_string()).with_provider(&config.provider_name)
            })?;
        Ok(Self {
            provider_name: config.provider_name.clone(),
            api_base,
            token: config.token.clone(),
            owner_user_id: config
                .owner_user_id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_owned),
            client,
            channel_registration: config.channel_registration,
            registered_writable: config.registered_channels_writable,
            channel_discovery: config.channel_discovery,
            upstream_clear: config.upstream_clear,
            self_user_id: Mutex::new(None),
            workspace_url: Mutex::new(
                config
                    .workspace_url
                    .as_deref()
                    .and_then(crate::upstream_url::workspace_origin),
            ),
            team_id: Mutex::new(None),
            users: Mutex::new(HashMap::new()),
            group_members: GroupMembers::default(),
            channel_names: super::mentions::ChannelNames::default(),
            threaded: Mutex::new(HashSet::new()),
            proven: Mutex::new(HashMap::new()),
            flat_memory: Mutex::default(),
            flat_reconcile: super::timeline::FLAT_RECONCILE,
            live_threads: Mutex::new(HashMap::new()),
            post_limit: config.post_limit(),
            shim_profile: None,
            directory_pages: crate::directory::Kept::default(),
            pressure: Mutex::new(HashMap::new()),
            posted_roots: Mutex::new(HashMap::new()),
            op_keys_returned: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Time every Web API request this client sends into `profile`, by method and outcome.
    /// `#208 persistent-cache`.
    ///
    /// Each request counts once, including one answered with a rate limit and sent again, which
    /// is how the far end's own log counts them. A request the caller stopped waiting for counts
    /// as abandoned.
    #[must_use]
    pub fn with_shim_profile(mut self, profile: Arc<ProviderProfile>) -> Self {
        self.shim_profile = Some(profile);
        self
    }

    /// How long the replies a Flat read remembers for a thread are trusted before a Flat read
    /// reads that thread in full again: fifteen minutes unless set, the page's own reconcile
    /// period. `#282 slack-flat-skip`.
    ///
    /// Until then a thread whose root reports the same reply count and latest reply costs no call,
    /// and one with new replies costs one. An edit to a reply changes nothing on its root, so it
    /// reaches Flat when this period runs out, or on the next Flat read after the thread view has
    /// shown it (the thread view shows it at once). That holds for every reader of Flat, not only
    /// the page: a summary, or the newest messages read for `/messages` and the voice tools, which
    /// take their replies from the same memory (`#307 slack-reply-live`), may also see a reply's
    /// text as it was up to this long ago. `Duration::ZERO` reads every thread in full on every
    /// Flat read.
    #[must_use]
    pub fn with_flat_reconcile(mut self, period: Duration) -> Self {
        self.flat_reconcile = period;
        self
    }

    /// How long the channel browser keeps Slack's listing of the app's conversations: two minutes
    /// unless set ([`crate::directory::DIRECTORY_KEEP`]), and forgotten whenever the list of
    /// channels here changes. `Duration::ZERO` reads it again for every search.
    /// `#371 channel-search-hash`.
    #[must_use]
    pub fn with_directory_keep(mut self, keep: Duration) -> Self {
        self.directory_pages = crate::directory::Kept::new(keep);
        self
    }

    /// Call one Web API method, waiting out rate limits within [`RATE_LIMIT_BUDGET`]; or, for the
    /// message cache's backfill, giving up at the first one: a crawler that waits a `Retry-After`
    /// and sends again keeps a route full that a person's read is waiting on, and the backfill
    /// holds off the provider itself once told (`#602 history-settings-and-limits`).
    pub(super) async fn call(
        &self,
        method: &'static str,
        params: &[(&str, String)],
    ) -> Result<Value, ChatError> {
        let patience = if crate::read_reason::ReadReason::current()
            == crate::read_reason::ReadReason::Backfill
        {
            Patience::BestEffort
        } else {
            Patience::Budgeted
        };
        self.call_with(method, params, patience).await
    }

    pub(super) async fn call_with(
        &self,
        method: &'static str,
        params: &[(&str, String)],
        patience: Patience,
    ) -> Result<Value, ChatError> {
        let url = format!("{}/{method}", self.api_base);
        let mut waited = Duration::ZERO;
        let mut attempts = 0;
        loop {
            attempts += 1;
            let timer = self
                .shim_profile
                .as_deref()
                .map(|profile| profile.start(Series::ShimCall, method));
            let answer = self.attempt(&url, method, params).await;
            if let Some(timer) = timer {
                timer.finish(
                    match &answer {
                        Ok(Outcome::Done(_)) => upstream_profile::Outcome::Ok,
                        Ok(Outcome::Limited(_)) => upstream_profile::Outcome::RateLimited,
                        Err(error) => upstream_profile::Outcome::of(error),
                    },
                    0,
                );
            }
            match answer? {
                Outcome::Done(value) => return Ok(value),
                Outcome::Limited(retry_after) => {
                    self.note_limited(method, retry_after);
                    let wait = retry_after.max(MIN_RETRY_WAIT);
                    // A wait longer than what is left is refused BEFORE it is taken: parking a
                    // caller and then failing anyway is the worst of both.
                    if patience == Patience::BestEffort
                        || attempts >= MAX_RATE_LIMIT_ATTEMPTS
                        || waited + wait > RATE_LIMIT_BUDGET
                    {
                        return Err(ChatError::RateLimited(RateLimitExhausted {
                            provider: "slack",
                            route: method.to_owned(),
                            attempts,
                            waited,
                            budget: RATE_LIMIT_BUDGET,
                            retry_after,
                            global: false,
                        }));
                    }
                    tokio::time::sleep(wait).await;
                    waited += wait;
                }
            }
        }
    }

    fn pressure(&self) -> std::sync::MutexGuard<'_, HashMap<String, MethodPressure>> {
        self.pressure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Remember that Slack rate-limited `method`, asking for `retry_after`.
    fn note_limited(&self, method: &str, retry_after: Duration) {
        let now = tokio::time::Instant::now();
        let mut pressure = self.pressure();
        let entry = pressure.entry(method.to_owned()).or_default();
        entry.prune(now);
        entry.limited.push_back(now);
        let until = now + retry_after.max(MIN_RETRY_WAIT);
        if entry.retry_until.is_none_or(|at| until > at) {
            entry.retry_until = Some(until);
        }
    }

    /// Remember the quota a response reported for `method`, when Slack (or a bridge in front of
    /// it) sends `X-RateLimit-Remaining`: a low one holds crawling until its reset.
    fn note_quota(&self, method: &str, headers: &reqwest::header::HeaderMap) {
        let number = |name: &str| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
        };
        let Some(remaining) = number("x-ratelimit-remaining") else {
            return;
        };
        let now = tokio::time::Instant::now();
        let mut pressure = self.pressure();
        let entry = pressure.entry(method.to_owned()).or_default();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let left = remaining as u64;
        entry.remaining = Some(left);
        if left <= LOW_REMAINING {
            let reset = number("x-ratelimit-reset-after")
                .map(|seconds| Duration::from_secs_f64(seconds.min(MAX_PARSED_RETRY_AFTER_SECONDS)))
                .unwrap_or(crate::chat::CRAWL_QUIET_AFTER_LIMIT);
            entry.low_until = Some(now + reset);
        } else {
            entry.low_until = None;
        }
    }

    async fn attempt(
        &self,
        url: &str,
        method: &str,
        params: &[(&str, String)],
    ) -> Result<Outcome, ChatError> {
        // One request to the far end, counted against the operation sending it.
        upstream_profile::count_call();
        let response = self
            .client
            .post(url)
            .bearer_auth(self.token.expose())
            .form(params)
            .send()
            .await
            .map_err(|e| ChatError::Transport(self.redact(&e.to_string())))?;
        let status = response.status();
        self.note_quota(method, response.headers());
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .map(|seconds| Duration::from_secs_f64(seconds.min(MAX_PARSED_RETRY_AFTER_SECONDS)));
        let text = response
            .text()
            .await
            .map_err(|e| ChatError::Transport(self.redact(&e.to_string())))?;
        if status.as_u16() == 429 {
            return Ok(Outcome::Limited(retry_after.unwrap_or(DEFAULT_RETRY_AFTER)));
        }
        if !status.is_success() {
            // Truncated: an intermediary's error page is long, and this string ends up in a log.
            return Err(ChatError::Status {
                status: status.as_u16(),
                body: self.redact(&text).chars().take(500).collect(),
            });
        }
        let value: Value = serde_json::from_str(&text).map_err(|_| {
            ChatError::Shape(format!("{method} answered with something that is not JSON"))
        })?;
        match value.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(Outcome::Done(value)),
            Some(false) => {
                let error = value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown_error");
                if error == "ratelimited" {
                    return Ok(Outcome::Limited(retry_after.unwrap_or(DEFAULT_RETRY_AFTER)));
                }
                Err(self.slack_error(method, error, &value))
            }
            None => Err(ChatError::Shape(format!(
                "{method} answer has no boolean \"ok\" field"
            ))),
        }
    }

    /// Translate a Slack `{ok:false, error}` into the status the probe and the API classify.
    fn slack_error(&self, method: &str, error: &str, value: &Value) -> ChatError {
        let status = if UNAUTHORIZED.contains(&error) {
            401
        } else if FORBIDDEN.contains(&error) {
            403
        } else if NOT_FOUND.contains(&error) {
            404
        } else {
            400
        };
        // Not JSON on purpose: the probe reads a numeric `code` out of a JSON body as a Discord
        // error code, and Slack's answer must be classified by its status alone.
        let mut body = format!("{method} answered {error}");
        if let Some(needed) = value.get("needed").and_then(Value::as_str) {
            body.push_str(&format!(" (needed scope: {needed})"));
        }
        ChatError::Status {
            status,
            body: self.redact(&body).chars().take(500).collect(),
        }
    }

    /// The Slack `error` code inside a failure `slack_error` made for `method`,
    /// and the scope Slack said was needed, when it said one.
    ///
    /// `None` for any other failure: a transport fault, a rate limit, or another method's answer.
    pub(super) fn slack_error_code<'e>(
        error: &'e ChatError,
        method: &str,
    ) -> Option<(&'e str, Option<&'e str>)> {
        let ChatError::Status { body, .. } = error.cause() else {
            return None;
        };
        let rest = body.strip_prefix(method)?.strip_prefix(" answered ")?;
        let end = rest
            .find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'))
            .unwrap_or(rest.len());
        let (code, tail) = rest.split_at(end);
        if code.is_empty() {
            return None;
        }
        let needed = tail
            .strip_prefix(" (needed scope: ")
            .and_then(|scope| scope.strip_suffix(')'))
            .filter(|scope| !scope.is_empty());
        Some((code, needed))
    }

    fn redact(&self, text: &str) -> String {
        let secret = self.token.expose();
        if secret.is_empty() {
            text.to_owned()
        } else {
            text.replace(secret, "<redacted>")
        }
    }

    /// Remember the account this client posts as.
    pub(super) fn learn_self(&self, id: &str) {
        let mut slot = self
            .self_user_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_none() && !id.is_empty() {
            *slot = Some(id.to_owned());
        }
    }

    /// The owner's own Slack user id, when configured.
    pub(super) fn owner_id(&self) -> Option<String> {
        self.owner_user_id.clone()
    }

    /// The account this client posts as, once known.
    pub(super) fn self_id(&self) -> Option<String> {
        self.self_user_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Note that `conversation` has threads.
    pub(super) fn mark_threaded(&self, conversation: &str) {
        let mut threaded = self
            .threaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if threaded.len() >= THREADED_MAX {
            threaded.clear();
        }
        threaded.insert(conversation.to_owned());
    }

    /// Whether a thread root has been seen in `conversation`.
    pub(super) fn is_threaded(&self, conversation: &str) -> bool {
        self.threaded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(conversation)
    }

    /// Whether `id`'s name is in the cache, found or not, and still trusted: looking it up costs no
    /// call. `#318 slack-mention-names`.
    pub(super) fn knows_user(&self, id: &str) -> bool {
        self.users
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .is_some_and(|cached| {
                let ttl = if cached.name.is_some() {
                    USER_CACHE_TTL
                } else {
                    USER_FAILURE_TTL
                };
                cached.at.elapsed() < ttl
            })
    }

    /// An author's display name and bot flag from `users.info`, cached; `None` when unknown.
    ///
    /// Never fails a read: a lookup that fails, including on a rate limit, degrades to the raw id
    /// and is remembered briefly so a missing `users:read` scope does not cost a call per message.
    pub(super) async fn user(&self, id: &str) -> Option<(String, bool)> {
        {
            let users = self
                .users
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cached) = users.get(id) {
                let ttl = if cached.name.is_some() {
                    USER_CACHE_TTL
                } else {
                    USER_FAILURE_TTL
                };
                if cached.at.elapsed() < ttl {
                    return cached.name.clone().map(|name| (name, cached.is_bot));
                }
            }
        }
        let answer = self
            .call_with(
                "users.info",
                &[("user", id.to_owned())],
                Patience::BestEffort,
            )
            .await;
        let (name, is_bot) = match &answer {
            Ok(value) => {
                let user = value.get("user");
                let profile = user.and_then(|user| user.get("profile"));
                let text = |object: Option<&Value>, key: &str| {
                    object
                        .and_then(|object| object.get(key))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                let name = text(profile, "display_name")
                    .or_else(|| text(profile, "real_name"))
                    .or_else(|| text(user, "real_name"))
                    .or_else(|| text(user, "name"));
                let is_bot = user
                    .and_then(|user| user.get("is_bot"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                (name, is_bot)
            }
            Err(error) => {
                tracing::debug!(user = id, %error, "Slack author lookup failed; showing the raw id");
                (None, false)
            }
        };
        let mut users = self
            .users
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if users.len() >= USER_CACHE_MAX {
            users.retain(|_, cached| cached.at.elapsed() < USER_CACHE_TTL);
            while users.len() >= USER_CACHE_MAX {
                let Some(oldest) = users
                    .iter()
                    .min_by_key(|(_, cached)| cached.at)
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                users.remove(&oldest);
            }
        }
        users.insert(
            id.to_owned(),
            CachedUser {
                name: name.clone(),
                is_bot,
                at: Instant::now(),
            },
        );
        name.map(|name| (name, is_bot))
    }

    /// Convert raw Slack messages into [`Message`]s for `channel`, resolving author names.
    ///
    /// Order is preserved. A message with no author of any kind is omitted, with a warning. Every
    /// read of a message passes through here, so each carries the reactions Slack reported on it,
    /// marked where the owner is among those who reacted (`#312 slack-reactions`), when it was
    /// last edited (`#320 slack-edited-mark`), and who each of its mentions names
    /// (`#318 slack-mention-names`): the people are looked up with the authors, at most
    /// [`super::mentions::MENTION_LOOKUPS_MAX`] of them not already known, and the channels a
    /// mention gives no name are named from `conversations.info`.
    pub(super) async fn messages(&self, channel: &ChannelId, raws: &[Raw]) -> Vec<Message> {
        let mut wanted: Vec<&str> = raws.iter().filter_map(Raw::lookup_needed).collect();
        wanted.sort_unstable();
        wanted.dedup();
        let mentioned = self.mentions_to_look_up(raws, &wanted);
        wanted.extend(mentioned);
        let lookups: Vec<_> = wanted.iter().map(|id| self.user(id)).collect();
        let found: Vec<Option<(String, bool)>> = futures_util::stream::iter(lookups)
            .buffered(4)
            .collect()
            .await;
        let resolved: HashMap<String, (String, bool)> = wanted
            .iter()
            .zip(found)
            .filter_map(|(id, found)| found.map(|found| ((*id).to_owned(), found)))
            .collect();
        let channels = self.mentioned_channels(raws).await;
        let messages: Vec<Message> = raws
            .iter()
            .filter_map(|raw| {
                let message =
                    raw.to_message(channel, &resolved, &channels, self.owner_user_id.as_deref());
                if message.is_none() {
                    tracing::warn!(ts = %raw.ts, "omitting a Slack message with no author");
                }
                message
            })
            .collect();
        if messages.iter().any(|message| message.op_key.is_some()) {
            self.op_keys_returned
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        messages
    }

    /// Resolve a vibe-talk channel id into its Slack conversation and optional thread.
    pub(super) fn channel_ref(channel: &ChannelId) -> Result<ChannelRef, ChatError> {
        ChannelRef::parse(channel.as_str()).ok_or_else(|| {
            ChatError::Refused(format!(
                "{channel} is not a Slack channel id: expected a conversation id such as C0123ABCD, \
                 or one narrowed to a thread such as C0123ABCD~1700000000.000100"
            ))
        })
    }
}

/// One Slack message object with its parsed `ts`.
#[derive(Clone, Debug)]
pub(super) struct Raw {
    /// Microseconds since the Unix epoch.
    pub micros: i64,
    /// Canonical `ts`.
    pub ts: String,
    /// The message object as Slack sent it.
    pub value: Value,
}

impl Raw {
    /// Wrap one message object, refusing one without a usable `ts`.
    pub fn parse(value: Value) -> Result<Self, ChatError> {
        let micros = value
            .get("ts")
            .and_then(Value::as_str)
            .and_then(ids::ts_micros)
            .ok_or_else(|| ChatError::Shape("a message has no valid \"ts\"".to_owned()))?;
        Ok(Self {
            micros,
            ts: ids::micros_ts(micros),
            value,
        })
    }

    fn str_field(&self, key: &str) -> Option<&str> {
        self.value
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
    }

    /// Whether this is somebody's words rather than a system event, and can be given an id.
    pub fn is_real(&self) -> bool {
        self.str_field("subtype")
            .is_none_or(|subtype| KEPT_SUBTYPES.contains(&subtype))
            && ids::micros_message_id(self.micros).is_some()
    }

    /// The canonical root `ts` of the thread this message belongs to, when it belongs to one.
    pub fn thread_ts(&self) -> Option<String> {
        self.str_field("thread_ts").and_then(ids::canonical_ts)
    }

    /// Whether this is a thread reply also sent to the conversation (`thread_broadcast`), which
    /// history lists among the conversation's own messages.
    pub fn is_broadcast(&self) -> bool {
        self.str_field("subtype") == Some("thread_broadcast")
    }

    /// Whether this message is the root of a thread with at least one reply.
    pub fn is_thread_root(&self) -> bool {
        self.thread_ts().as_deref() == Some(self.ts.as_str()) && self.reply_count() > 0
    }

    /// Replies Slack reports for this root; zero when absent.
    pub fn reply_count(&self) -> u64 {
        self.value
            .get("reply_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    }

    /// The latest reply's instant, for a root; the message's own otherwise.
    pub fn activity_micros(&self) -> i64 {
        self.latest_reply_micros().unwrap_or(self.micros)
    }

    /// The instant of the latest reply this root names, when it names one (`latest_reply`).
    pub fn latest_reply_micros(&self) -> Option<i64> {
        self.str_field("latest_reply").and_then(ids::ts_micros)
    }

    /// When this message was last edited, from Slack's `edited: {user, ts}`, when it was and the
    /// `ts` reads as one. `#320 slack-edited-mark`.
    pub fn edited_micros(&self) -> Option<i64> {
        self.value
            .get("edited")
            .and_then(|edited| edited.get("ts"))
            .and_then(Value::as_str)
            .and_then(ids::ts_micros)
    }

    fn author_id(&self) -> Option<&str> {
        self.str_field("user")
            .or_else(|| self.str_field("bot_id"))
            .or_else(|| self.str_field("app_id"))
    }

    fn own_name(&self) -> Option<String> {
        let nested = |object: &str, key: &str| {
            self.value
                .get(object)
                .and_then(|object| object.get(key))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        nested("user_profile", "display_name")
            .or_else(|| nested("user_profile", "real_name"))
            .or_else(|| nested("bot_profile", "name"))
            .or_else(|| {
                self.str_field("username")
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            })
    }

    /// The user id whose name must be looked up, when the message does not carry one.
    fn lookup_needed(&self) -> Option<&str> {
        if self.own_name().is_some() {
            return None;
        }
        self.str_field("user")
    }

    /// Map onto the server's [`Message`] for `channel`, for an owner whose Slack id is `owner`.
    ///
    /// `resolved` holds the names looked up by user id, for authors and mentions alike, and
    /// `channels` the names of channels mentioned without one, by channel id.
    pub fn to_message(
        &self,
        channel: &ChannelId,
        resolved: &HashMap<String, (String, bool)>,
        channels: &HashMap<String, String>,
        owner: Option<&str>,
    ) -> Option<Message> {
        let text = self.str_field("text").unwrap_or_default();
        let id = ids::micros_message_id(self.micros)?;
        let author_id = self.author_id()?.to_owned();
        let looked_up = resolved.get(&author_id);
        let author = self
            .own_name()
            .or_else(|| looked_up.map(|(name, _)| name.clone()))
            .unwrap_or_else(|| author_id.clone());
        // Slackbot is Slack's own system bot, though `users.info` says `is_bot: false` of it; it
        // is drawn as the bot it is. `#323 slack-system-bots`.
        let author_is_bot = self.str_field("bot_id").is_some()
            || self.str_field("subtype") == Some("bot_message")
            || author_id == SLACKBOT_USER_ID
            || looked_up.is_some_and(|(_, is_bot)| *is_bot);
        // A message with no `thread_ts` is on the conversation's own timeline, and Slack lets any
        // such message head a thread: posting with `thread_ts` = its `ts` starts one. So it is
        // named as the root of its own thread with no replies, as a message nobody has answered on
        // Google Chat is, and the page can offer to start that thread (`#252 thread-button-state`).
        // Slack only writes `thread_ts` and `reply_count` once there is a reply, which makes the
        // count of nothing exact.
        let thread_ts = self.thread_ts().unwrap_or_else(|| self.ts.clone());
        let is_root = thread_ts == self.ts;
        let reply_count = is_root.then(|| self.reply_count());
        let thread = Some(MessageThread {
            hue: None,
            display_name: None,
            summary: None,
            root_message_id: ids::message_id_from_ts(&thread_ts),
            id: thread_ts,
            is_root,
            reply_count,
            reply_count_exact: reply_count.is_some(),
        });
        Some(Message {
            thread,
            id,
            channel_id: channel.clone(),
            author,
            author_id: UserId(author_id),
            author_is_bot,
            timestamp: rfc3339(self.micros),
            // Left EMPTY: this layer holds no configuration, and `crate::ops` is the one filler.
            spoken_time: String::new(),
            // Slack's `edited.ts`, on an edited message only. `#320 slack-edited-mark`.
            edited_timestamp: self.edited_micros().map(rfc3339),
            // Slack has no reply pointer; a reply is thread membership, carried in `thread`.
            reply_to: None,
            // The words, then a line for each file the message shares: a post that is only a file
            // has no words at all (`#313 slack-file-posts`, `super::files`).
            content: super::files::body(
                super::mrkdwn::to_markdown_with(text, channels),
                self.value.get("files"),
            ),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            // Absent stays absent: Slack omits the field on a message nobody reacted to, and a
            // bridge that cannot supply reactions omits it on every message. `#312
            // slack-reactions`; see `super::reactions`.
            reactions: super::reactions::parse(self.value.get("reactions"), owner),
            heading: None,
            // Who each `<@U…>` in the words names, so the page and speech can say. `#318
            // slack-mention-names`; see `super::mentions`.
            mentions: super::mentions::names_for(text, resolved),
            // Formed when the message is served (`ops::stamp` asks this client's
            // `upstream_url`), never at read: the workspace address may not be learned yet.
            upstream_url: None,
            upstream_app_url: None,
            // The operation key a keyed post filed in its metadata. `#465 replication-c6-op-key-marker`.
            op_key: op_key_of(&self.value),
            observed_ms: None,
        })
    }
}

/// RFC 3339 UTC with microseconds: `2023-11-14T22:13:20.000100Z`.
pub(super) fn rfc3339(micros: i64) -> String {
    let secs = micros.div_euclid(1_000_000);
    let frac = micros.rem_euclid(1_000_000);
    match jiff::Timestamp::from_second(secs) {
        Ok(at) => format!("{}.{frac:06}Z", at.strftime("%Y-%m-%dT%H:%M:%S")),
        Err(_) => ids::micros_ts(micros),
    }
}

/// Map a refusal of a channel id or cursor before any request is sent.
pub(super) fn refused(detail: &str) -> ChatError {
    ChatError::Refused(detail.to_owned())
}

/// Map a malformed answer.
pub(super) fn shape(detail: &str) -> ChatError {
    ChatError::Shape(detail.to_owned())
}

#[async_trait]
impl ChatClient for HttpSlackClient {
    fn provider_name(&self) -> &str {
        &self.provider_name
    }

    fn claims_source(&self, source: &str) -> SourceClaim {
        // Any Slack link is ours, even one this client cannot parse: registration then explains
        // which link shapes it accepts, rather than another provider guessing at it.
        if ids::is_slack_link(source) || ids::parse_source(source).is_some() {
            SourceClaim::Certain
        } else {
            SourceClaim::Never
        }
    }

    fn self_author_id(&self) -> Option<String> {
        self.self_id()
    }

    fn owner_author_id(&self) -> Option<String> {
        self.owner_user_id.clone()
    }

    /// The message's Slack permalink. `#408 copy-upstream-link`.
    ///
    /// The conversation is read from the channel the message was served in — a thread-scoped
    /// channel names it before its `~` — and the `ts` from the message's own id. A reply's
    /// thread root is its thread record's id, which for Slack IS the root's `ts`; a root links
    /// plainly, as Slack's own link to it does. No workspace address held, no link.
    fn upstream_url(
        &self,
        channel: &ChannelId,
        id: &MessageId,
        thread: Option<&crate::threads::MessageThread>,
    ) -> Option<String> {
        let workspace = self
            .workspace_url
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let reference = ids::ChannelRef::parse(channel.as_str())?;
        let ts = ids::ts_from_message_id(id)?;
        let root = thread
            .filter(|thread| !thread.is_root)
            .map(|thread| thread.id.as_str())
            .or(reference.thread_ts.as_deref().filter(|root| *root != ts));
        crate::upstream_url::slack_permalink(&workspace, &reference.conversation, &ts, root)
    }

    /// The message's address in Slack's own app. `#481 slack-app-deep-link`.
    ///
    /// The same parts read the same way as the permalink, on the workspace's team: the team
    /// `auth.test` reported, held as the workspace's origin is. No team learned yet, no app
    /// address — the page then opens the permalink, as it always did.
    fn upstream_app_url(
        &self,
        channel: &ChannelId,
        id: &MessageId,
        thread: Option<&crate::threads::MessageThread>,
    ) -> Option<String> {
        let team = self
            .team_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        let reference = ids::ChannelRef::parse(channel.as_str())?;
        let ts = ids::ts_from_message_id(id)?;
        let root = thread
            .filter(|thread| !thread.is_root)
            .map(|thread| thread.id.as_str())
            .or(reference.thread_ts.as_deref().filter(|root| *root != ts));
        crate::upstream_url::slack_app_url(&team, &reference.conversation, &ts, root)
    }

    fn supports_threading(&self) -> bool {
        true
    }

    /// Slack has no reply pointer: a reply is posted in the target's thread (`timeline::post`).
    fn replies_in_thread(&self) -> bool {
        true
    }

    fn post_limit(&self, _channel: &ChannelId) -> crate::chat::PostLimit {
        self.post_limit
    }

    /// The thread the first part went into, or the one it now roots. `#263
    /// post-split-per-provider`.
    ///
    /// A Slack reply is a thread reply, and any message can root a thread: its own `ts` is the
    /// thread id. So the rest of a long post goes under the first part rather than after it, and a
    /// first part that answered a message — which Slack placed in that message's thread — is
    /// followed into the same thread rather than left there alone.
    fn continuation_thread(&self, channel: &ChannelId, first: &Message) -> Option<String> {
        let thread = first
            .thread
            .as_ref()
            .map(|thread| thread.id.clone())
            .or_else(|| ids::ts_from_message_id(&first.id))?;
        // Slack's answer to the first part has just shown this thread is there, so the rest goes
        // in without reading it first (`#448 send-dedupe`).
        if let Some(target) = ids::ChannelRef::parse(channel.as_str()) {
            self.note_posted_root(&target.conversation, &thread);
        }
        Some(thread)
    }

    async fn fetch_timeline(
        &self,
        channel: &ChannelId,
        request: &crate::threads::TimelineRequest,
    ) -> Result<crate::threads::TimelinePage, ChatError> {
        self.timeline(channel, request)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn post_in_thread(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.thread_post(channel, thread_id, content, reply_to, None)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn post_in_thread_keyed(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        content: &str,
        reply_to: Option<&MessageId>,
        key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.thread_post(channel, thread_id, content, reply_to, key)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// Slack keeps message metadata, invisible in its clients and returned by
    /// `conversations.history` and `conversations.replies` when asked for; Slack does not
    /// deduplicate by it. Every keyed post carries its key. `#465 replication-c6-op-key-marker`.
    ///
    /// Readable only from Slack's own Web API, and only once it has returned a key. A bridge in
    /// front of Slack is never readable, whatever it echoes: one whose client cannot read message
    /// metadata can only hand back the keys of posts it saw succeed, from memory, so a post whose
    /// answer was lost never shows its key, and a read without one proves nothing.
    fn op_key_carrier(&self, _channel: &ChannelId) -> crate::chat::OpKeyCarrier {
        crate::chat::OpKeyCarrier {
            carried: true,
            dedupes: crate::chat::OpKeyDedupe::Never,
            readable: super::is_official_slack_api_base(&self.api_base)
                && self
                    .op_keys_returned
                    .load(std::sync::atomic::Ordering::Relaxed),
        }
    }

    async fn fetch_thread_message(
        &self,
        channel: &ChannelId,
        thread_id: &str,
        message_id: &MessageId,
    ) -> Result<Message, ChatError> {
        self.thread_message(channel, thread_id, message_id)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    fn supports_channel_registration(&self) -> bool {
        self.channel_registration
    }

    async fn register_channel(
        &self,
        source: &str,
        label: &str,
    ) -> Result<RegisteredChannel, ChatError> {
        self.register(source, label)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn unregister_channel(&self, _channel: &ChannelId) -> Result<(), ChatError> {
        // Registration created nothing upstream, so there is nothing to remove there.
        if self.channel_registration {
            Ok(())
        } else {
            Err(refused(
                "channel registration is disabled; ask the deployment operator to enable it for this backend",
            )
            .with_provider(&self.provider_name))
        }
    }

    fn supports_channel_discovery(&self) -> bool {
        self.channel_discovery
    }

    fn forget_channel_directory(&self) {
        self.directory_pages.forget();
    }

    /// While it registers channels, every conversation added in the app is one it registered, and
    /// `registered_channels_writable` is its policy for them. `#374 registered-writable-follows-policy`.
    fn added_channels_writable(&self) -> Option<bool> {
        self.channel_registration
            .then_some(self.registered_writable)
    }

    async fn discover_channels(
        &self,
        request: &crate::directory::DirectoryRequest,
    ) -> Result<crate::directory::DirectoryPage, ChatError> {
        self.discover(request)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// The owner's manual "Mark read in" a message is not offered: only the automatic clear is.
    fn supports_upstream_read_mark(&self) -> bool {
        false
    }

    /// Only when the deployment turned on `upstream_clear` for this provider. `#258
    /// slack-clear-unread`.
    fn supports_upstream_clear(&self) -> bool {
        self.upstream_clear
    }

    fn crawl_hold(&self) -> Option<Duration> {
        let now = tokio::time::Instant::now();
        let mut pressure = self.pressure();
        let mut latest: Option<tokio::time::Instant> = None;
        for method in CRAWL_METHODS {
            if let Some(until) = pressure
                .get_mut(method)
                .and_then(|entry| entry.hold_until(now))
            {
                latest = Some(latest.map_or(until, |at| at.max(until)));
            }
        }
        latest
            .map(|until| until.saturating_duration_since(now))
            .filter(|wait| !wait.is_zero())
    }

    fn rate_limit_pressure(&self) -> Option<crate::chat::RateLimitPressure> {
        let now = tokio::time::Instant::now();
        let mut report = crate::chat::RateLimitPressure::default();
        {
            let mut pressure = self.pressure();
            for (method, entry) in pressure.iter_mut() {
                entry.prune(now);
                if entry.limited.is_empty() && entry.remaining.is_none() {
                    continue;
                }
                let count = u32::try_from(entry.limited.len()).unwrap_or(u32::MAX);
                report.limited_last_10_min = report.limited_last_10_min.saturating_add(count);
                report.routes.insert(
                    method.clone(),
                    crate::chat::RoutePressure {
                        limited_last_10_min: count,
                        last_limited_s_ago: entry
                            .limited
                            .back()
                            .map(|at| now.saturating_duration_since(*at).as_secs()),
                        remaining: entry.remaining,
                    },
                );
            }
        }
        report.crawl_hold_s = self.crawl_hold().map(|wait| wait.as_secs().max(1));
        Some(report)
    }

    async fn clear_upstream_through(
        &self,
        channel: &ChannelId,
        through: &MessageId,
        dry_run: bool,
    ) -> Result<crate::chat::UpstreamClearOutcome, ChatError> {
        self.clear_through(channel, through, dry_run)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// The token's account from `auth.test`, named as people see it: by its display name from
    /// `users.info` (the cache authors' names go through, best effort), and by the handle
    /// `auth.test` reports only when no display name can be had. A handle may be a number, which
    /// names nobody to a reader. `#325 provider-diagnostics-ids`.
    async fn identity(&self) -> Result<ChatIdentity, ChatError> {
        async {
            let value = self.call("auth.test", &[]).await?;
            let id = value
                .get("user_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| shape("auth.test answer has no string \"user_id\" field"))?;
            self.learn_self(id);
            // `#408 copy-upstream-link`: `auth.test` also reports the workspace's own address,
            // which is where a message's permalink lives. Learned only where the configuration
            // did not say one — the operator's word stands over the provider's — and only when
            // it is a Slack host: whatever answers at the API base does not get to name the host
            // of every link a reader copies. A bridge whose permalinks live elsewhere says so in
            // the configuration (`workspace_url`), which is the operator's word to give.
            if let Some(reported) = value
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| ids::is_slack_link(url))
                .and_then(crate::upstream_url::workspace_origin)
            {
                let mut slot = self
                    .workspace_url
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if slot.is_none() {
                    *slot = Some(reported);
                }
            }
            // The team id a message's address in Slack's app names (`#481
            // slack-app-deep-link`), held as the workspace's origin is: learned once, only
            // ever read back to form an address, and only in Slack's own shape.
            if let Some(team) = value
                .get("team_id")
                .and_then(Value::as_str)
                .filter(|team| ids::is_team_id(team))
            {
                let mut slot = self
                    .team_id
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if slot.is_none() {
                    *slot = Some(team.to_owned());
                }
            }
            let handle = value
                .get("user")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|handle| !handle.is_empty())
                .unwrap_or("(no username)");
            let username = match self.user(id).await {
                Some((name, _)) => name,
                None => handle.to_owned(),
            };
            Ok::<_, ChatError>(ChatIdentity {
                id: id.to_owned(),
                username,
                // A blank team id is no workspace at all: one user id can then not be told
                // apart across workspaces, and saying `Some("")` would pretend it could.
                workspace: value
                    .get("team_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|team| !team.is_empty())
                    .map(str::to_owned),
            })
        }
        .await
        .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, ChatError> {
        self.read_page(channel, limit, before, after)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// The conversation's own newest messages, as before replies were read for the newest
    /// messages: a reachability check needs no thread. `#307 slack-reply-live`.
    async fn probe_channel(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.read_page(channel, limit, None, None)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// The newest messages, thread replies included. `#307 slack-reply-live`; see
    /// `HttpSlackClient::recent`.
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, ChatError> {
        self.recent(channel, limit)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// `conversations.history` lists no thread reply, so the live poller reads a whole
    /// conversation's replies apart. A channel narrowed to one thread reads them in its own forward
    /// read. `#307 slack-reply-live`; see `super::live`.
    fn polls_thread_replies(&self, channel: &ChannelId) -> bool {
        Self::channel_ref(channel).is_ok_and(|target| target.thread_ts.is_none())
    }

    async fn fetch_live(
        &self,
        channel: &ChannelId,
        limit: u16,
        after: Option<&MessageId>,
    ) -> Result<crate::chat::LiveRead, ChatError> {
        self.live(channel, limit, after)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    /// Slack's own event stream delivers each new message with its text, so it is normalized
    /// here as a read would normalize it. `#452 slack-live-ingest`; see `super::tail`.
    fn takes_pushed_messages(&self, channel: &ChannelId) -> bool {
        Self::channel_ref(channel).is_ok()
    }

    /// One `conversations.history`-shaped object, built by `super::tail` from a stream record,
    /// through the same [`HttpSlackClient::messages`] every read passes through: the same author
    /// lookup, mentions and thread mapping, so a later read of it is the same message.
    async fn pushed_message(
        &self,
        channel: &ChannelId,
        raw: Value,
    ) -> Result<Option<Message>, ChatError> {
        Self::channel_ref(channel)?;
        let raw = Raw::parse(raw)?;
        if !raw.is_real() {
            return Ok(None);
        }
        Ok(self.messages(channel, &[raw]).await.into_iter().next())
    }

    /// Slack reads by time itself: `conversations.history` and `conversations.replies` take
    /// `oldest`, `latest` and a cursor. remote-ops-audit W10 `#287 bulk-reads`; see `super::range`.
    /// The same bounds look several messages up a run of timestamps a call (W11, see
    /// `super::lookup`).
    ///
    /// Heads stay emulated (remote-ops-audit W12): no Slack call answers the newest message of
    /// several conversations, so a head here is a one-message history read per channel, and the
    /// callers that would use heads only as a cheap gate do not ask.
    ///
    /// Reads of several channels stay emulated too (remote-ops-audit W13): no Slack call reads the
    /// newest messages of several conversations at once, so a read of several is the trait's
    /// default, one history read per channel, and a catch-up reads Slack channels one by one as
    /// before.
    ///
    /// A read of several threads of a conversation is native (remote-ops-audit W14, see
    /// `super::threads`): a thread's newest page costs one `conversations.replies` window read
    /// instead of the thread view's two, and after an instant a thread whose root has not moved
    /// costs no read of its replies.
    fn read_caps(&self, _channel: &ChannelId) -> crate::chat::ReadCaps {
        crate::chat::ReadCaps {
            range: crate::chat::Support::Native,
            by_ids: crate::chat::Support::Native,
            heads: crate::chat::Support::Emulated,
            heads_cover_edits: false,
            many_channels: crate::chat::Support::Emulated,
            many_threads: crate::chat::Support::Native,
            page_rows: super::range::PAGE_ROWS,
        }
    }

    async fn fetch_range(
        &self,
        channel: &ChannelId,
        scope: &crate::chat::ReadScope,
        range: crate::chat::TimeRange,
        order: crate::chat::RangeOrder,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<crate::chat::RangeRead, ChatError> {
        self.read_range(channel, scope, range, order, limit, cursor)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn fetch_messages(
        &self,
        channel: &ChannelId,
        thread: Option<&str>,
        ids: &[MessageId],
    ) -> Result<Vec<crate::chat::Lookup>, ChatError> {
        self.lookup(channel, thread, ids)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn fetch_threads(
        &self,
        channel: &ChannelId,
        asks: &[crate::chat::ThreadAsk],
    ) -> crate::chat::ThreadAnswers {
        self.threads(channel, asks)
            .await
            .into_iter()
            .map(|(id, answer)| {
                (
                    id,
                    answer.map_err(|error| match error {
                        // The emulated path for a narrowed channel already labelled its errors.
                        ChatError::Provider { .. } => error,
                        error => error.with_provider(&self.provider_name),
                    }),
                )
            })
            .collect()
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, ChatError> {
        self.post(channel, content, reply_to, None)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }

    async fn post_message_keyed(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
        key: Option<&str>,
    ) -> Result<Message, ChatError> {
        self.post(channel, content, reply_to, key)
            .await
            .map_err(|error| error.with_provider(&self.provider_name))
    }
}

/// The `event_type` vibe-talk's operation-key metadata is filed under. `#465
/// replication-c6-op-key-marker`. Its own name, so metadata a transport files for its own purposes
/// under another type is never read as a key.
pub const OP_KEY_EVENT_TYPE: &str = "vibe_talk_op_key";

/// The `metadata` parameter of a `chat.postMessage` carrying `key`: Slack's documented shape,
/// `{event_type, event_payload}`.
pub(super) fn op_key_metadata(key: &str) -> String {
    serde_json::json!({
        "event_type": OP_KEY_EVENT_TYPE,
        "event_payload": { "op_key": key },
    })
    .to_string()
}

/// The operation key a message object carries in vibe-talk's metadata, if any. Metadata another
/// app filed under another `event_type` is not ours and is ignored.
pub(super) fn op_key_of(value: &Value) -> Option<String> {
    let metadata = value.get("metadata")?;
    if metadata.get("event_type").and_then(Value::as_str) != Some(OP_KEY_EVENT_TYPE) {
        return None;
    }
    metadata
        .get("event_payload")
        .and_then(|payload| payload.get("op_key"))
        .and_then(Value::as_str)
        .filter(|key| !key.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> HttpSlackClient {
        HttpSlackClient::new(&SlackConfig {
            provider_name: "Slack".to_owned(),
            api_base: "https://slack.example/api".to_owned(),
            token: Secret::new("xoxp-not-a-real-token"),
            owner_user_id: None,
            request_timeout_seconds: 5,
            channel_registration: false,
            registered_channels_writable: false,
            channel_discovery: false,
            upstream_clear: false,
            max_post_chars: None,
            max_post_bytes: None,
            workspace_url: None,
        })
        .expect("client")
    }

    fn client_at(api_base: &str) -> HttpSlackClient {
        HttpSlackClient::new(&SlackConfig {
            provider_name: "Slack".to_owned(),
            api_base: api_base.to_owned(),
            token: Secret::new("xoxp-not-a-real-token"),
            owner_user_id: None,
            request_timeout_seconds: 5,
            channel_registration: false,
            registered_channels_writable: false,
            channel_discovery: false,
            upstream_clear: false,
            max_post_chars: None,
            max_post_bytes: None,
            workspace_url: None,
        })
        .expect("client")
    }

    #[test]
    fn an_app_address_waits_for_a_learned_team_and_names_the_message_and_its_thread() {
        // `#481 slack-app-deep-link`: no team learned yet, no app address; once `auth.test`
        // has reported one (held in the slot as it learns it), a message's address names its
        // conversation and ts, a reply's its thread root too — by the same reads as the
        // permalink beside it, so the two cannot name different messages.
        let client = client();
        let channel = ChannelId("C0123ABCD".to_owned());
        let root_id = ids::message_id_from_ts("1700000000.000100").expect("root id");
        let reply_id = ids::message_id_from_ts("1700000060.000200").expect("reply id");
        assert_eq!(client.upstream_app_url(&channel, &reply_id, None), None);
        *client.team_id.lock().expect("team slot") = Some("T0123ABCD".to_owned());
        assert_eq!(
            client.upstream_app_url(&channel, &root_id, None),
            Some(
                "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000000.000100".to_owned()
            )
        );
        let thread = MessageThread {
            hue: None,
            display_name: None,
            summary: None,
            root_message_id: Some(root_id.clone()),
            id: "1700000000.000100".to_owned(),
            is_root: false,
            reply_count: None,
            reply_count_exact: false,
        };
        assert_eq!(
            client.upstream_app_url(&channel, &reply_id, Some(&thread)),
            Some(
                "slack://channel?team=T0123ABCD&id=C0123ABCD&message=1700000060.000200\
                 &thread_ts=1700000000.000100"
                    .to_owned()
            )
        );
    }

    #[tokio::test]
    async fn only_slack_itself_is_readable_and_only_once_a_key_came_back() {
        // `#465 replication-c6-op-key-marker`. A message as a read returns it with our metadata,
        // and one with another party's.
        let ours = Raw::parse(serde_json::json!({
            "ts": "1700000000.000100", "user": "U0ADA", "text": "on it",
            "user_profile": {"display_name": "Ada"},
            "metadata": {"event_type": OP_KEY_EVENT_TYPE, "event_payload": {"op_key": "k.1"}},
        }))
        .expect("raw");
        let channel = ChannelId("C0123ABCDE".to_owned());
        for (base, readable_after) in [
            ("https://slack.com/api", true),
            ("https://slack.example/api", false),
        ] {
            let client = client_at(base);
            let before = client.op_key_carrier(&channel);
            assert!(before.carried && !before.readable, "{base}: {before:?}");
            let read = client.messages(&channel, std::slice::from_ref(&ours)).await;
            assert_eq!(read[0].op_key.as_deref(), Some("k.1"));
            assert_eq!(
                client.op_key_carrier(&channel).readable,
                readable_after,
                "{base}: a bridge that echoes some keys is still not readable"
            );
        }
        assert_eq!(
            op_key_of(
                &serde_json::json!({"metadata": {"event_type": "vibe_talk_post",
            "event_payload": {"op_key": "theirs"}}})
            ),
            None
        );
    }

    #[test]
    fn a_slack_error_code_is_read_back_out_of_the_failure_it_became() {
        let client = client();
        let scoped = client.slack_error(
            "conversations.mark",
            "missing_scope",
            &serde_json::json!({"ok": false, "error": "missing_scope", "needed": "groups:write"}),
        );
        assert_eq!(
            HttpSlackClient::slack_error_code(&scoped, "conversations.mark"),
            Some(("missing_scope", Some("groups:write")))
        );
        let wrapped = client
            .slack_error("conversations.info", "unknown_method", &Value::Null)
            .with_provider("Slack");
        assert_eq!(
            HttpSlackClient::slack_error_code(&wrapped, "conversations.info"),
            Some(("unknown_method", None))
        );
        assert_eq!(
            HttpSlackClient::slack_error_code(&wrapped, "conversations.mark"),
            None,
            "another method's answer is not this one's"
        );
        assert_eq!(
            HttpSlackClient::slack_error_code(
                &ChatError::Transport("reset".to_owned()),
                "conversations.mark"
            ),
            None
        );
    }

    /// `#208 persistent-cache`: the message cache reads Slack's code back out of the failure this
    /// client makes ([`crate::cache::access::classify`]) to decide whether access to a channel or
    /// a thread is lost, or the credential refused. The two sides are pinned together here, so a
    /// change to the body's shape fails this test rather than a cache in production.
    #[test]
    fn the_message_cache_reads_back_the_code_a_slack_failure_carries() {
        use crate::cache::access::{slack_code, Dialect, Signal};
        let classify = |op: &str, thread: Option<&str>, error: &ChatError| {
            crate::cache::access::classify(Dialect::Slack, op, thread, error)
        };
        let client = client();
        let failure = |method: &str, code: &str| {
            client
                .slack_error(
                    method,
                    code,
                    &serde_json::json!({"ok": false, "error": code, "needed": "channels:history"}),
                )
                .with_provider("Slack")
        };
        for code in UNAUTHORIZED
            .iter()
            .chain(FORBIDDEN.iter())
            .chain(NOT_FOUND.iter())
            .chain(["invalid_cursor"].iter())
        {
            let error = failure("conversations.history", code);
            let ChatError::Status { body, .. } = error.cause() else {
                panic!("{code}: a status");
            };
            assert_eq!(slack_code(body), Some(*code), "{body}");
        }

        for (code, revoked) in [
            ("not_authed", false),
            ("invalid_auth", false),
            ("token_expired", false),
            ("account_inactive", true),
            ("token_revoked", true),
        ] {
            assert_eq!(
                classify("post_message", None, &failure("chat.postMessage", code)),
                Signal::CredentialRefused { revoked },
                "{code}"
            );
        }
        let lost = |thread: Option<&str>| Signal::ChannelLost {
            thread: thread.map(str::to_owned),
        };
        assert_eq!(
            classify(
                "fetch_timeline",
                None,
                &failure("conversations.history", "channel_not_found")
            ),
            lost(None)
        );
        assert_eq!(
            classify(
                "fetch_page",
                None,
                &failure("conversations.history", "not_in_channel")
            ),
            lost(None)
        );
        let root = "1700000000.000100";
        assert_eq!(
            classify(
                "fetch_timeline",
                Some(root),
                &failure("conversations.replies", "thread_not_found")
            ),
            lost(Some(root))
        );
        for code in [
            "message_not_found",
            "user_not_found",
            "missing_scope",
            "not_allowed_token_type",
            "restricted_action",
            "is_archived",
            "access_denied",
            "no_permission",
            "invalid_cursor",
        ] {
            assert_eq!(
                classify(
                    "fetch_timeline",
                    None,
                    &failure("conversations.history", code)
                ),
                Signal::Excluded,
                "{code}"
            );
        }
    }
}
