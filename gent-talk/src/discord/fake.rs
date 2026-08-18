//! An in-memory Discord stand-in for tests and for local development.
//!
//! It is not a mock that agrees with whatever it is asked. It shares the real client's request
//! validation ([`super::http::post_request`]) and the real client's ordering contract, so a test
//! written against it exercises the same refusals and the same oldest-first normalization that
//! production code takes. The binary never constructs one unless `--fake-discord` is passed, and
//! that flag logs a warning on every startup.

use std::sync::Mutex;

use async_trait::async_trait;

use super::{DiscordClient, DiscordError};
use crate::config::DEFAULT_DISCORD_API_BASE;
use crate::model::{sort_oldest_first, ChannelId, Message, MessageId};

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
    messages: Vec<Message>,
    posted: Vec<PostedMessage>,
    next_id: u64,
    fail_with: Option<String>,
}

impl FakeDiscord {
    /// An empty fake.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a message into a channel. Returns its assigned snowflake.
    pub fn seed(&self, channel: &ChannelId, author: &str, content: &str) -> MessageId {
        let mut state = self.lock();
        state.next_id += 1;
        let seq = state.next_id;
        // Start well above 0 so ids are snowflake-shaped and exercise numeric ordering.
        let id = MessageId(format!("{}", 1_000_000_000_000_000_000_u64 + seq));
        state.messages.push(Message {
            id: id.clone(),
            channel_id: channel.clone(),
            author: author.to_owned(),
            author_is_bot: author.ends_with("-bot"),
            timestamp: format!("2026-08-18T12:{:02}:00+00:00", seq % 60),
            content: content.to_owned(),
        });
        id
    }

    /// Make the next operation fail, so error handling can be tested.
    pub fn fail_next(&self, detail: &str) {
        self.lock().fail_with = Some(detail.to_owned());
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
}

#[async_trait]
impl DiscordClient for FakeDiscord {
    async fn fetch_recent(
        &self,
        channel: &ChannelId,
        limit: u16,
    ) -> Result<Vec<Message>, DiscordError> {
        if let Some(failure) = self.take_failure() {
            return Err(failure);
        }
        let state = self.lock();
        let mut messages: Vec<Message> = state
            .messages
            .iter()
            .filter(|m| &m.channel_id == channel)
            .cloned()
            .collect();
        sort_oldest_first(&mut messages);
        let limit = usize::from(limit.clamp(1, super::http::DISCORD_MAX_LIMIT));
        if messages.len() > limit {
            messages.drain(..messages.len() - limit);
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
        let _ = super::http::post_request(DEFAULT_DISCORD_API_BASE, channel, content, reply_to)?;
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
    async fn injected_failures_surface() {
        let fake = FakeDiscord::new();
        fake.fail_next("network down");
        assert!(matches!(
            fake.fetch_recent(&channel(), 10).await,
            Err(DiscordError::Transport(_))
        ));
        // The failure is one-shot.
        assert!(fake.fetch_recent(&channel(), 10).await.is_ok());
    }
}
