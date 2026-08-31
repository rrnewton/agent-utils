//! An in-memory Discord stand-in for tests and for local development.
//!
//! It is not a mock that agrees with whatever it is asked. It shares the real client's request
//! validation ([`super::http::post_request`]) and the real client's ordering contract, so a test
//! written against it exercises the same refusals and the same oldest-first normalization that
//! production code takes. The binary never constructs one unless `--fake-discord` is passed, and
//! that flag logs a warning on every startup.
//!
//! # It must be able to say no
//!
//! A fake that answers every fetch with an empty success cannot fail a reachability check, so any
//! check written against it would be self-certifying — green here and worth nothing about the real
//! client. So this fake tracks which channels it actually *has*: a channel is known once it has
//! been seeded or explicitly registered, and a fetch or post aimed at any other id answers the way
//! Discord answers for a channel a bot cannot see — HTTP 404, `Unknown Channel`, code 10003. That
//! is what makes [`crate::probe`] a real check when it runs against this fake.
//!
//! # It must be able to say "not so fast"
//!
//! For the same reason it can 404, it can **429**. [`FakeDiscord::rate_limit_next`] queues
//! Discord's own rate-limit answer — the JSON body with `retry_after` in fractional seconds and a
//! `global` flag — and [`FakeDiscord::set_bucket`] makes its successful answers carry Discord's
//! `X-RateLimit-*` accounting. Both go through the SAME [`super::ratelimit::RateLimiter`] the live
//! client uses, so a test written here exercises production's waiting, its bucket bookkeeping and
//! its loud exhaustion rather than a second implementation of them.
//!
//! What it is not is evidence about live Discord. The header names and the body shape are from the
//! documentation; the first real 429 will be the first real 429.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;

use super::ratelimit::{Attempt, Headers, RateLimit, RateLimiter, RetryPolicy};
use super::{BotIdentity, DiscordClient, DiscordError};
use crate::config::DEFAULT_DISCORD_API_BASE;
use crate::model::{sort_oldest_first, ChannelId, Message, MessageId, UserId};

/// The bot user id [`FakeDiscord`] answers `GET /users/@me` with.
pub const FAKE_BOT_USER_ID: &str = "3000000000000000001";
/// The bot username [`FakeDiscord`] answers `GET /users/@me` with.
pub const FAKE_BOT_USERNAME: &str = "gent-talk-fake-bot";

/// A message this fake was asked to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostedMessage {
    /// Channel the post was aimed at.
    pub channel: ChannelId,
    /// Body that was posted.
    pub content: String,
    /// Message being replied to, when any.
    pub reply_to: Option<MessageId>,
}

/// In-memory Discord.
#[derive(Debug, Default)]
pub struct FakeDiscord {
    state: Mutex<State>,
    /// The real rate-limit engine, not a stand-in for it. See the module documentation.
    limiter: RateLimiter,
}

#[derive(Debug, Default)]
struct State {
    known: BTreeSet<ChannelId>,
    fetches: usize,
    messages: Vec<Message>,
    posted: Vec<PostedMessage>,
    next_id: u64,
    /// Author display name -> snowflake, so one author keeps one id and two authors never share
    /// one. A fake that handed every author the same id would let a test pass while the server
    /// mentioned the wrong person.
    authors: BTreeMap<String, UserId>,
    fail_with: Option<String>,
    /// Whether this fake's account has revoked the token it is being called with.
    ///
    /// Sticky rather than one-shot, unlike `fail_with`: a revoked credential does not heal on the
    /// next request, and a fake whose 401 cleared itself would let a caller that simply retries
    /// past a bad token look correct.
    token_revoked: bool,
    /// Queued rate-limit answers. Each one is spent by the next request that is actually SENT, so
    /// a request the limiter held back does not consume one.
    rate_limits: VecDeque<RateLimit>,
    /// The `X-RateLimit-*` accounting successful answers carry, when a test asked for any.
    bucket_headers: Headers,
}

