//! The replies of a thread the server's own copy already holds whole, for a view that lists every
//! thread's replies. `#603 server-side-search` (Phase 0 of `#521 server-side-search`).
//!
//! Slack's history lists top-level messages only, and a thread's replies cost a call of their own,
//! so All once read the replies of at most ten threads a page and left the rest out. All now
//! brings the replies of every thread rooted on the page. Where the message cache already holds a
//! thread whole, its copy stands in for that call: the root the page has just read upstream says
//! how many replies the thread has and which is the latest, and a copy with exactly that many
//! replies, ending on exactly that one, is the thread as the root describes it. It is the same
//! test the provider's own memory of a thread's replies passes before it is reused at no call.
//!
//! What may hand those replies out is the cache's business, and the rules are the serving rules
//! (see [`crate::cache::Sink`]'s implementation). Which reads may ask is the caller's: the source
//! is carried in a task-local, set by [`scope`] around a read only for a caller that may be
//! answered from the cache at all (the write scope, with serving on). With none set, every thread
//! is read upstream, as before.
//!
//! Replies taken from the cache are named on the page ([`crate::threads::TimelinePage::held`]) so
//! that the read is never offered back to the cache as having seen them: it did not.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use crate::model::{ChannelId, Message, MessageId};

/// What a thread's root, just read upstream, says of the thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootSays {
    /// The thread, as messages name it (`MessageThread::id`).
    pub thread_id: String,
    /// The root message.
    pub root_id: MessageId,
    /// When the root was written, provider time in milliseconds: where the thread begins.
    pub root_ms: i64,
    /// The replies the root counts.
    pub reply_count: u64,
    /// The latest reply the root names.
    pub latest_reply: MessageId,
}

/// Where whole threads come from.
#[async_trait]
pub trait HeldReplies: Send + Sync {
    /// Every reply of the thread `root` describes, oldest first, the root itself left out; `None`
    /// unless the copy held is that thread exactly and may be handed out now.
    async fn whole(&self, channel: &ChannelId, root: &RootSays) -> Option<Vec<Message>>;
}

tokio::task_local! {
    static CURRENT: Option<Arc<dyn HeldReplies>>;
}

/// Run `work` with `source` as where its reads may take whole threads from; `None` takes none.
pub async fn scope<F: Future>(source: Option<Arc<dyn HeldReplies>>, work: F) -> F::Output {
    CURRENT.scope(source, work).await
}

/// The source the read under way may take whole threads from, if any.
#[must_use]
pub fn current() -> Option<Arc<dyn HeldReplies>> {
    CURRENT.try_with(Clone::clone).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Never;

    #[async_trait]
    impl HeldReplies for Never {
        async fn whole(&self, _: &ChannelId, _: &RootSays) -> Option<Vec<Message>> {
            None
        }
    }

    #[tokio::test]
    async fn a_source_is_seen_only_inside_its_scope() {
        assert!(current().is_none());
        let seen = scope(Some(Arc::new(Never)), async { current().is_some() }).await;
        assert!(seen);
        assert!(scope(None, async { current().is_none() }).await);
        assert!(current().is_none());
    }
}
