//! The store a server runs with when no storage is configured: one that refuses.
//!
//! There are three things a server could do when `storage.path` is unset, and two of them are
//! wrong. It could keep state in memory, which looks like it works and loses everything on the
//! next restart — the worst outcome, because the loss is invisible until it matters. It could
//! pick a path itself, which puts the owner's speech somewhere he did not choose, quite possibly
//! inside a container image that is rebuilt weekly. Or it can refuse, loudly and specifically,
//! naming the setting to add.
//!
//! It refuses. That is the same posture [`crate::elevenlabs::credentials`] takes toward a missing
//! API key, for the same reason: an unconfigured feature must be distinguishable from a broken
//! one.

use async_trait::async_trait;

use super::{
    ConversationId, ConversationSummary, ReadMark, StateStore, StoreError, SummaryKey, Turn,
};
use crate::model::{ChannelId, MessageId};

/// The setting whose absence installs [`DisabledStore`].
pub const SETTING: &str = "storage.path";

/// A store that holds nothing and says so.
#[derive(Clone, Copy, Debug, Default)]
pub struct DisabledStore;

fn refuse<T>() -> Result<T, StoreError> {
    Err(StoreError::Unavailable(SETTING))
}

#[async_trait]
impl StateStore for DisabledStore {
    fn describe(&self) -> String {
        format!("no durable state ({SETTING} is not configured)")
    }

    async fn append_turn(&self, _: &ConversationId, _: &Turn) -> Result<(), StoreError> {
        refuse()
    }

    async fn conversations(&self) -> Result<Vec<ConversationSummary>, StoreError> {
        refuse()
    }

    async fn turns(&self, _: &ConversationId) -> Result<Vec<Turn>, StoreError> {
        refuse()
    }

    async fn forget_conversation(&self, _: &ConversationId) -> Result<(), StoreError> {
        refuse()
    }

    async fn forget_all_conversations(&self) -> Result<u64, StoreError> {
        refuse()
    }

    async fn read_mark(&self, _: &ChannelId) -> Result<Option<ReadMark>, StoreError> {
        refuse()
    }

    async fn read_marks(&self) -> Result<Vec<ReadMark>, StoreError> {
        refuse()
    }

    async fn mark_read(&self, _: &ChannelId, _: &MessageId) -> Result<ReadMark, StoreError> {
        refuse()
    }

    async fn forget_read_mark(&self, _: &ChannelId) -> Result<(), StoreError> {
        refuse()
    }

    async fn dismissals(&self, _: &ChannelId) -> Result<Vec<MessageId>, StoreError> {
        refuse()
    }

    async fn dismiss(&self, _: &ChannelId, _: &[MessageId]) -> Result<u64, StoreError> {
        refuse()
    }

    async fn restore(&self, _: &ChannelId, _: &[MessageId]) -> Result<u64, StoreError> {
        refuse()
    }

    async fn cached_summary(&self, _: &SummaryKey) -> Result<Option<String>, StoreError> {
        refuse()
    }

    async fn cache_summary(&self, _: &SummaryKey, _: &str) -> Result<(), StoreError> {
        refuse()
    }

    async fn forget_summaries_except(&self, _: &str) -> Result<u64, StoreError> {
        refuse()
    }

    async fn purge_everything(&self) -> Result<(), StoreError> {
        refuse()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Speaker;
    use super::*;

    #[tokio::test]
    async fn every_operation_refuses_and_names_the_setting_to_add() {
        let store = DisabledStore;
        let id = ConversationId::parse("conv").expect("valid");
        let channel = ChannelId("1111111111".to_owned());
        let message = MessageId("1000000000000000200".to_owned());

        let errors: Vec<StoreError> = vec![
            store
                .append_turn(&id, &Turn::now(Speaker::You, "hi"))
                .await
                .expect_err("append"),
            store.conversations().await.expect_err("list"),
            store.turns(&id).await.expect_err("turns"),
            store.forget_conversation(&id).await.expect_err("forget"),
            store
                .forget_all_conversations()
                .await
                .expect_err("forget all"),
            store.read_mark(&channel).await.expect_err("mark"),
            store.read_marks().await.expect_err("marks"),
            store
                .mark_read(&channel, &message)
                .await
                .expect_err("mark read"),
            store.forget_read_mark(&channel).await.expect_err("unmark"),
            store.dismissals(&channel).await.expect_err("dismissals"),
            store
                .dismiss(&channel, std::slice::from_ref(&message))
                .await
                .expect_err("dismiss"),
            store
                .restore(&channel, std::slice::from_ref(&message))
                .await
                .expect_err("restore"),
            store.purge_everything().await.expect_err("purge"),
        ];
        for error in errors {
            assert_eq!(error.code(), "storage_not_configured", "{error}");
            assert!(
                error.to_string().contains(SETTING),
                "the operator must be told which setting to add: {error}"
            );
        }
    }
}