impl FakeDiscord {
    /// An empty fake.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty fake whose rate-limit budget is `policy` rather than production's.
    ///
    /// Tests that need EXHAUSTION reachable use this; everything else takes the real bounds, so
    /// the ordinary suite is measuring the ordinary policy.
    #[must_use]
    pub fn with_retry_policy(policy: RetryPolicy) -> Self {
        Self {
            state: Mutex::new(State::default()),
            limiter: RateLimiter::with_policy(policy),
        }
    }

    /// The rate-limit engine this fake shares with the live client.
    #[must_use]
    pub fn limiter(&self) -> &RateLimiter {
        &self.limiter
    }

    /// Answer the next `count` requests with Discord's own 429, asking for `retry_after`.
    ///
    /// `global` picks which kind: a per-route bucket limit, or the whole-token one that stops
    /// every channel at once. Queued rather than sticky, so a test can say "rate-limited twice,
    /// then fine" and watch the retry actually clear it.
    pub fn rate_limit_next(&self, count: usize, retry_after: Duration, global: bool) {
        let mut state = self.lock();
        for _ in 0..count {
            state.rate_limits.push_back(RateLimit {
                retry_after,
                global,
                bucket: None,
                scope: Some(if global { "global" } else { "user" }.to_owned()),
            });
        }
    }

    /// Make successful answers carry Discord's bucket accounting.
    ///
    /// `remaining: 0` is Discord saying the next request on this bucket is going to be rejected;
    /// the limiter is expected to wait `reset_after` out rather than spend it.
    pub fn set_bucket(&self, remaining: u64, reset_after: Duration) {
        self.lock().bucket_headers = Headers::new()
            .with("x-ratelimit-remaining", &remaining.to_string())
            .with(
                "x-ratelimit-reset-after",
                &format!("{:.3}", reset_after.as_secs_f64()),
            );
    }

    /// Declare that this fake has `channel`, without putting anything in it.
    ///
    /// The counterpart of a real bot having been invited to a channel that happens to be empty.
    /// Anything not registered — and not seeded — is answered exactly as Discord answers for a
    /// channel a bot cannot see.
    pub fn register_channel(&self, channel: &ChannelId) {
        self.lock().known.insert(channel.clone());
    }

    /// Seed a message into a channel, registering the channel as a side effect. Returns its
    /// assigned snowflake.
    pub fn seed(&self, channel: &ChannelId, author: &str, content: &str) -> MessageId {
        let mut state = self.lock();
        state.known.insert(channel.clone());
        state.next_id += 1;
        let seq = state.next_id;
        // Start well above 0 so ids are snowflake-shaped and exercise numeric ordering.
        let id = MessageId(format!("{}", 1_000_000_000_000_000_000_u64 + seq));
        let next_author = 2_000_000_000_000_000_000_u64 + state.authors.len() as u64 + 1;
        let author_id = state
            .authors
            .entry(author.to_owned())
            .or_insert_with(|| UserId(next_author.to_string()))
            .clone();
        state.messages.push(Message {
            id: id.clone(),
            channel_id: channel.clone(),
            author: author.to_owned(),
            author_id,
            author_is_bot: author.ends_with("-bot"),
            timestamp: format!("2026-08-18T12:{:02}:00+00:00", seq % 60),
            // Empty for the same reason the real client leaves it empty: this layer knows nothing
            // about the operator's zone. A fake that pre-filled it would let a test pass with
            // `ops::stamp` deleted. See `crate::model::Message::spoken_time`.
            spoken_time: String::new(),
            // Seeding an ordinary message. `seed_reply` is how a fixture makes one that answers
            // another, and it is a separate call so that "this is a reply" is always something a
            // test said out loud rather than something it inherited.
            reply_to: None,
            content: content.to_owned(),
        });
        id
    }

    /// Make an already-seeded message a reply to `parent`.
    ///
    /// The counterpart of [`FakeDiscord::seed_reply`] for a fixture that seeded a whole backlog
    /// first and only then decided which of its messages answer which — which is what a
    /// development seed does, because the answers are chosen by reading the text.
    pub fn set_reply_to(&self, message: &MessageId, parent: &MessageId) {
        let mut state = self.lock();
        if let Some(found) = state.messages.iter_mut().find(|m| &m.id == message) {
            found.reply_to = Some(parent.clone());
        }
    }

