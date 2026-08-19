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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use async_trait::async_trait;

use super::{DiscordClient, DiscordError};
use crate::config::DEFAULT_DISCORD_API_BASE;
use crate::model::{sort_oldest_first, ChannelId, Message, MessageId, UserId};

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
}

impl FakeDiscord {
    /// An empty fake.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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
            content: content.to_owned(),
        });
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
    async fn fetch_page(
        &self,
        channel: &ChannelId,
        limit: u16,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
    ) -> Result<Vec<Message>, DiscordError> {
        self.lock().fetches += 1;
        if let Some(failure) = self.take_failure() {
            return Err(failure);
        }
        // Share the real client's refusal, so a caller cannot get away here with a request
        // Discord would not define.
        let _ = super::http::page_request(DEFAULT_DISCORD_API_BASE, channel, limit, before, after)?;
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
                    // An unparseable id cannot be placed relative to a cursor, so it is only ever
                    // returned by an uncursored fetch. Guessing would fabricate an ordering.
                    return before.is_none() && after.is_none();
                };
                before.is_none_or(|b| id < b) && after.is_none_or(|a| id > a)
            })
            .cloned()
            .collect();
        sort_oldest_first(&mut messages);
        let limit = usize::from(limit.clamp(1, super::http::DISCORD_MAX_LIMIT));
        if messages.len() > limit {
            if after.is_some() {
                // `after` walks FORWARD: Discord answers with the OLDEST messages following the
                // cursor, not the newest ones. Taking the newest here would make a forward walk
                // skip everything in between and still look correct in a test.
                messages.truncate(limit);
            } else {
                messages.drain(..messages.len() - limit);
            }
        }
        Ok(messages)
    }

    async fn post_message(
        &self,
        channel: &ChannelId,
        content: &str,
        reply_to: Option<&MessageId>,
    ) -> Result<Message, DiscordError> {
        if let Some(failure) = self.take_failure() {
            return Err(failure);
        }
        // Share the real client's validation so a test cannot pass on input Discord would reject.
        // Validation first, then existence, because Discord also rejects an empty body before it
        // ever looks the channel up.
        let _ = super::http::post_request(DEFAULT_DISCORD_API_BASE, channel, content, reply_to)?;
        if let Some(unknown) = self.unknown_channel(channel) {
            return Err(unknown);
        }
        let id = self.seed(channel, "gent-talk", content);
        self.lock().posted.push(PostedMessage {
            channel: channel.clone(),
            content: content.to_owned(),
            reply_to: reply_to.cloned(),
        });
        let state = self.lock();
        state
            .messages
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| DiscordError::Shape("posted message vanished".to_owned()))
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