    /// Seed a message that REPLIES to `parent`, as Discord records a reply.
    ///
    /// The pointer lives on the answering message and nowhere else, which is exactly the fact the
    /// inbox view is built on, so a fixture has to be able to produce that shape.
    pub fn seed_reply(
        &self,
        channel: &ChannelId,
        author: &str,
        content: &str,
        parent: &MessageId,
    ) -> MessageId {
        let id = self.seed(channel, author, content);
        let mut state = self.lock();
        let message = state
            .messages
            .iter_mut()
            .find(|m| m.id == id)
            .expect("the message just seeded is present");
        message.reply_to = Some(parent.clone());
        id
    }

    /// Seed a message that really was created at `at_ms`, id and timestamp agreeing.
    ///
    /// [`FakeDiscord::seed`] mints sequential ids whose embedded snowflake time has nothing to do
    /// with the `timestamp` it writes — harmless for ordering, useless for a time range, and
    /// actively misleading if a range test were written against it. Real Discord ids encode their
    /// own creation instant, and anything walking a time span depends on that; so a test about
    /// time uses this, and gets a fixture where the two agree to the millisecond.
    pub fn seed_at(
        &self,
        channel: &ChannelId,
        author: &str,
        content: &str,
        at_ms: i64,
    ) -> MessageId {
        let id = self.seed(channel, author, content);
        let mut state = self.lock();
        let at = MessageId::at_time_ms(at_ms);
        let iso = crate::clock::iso_from_ms(at_ms);
        let message = state
            .messages
            .iter_mut()
            .find(|m| m.id == id)
            .expect("the message just seeded is present");
        message.id = at.clone();
        message.timestamp = iso;
        at
    }

    /// Rewrite the content of a message already seeded, as an upstream EDIT would.
    ///
    /// Discord messages are editable and the id does not change, so anything derived from a
    /// message's TEXT — `#49 cached-summaries`, in particular — has to be able to notice. Without
    /// this the only way to get different text is a different id, and a cache keyed on the id
    /// alone would pass that test while serving a stale summary forever.
    ///
    /// # Panics
    ///
    /// Panics if no such message was seeded, which is a mistake in the test that called it.
    pub fn edit(&self, id: &MessageId, content: &str) {
        let mut state = self.lock();
        let message = state
            .messages
            .iter_mut()
            .find(|m| m.id == *id)
            .unwrap_or_else(|| panic!("no seeded message {id}"));
        message.content = content.to_owned();
    }

    /// The snowflake this fake assigned to `author`, if it has ever seen them speak.
    ///
    /// Deliberately NOT a general directory: a name this fake has never seen has no id, exactly
    /// as a real caller can only mention someone who has actually posted in an allowlisted
    /// channel.
    #[must_use]
    pub fn author_id(&self, author: &str) -> Option<UserId> {
        self.lock().authors.get(author).cloned()
    }

    /// Make the next operation fail, so error handling can be tested.
    pub fn fail_next(&self, detail: &str) {
        self.lock().fail_with = Some(detail.to_owned());
    }

    /// Answer every call from now on with Discord's own 401, as a revoked bot token does.
    ///
    /// The counterpart of [`FakeDiscord::register_channel`] for the CREDENTIAL. Without it this
    /// fake accepts any token, so "the token is wrong" and "the channel is unreachable" — the two
    /// failures the diagnostics route most needs to keep apart — would be untestable here, and a
    /// check written against this fake would be certifying nothing. Sticky, because a real
    /// revoked token is.
    pub fn revoke_token(&self) {
        self.lock().token_revoked = true;
    }

    /// How many reads this fake has been asked for.
    ///
    /// Lets a test prove a read did *not* happen — "the probe was skipped" is otherwise
    /// indistinguishable from "the probe ran and happened to pass".
    #[must_use]
    pub fn fetch_count(&self) -> usize {
        self.lock().fetches
    }

    /// Everything that has been posted through this fake, in order.
    #[must_use]
    pub fn posted(&self) -> Vec<PostedMessage> {
        self.lock().posted.clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn take_failure(&self) -> Option<DiscordError> {
        self.lock().fail_with.take().map(DiscordError::Transport)
    }

    /// Discord's own answer for a token it will not accept, in the shape a live 401 arrives in,
    /// so [`crate::probe::classify`] reaches [`crate::probe::Diagnosis::InvalidToken`] through
    /// the production code path rather than through a test-only shortcut.
    fn rejected_token(&self) -> Option<DiscordError> {
        self.lock().token_revoked.then(|| DiscordError::Status {
            status: 401,
            body: r#"{"message": "401: Unauthorized", "code": 0}"#.to_owned(),
        })
    }

    /// The rate-limit answer this request gets, if one is queued for it.
    fn take_rate_limit(&self) -> Option<RateLimit> {
        self.lock().rate_limits.pop_front()
    }

    /// The headers a successful answer carries.
    fn success_headers(&self) -> Headers {
        self.lock().bucket_headers.clone()
    }

    /// Discord's own answer for a channel this bot cannot see, byte for byte in shape, so that
    /// [`crate::probe`] classifies it through exactly the code path a live 404 takes.
    fn unknown_channel(&self, channel: &ChannelId) -> Option<DiscordError> {
        if self.lock().known.contains(channel) {
            return None;
        }
        Some(DiscordError::Status {
            status: 404,
            body: r#"{"message": "Unknown Channel", "code": 10003}"#.to_owned(),
        })
    }
}

#[async_trait]
impl DiscordClient for FakeDiscord {
    async fn identity(&self) -> Result<BotIdentity, DiscordError> {
        // Through the real URL builder, so this fake cannot drift from the endpoint the live
        // client calls, and through the same limiter, so a queued 429 is spent here too.
        let request = super::http::identity_request(DEFAULT_DISCORD_API_BASE);
        self.limiter
            .run(request.method, &request.url, || async {
                if let Some(limit) = self.take_rate_limit() {
                    return Ok(Attempt::Limited(limit));
                }
                if let Some(failure) = self.take_failure() {
                    return Err(failure);
                }
                if let Some(rejected) = self.rejected_token() {
                    return Err(rejected);
                }
                Ok(Attempt::Done(
                    BotIdentity {
                        id: FAKE_BOT_USER_ID.to_owned(),
                        username: FAKE_BOT_USERNAME.to_owned(),
                    },
                    self.success_headers(),
                ))
            })
            .await
    }

    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, DiscordError> {
        // Share the real client's refusal, so a caller cannot get away here with a request
        // Discord would not define. OUTSIDE the retry loop: a request this server refuses to
        // build was never going to be sent, so it neither spends an attempt nor counts as a fetch.
        let request =
            super::http::page_request(DEFAULT_DISCORD_API_BASE, channel, limit, before, after)?;
        self.limiter
            .run(request.method, &request.url, || async {
                // Everything below here is ONE SENT REQUEST, which is what makes `fetch_count`
                // able to prove that a preempted request really was not sent.
                self.lock().fetches += 1;
                if let Some(limit) = self.take_rate_limit() {
                    return Ok(Attempt::Limited(limit));
                }
                if let Some(failure) = self.take_failure() {
                    return Err(failure);
                }
                // BEFORE the channel lookup, exactly as Discord orders it: a rejected credential
                // must never be reported as a channel that does not exist, or the operator is
                // sent to check a snowflake that was right all along.
                if let Some(rejected) = self.rejected_token() {
                    return Err(rejected);
                }
                if let Some(unknown) = self.unknown_channel(channel) {
                    return Err(unknown);
                }
                let cursor = |id: Option<&MessageId>| id.and_then(MessageId::numeric);
                let (before, after) = (cursor(before), cursor(after));
                let state = self.lock();
                let mut messages: Vec<Message> = state
                    .messages
                    .iter()
                    .filter(|m| &m.channel_id == channel)
                    .filter(|m| {
                        let Some(id) = m.id.numeric() else {
                            // An unparseable id cannot be placed relative to a cursor, so it is
                            // only ever returned by an uncursored fetch. Guessing would fabricate
                            // an ordering.
                            return before.is_none() && after.is_none();
                        };
                        before.is_none_or(|b| id < b) && after.is_none_or(|a| id > a)
                    })
                    .cloned()
                    .collect();
                drop(state);
                sort_oldest_first(&mut messages);
                let limit = usize::from(limit.clamp(1, super::http::DISCORD_MAX_LIMIT));
                if messages.len() > limit {
                    if after.is_some() {
                        // `after` walks FORWARD: Discord answers with the OLDEST messages
                        // following the cursor, not the newest ones. Taking the newest here would
                        // make a forward walk skip everything in between and still look correct
                        // in a test.
                        messages.truncate(limit);
                    } else {
                        messages.drain(..messages.len() - limit);
                    }
                }
                Ok(Attempt::Done(messages, self.success_headers()))
            })
            .await
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, DiscordError> {
        // Share the real client's validation so a test cannot pass on input Discord would reject.
        // Validation first, then existence, because Discord also rejects an empty body before it
        // ever looks the channel up.
        let request =
            super::http::post_request(DEFAULT_DISCORD_API_BASE, channel, content, reply_to)?;
        self.limiter
            .run(request.method, &request.url, || async {
                if let Some(limit) = self.take_rate_limit() {
                    return Ok(Attempt::Limited(limit));
                }
                if let Some(failure) = self.take_failure() {
                    return Err(failure);
                }
                if let Some(unknown) = self.unknown_channel(channel) {
                    return Err(unknown);
                }
                // Through `seed_reply` when this really is a reply, so the fake's own channel ends
                // up in the shape Discord's would: the pointer on the ANSWERING message. Without
                // this, posting a reply here produced a loose message and the parent never showed
                // as answered — which is exactly the behaviour the inbox view is built on, so the
                // fake would have been unable to exercise the one loop that matters.
                let id = match reply_to {
                    Some(parent) => self.seed_reply(channel, "gent-talk", content, parent),
                    None => self.seed(channel, "gent-talk", content),
                };
                self.lock().posted.push(PostedMessage {
                    channel: channel.clone(),
                    content: content.to_owned(),
                    reply_to: reply_to.cloned(),
                });
                let state = self.lock();
                let message = state
                    .messages
                    .iter()
                    .find(|m| m.id == id)
                    .cloned()
                    .ok_or_else(|| DiscordError::Shape("posted message vanished".to_owned()))?;
                drop(state);
                Ok(Attempt::Done(message, self.success_headers()))
            })
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel() -> ChannelId {
        ChannelId("c1".to_owned())
    }

    #[tokio::test]
    async fn fetch_returns_oldest_first_and_respects_the_limit() {
        let fake = FakeDiscord::new();
        for i in 0..5 {
            fake.seed(&channel(), "a", &format!("m{i}"));
        }
        let all = fake.fetch_recent(&channel(), 100).await.expect("fetch");
        assert_eq!(
            all.iter().map(|m| m.content.as_str()).collect::<Vec<_>>(),
            vec!["m0", "m1", "m2", "m3", "m4"]
        );
        let last_two = fake.fetch_recent(&channel(), 2).await.expect("fetch");
        assert_eq!(
            last_two
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>(),
            vec!["m3", "m4"],
            "a limited fetch must return the MOST RECENT messages, still oldest-first"
        );
    }

    #[tokio::test]
    async fn before_walks_backwards_and_after_walks_forwards_from_the_oldest() {
        // The asymmetry that is easy to get wrong. `before` yields the NEWEST messages older than
        // the cursor; `after` yields the OLDEST messages newer than it. A fake that took the
        // newest in both directions would certify a forward walk that skips whole spans against
        // live Discord.
        let fake = FakeDiscord::new();
        let mut ids = Vec::new();
        for i in 0..6 {
            ids.push(fake.seed(&channel(), "a", &format!("m{i}")));
        }
        let contents = |messages: &[Message]| {
            messages
                .iter()
                .map(|m| m.content.clone())
                .collect::<Vec<_>>()
        };

        let back = fake
            .fetch_page(&channel(), 2, Some(&ids[4]), None)
            .await
            .expect("fetch");
        assert_eq!(
            contents(&back),
            vec!["m2", "m3"],
            "before must return the newest messages OLDER than the cursor, oldest-first"
        );

        let forward = fake
            .fetch_page(&channel(), 2, None, Some(&ids[1]))
            .await
            .expect("fetch");
        assert_eq!(
            contents(&forward),
            vec!["m2", "m3"],
            "after must return the OLDEST messages newer than the cursor, not the newest ones"
        );

        assert!(
            fake.fetch_page(&channel(), 2, Some(&ids[0]), Some(&ids[1]))
                .await
                .is_err(),
            "both cursors at once must be refused here exactly as the real client refuses them"
        );
    }

    #[tokio::test]
    async fn a_seeded_instant_is_carried_by_the_id_as_well_as_the_timestamp() {
        let fake = FakeDiscord::new();
        let at = 1_787_233_885_123;
        let id = fake.seed_at(&channel(), "a", "on the hour", at);
        assert_eq!(id.created_at_ms(), Some(at));
        let messages = fake.fetch_recent(&channel(), 10).await.expect("fetch");
        assert_eq!(messages[0].id, id);
        assert_eq!(
            messages[0].timestamp,
            crate::clock::iso_from_ms(at),
            "the id and the timestamp must agree, or a time-range test proves nothing"
        );
    }

    #[tokio::test]
    async fn fetch_does_not_leak_other_channels() {
        let fake = FakeDiscord::new();
        fake.seed(&channel(), "a", "mine");
        fake.seed(&ChannelId("other".to_owned()), "a", "not mine");
        let messages = fake.fetch_recent(&channel(), 100).await.expect("fetch");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "mine");
    }

    #[tokio::test]
    async fn post_records_and_is_visible_to_a_later_fetch() {
        let fake = FakeDiscord::new();
        let target = fake.seed(&channel(), "a", "question");
        let posted = fake
            .post_message(&channel(), "answer", Some(&target))
            .await
            .expect("post");
        assert_eq!(posted.content, "answer");
        assert_eq!(
            fake.posted(),
            vec![PostedMessage {
                channel: channel(),
                content: "answer".to_owned(),
                reply_to: Some(target),
            }]
        );
        let messages = fake.fetch_recent(&channel(), 100).await.expect("fetch");
        assert_eq!(messages.last().expect("non-empty").content, "answer");
    }

    #[tokio::test]
    async fn the_fake_enforces_the_real_clients_refusals() {
        let fake = FakeDiscord::new();
        assert!(matches!(
            fake.post_message(&channel(), "", None).await,
            Err(DiscordError::Refused(_))
        ));
        assert!(
            fake.posted().is_empty(),
            "a refused post must not be recorded"
        );
    }

    #[tokio::test]
    async fn a_channel_this_fake_does_not_have_is_refused_the_way_discord_refuses_it() {
        // The load-bearing property: this fake can FAIL. If it answered an unknown channel with
        // an empty success, the startup probe would pass against it no matter what, and would be
        // evidence of nothing.
        let fake = FakeDiscord::new();
        let error = fake
            .fetch_recent(&ChannelId("nope".to_owned()), 1)
            .await
            .expect_err("a channel this fake was never given must not read as empty-but-fine");
        match error {
            DiscordError::Status { status, body } => {
                assert_eq!(status, 404);
                assert!(body.contains("10003"), "{body}");
            }
            other => panic!("expected Discord's own 404 shape, got {other:?}"),
        }
        assert!(
            fake.post_message(&ChannelId("nope".to_owned()), "hello", None)
                .await
                .is_err(),
            "posting to a channel this fake does not have must fail too"
        );
        // Registering it, with nothing in it, is the empty-but-real channel.
        fake.register_channel(&ChannelId("nope".to_owned()));
        assert!(fake
            .fetch_recent(&ChannelId("nope".to_owned()), 1)
            .await
            .expect("a registered channel reads")
            .is_empty());
    }

    #[tokio::test]
    async fn every_author_carries_a_distinct_snowflake_that_survives_a_fetch() {
        let fake = FakeDiscord::new();
        fake.seed(&channel(), "codex-eng", "one");
        fake.seed(&channel(), "coder-bot", "two");
        fake.seed(&channel(), "codex-eng", "three");
        let messages = fake.fetch_recent(&channel(), 100).await.expect("fetch");
        assert_eq!(
            messages[0].author_id, messages[2].author_id,
            "the same author must keep the same id"
        );
        assert_ne!(
            messages[0].author_id, messages[1].author_id,
            "two authors sharing one id would let a test pass while mentioning the wrong person"
        );
        assert!(
            messages[1].author_is_bot,
            "the bot author is still a bot; it just also has an id"
        );
        assert_eq!(
            fake.author_id("codex-eng"),
            Some(messages[0].author_id.clone())
        );
        assert_eq!(
            fake.author_id("nobody-here"),
            None,
            "this fake is not a user directory: it only knows who has spoken"
        );
        // Found by mutation: handing every author its own MESSAGE id kept this fake internally
        // consistent, so every test still passed while user ids and message ids shared one
        // namespace. A server that confused the two would then have been certified by the
        // fixture. The two spaces must not overlap.
        let message_ids: Vec<&str> = messages.iter().map(|m| m.id.as_str()).collect();
        for message in &messages {
            assert!(
                !message_ids.contains(&message.author_id.as_str()),
                "author id {} is also a message id: the fake's id spaces overlap",
                message.author_id
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_429_is_waited_out_and_the_read_succeeds_instead_of_failing() {
        // The gap this closes. Before, a 429 came back as `DiscordError::Status { status: 429 }`
        // and the API layer turned it into HTTP 502 — a question that would have been answered
        // half a second later instead failed.
        let fake = FakeDiscord::new();
        fake.seed(&channel(), "a", "hello");
        fake.rate_limit_next(2, Duration::from_millis(750), false);
        let started = tokio::time::Instant::now();
        let messages = fake
            .fetch_recent(&channel(), 10)
            .await
            .expect("a rate limit that clears must not surface as a failure");
        assert_eq!(messages.len(), 1);
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(1500),
            "it must have waited the 750ms Discord asked for, twice"
        );
        assert_eq!(fake.fetch_count(), 3, "one original and two retries");
        assert_eq!(fake.limiter().stats().retries, 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_posted_reply_survives_a_rate_limit_too() {
        let fake = FakeDiscord::new();
        let target = fake.seed(&channel(), "a", "question");
        fake.rate_limit_next(1, Duration::from_millis(400), false);
        let posted = fake
            .post_message(&channel(), "answer", Some(&target))
            .await
            .expect("the retry posts it");
        assert_eq!(posted.content, "answer");
        assert_eq!(
            fake.posted().len(),
            1,
            "a retried post must be posted ONCE, not once per attempt"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_exhausted_retry_budget_fails_loudly_and_names_the_rate_limit() {
        // The budget must not be a way of failing quietly: a caller that cannot tell quota from
        // an outage cannot do anything sensible about either.
        let fake = FakeDiscord::with_retry_policy(RetryPolicy {
            max_attempts: 2,
            max_total_wait: Duration::from_secs(30),
        });
        fake.seed(&channel(), "a", "hello");
        fake.rate_limit_next(5, Duration::from_millis(300), false);
        let error = fake
            .fetch_recent(&channel(), 10)
            .await
            .expect_err("a limit that never clears must not read as an empty channel");
        assert_eq!(fake.fetch_count(), 2, "bounded: two attempts, then stop");
        let message = error.to_string();
        assert!(
            message.contains("RATE LIMIT"),
            "the reason must name the rate limit: {message}"
        );
        match error {
            DiscordError::RateLimited(detail) => {
                assert_eq!(detail.attempts, 2);
                assert!(!detail.global);
                assert_eq!(detail.retry_after, Duration::from_millis(300));
            }
            other => panic!("expected a rate limit, got {other:?}"),
        }
    }

    /// A fake that never retries, so a rejection LEAVES its window standing for the next caller.
    fn no_retries() -> FakeDiscord {
        FakeDiscord::with_retry_policy(RetryPolicy {
            max_attempts: 1,
            max_total_wait: Duration::from_secs(30),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_global_limit_holds_back_a_channel_that_never_saw_it() {
        // Discord's global limit is on the TOKEN. Filing it under the channel that observed it
        // would let the very next channel spend a request walking into the identical wall.
        let other = ChannelId("c2".to_owned());
        let fake = no_retries();
        fake.seed(&channel(), "a", "one");
        fake.seed(&other, "a", "two");

        fake.rate_limit_next(1, Duration::from_secs(5), true);
        fake.fetch_recent(&channel(), 10)
            .await
            .expect_err("one attempt, so this one gives up");
        let spent = fake.fetch_count();

        let started = tokio::time::Instant::now();
        fake.fetch_recent(&other, 10)
            .await
            .expect("the other channel reads once the global window passes");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(5),
            "a GLOBAL limit is the whole token: every channel waits behind it"
        );
        assert_eq!(
            fake.fetch_count(),
            spent + 1,
            "and the doomed request was held rather than sent and rejected"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_per_route_limit_holds_back_only_its_own_channel() {
        // The contrast that makes the test above mean something: treating every 429 as global
        // would stall the whole server on one busy channel.
        let other = ChannelId("c2".to_owned());
        let fake = no_retries();
        fake.seed(&channel(), "a", "one");
        fake.seed(&other, "a", "two");

        fake.rate_limit_next(1, Duration::from_secs(5), false);
        fake.fetch_recent(&channel(), 10)
            .await
            .expect_err("one attempt, so this one gives up");

        let started = tokio::time::Instant::now();
        fake.fetch_recent(&other, 10)
            .await
            .expect("a different channel is a different bucket");
        assert_eq!(
            started.elapsed(),
            Duration::ZERO,
            "one channel's bucket must not stall every other channel"
        );

        // ...and the channel that WAS limited is held back rather than sent again.
        let held = tokio::time::Instant::now();
        fake.fetch_recent(&channel(), 10)
            .await
            .expect("reads once its own window passes");
        assert_eq!(held.elapsed(), Duration::from_secs(5));
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_bucket_is_waited_out_rather_than_spent_on_a_certain_rejection() {
        let fake = FakeDiscord::new();
        fake.seed(&channel(), "a", "hello");
        // Discord's accounting on a SUCCESSFUL answer: no requests left, refilling in two seconds.
        fake.set_bucket(0, Duration::from_secs(2));
        fake.fetch_recent(&channel(), 10).await.expect("reads");
        let after_first = fake.fetch_count();

        let started = tokio::time::Instant::now();
        fake.fetch_recent(&channel(), 10).await.expect("reads");
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(2),
            "the next request was known to be doomed and must be held, not sent"
        );
        assert_eq!(
            fake.fetch_count(),
            after_first + 1,
            "held back, then sent exactly once"
        );
        assert_eq!(fake.limiter().stats().preempted, 1);
    }

    #[tokio::test]
    async fn injected_failures_surface() {
        let fake = FakeDiscord::new();
        fake.register_channel(&channel());
        fake.fail_next("network down");
        assert!(matches!(
            fake.fetch_recent(&channel(), 10).await,
            Err(DiscordError::Transport(_))
        ));
        // The failure is one-shot.
        assert!(fake.fetch_recent(&channel(), 10).await.is_ok());
    }
}
