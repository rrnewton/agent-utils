//! Keeping the durable store's summaries in step with what the message cache learns. `#208
//! persistent-cache`.
//!
//! The store keeps summaries of messages, keyed by the text they summarise, and the cache never
//! holds one. But the cache is where the server learns that a message was deleted or edited
//! upstream, or that a channel was erased, and a summary of words a message no longer says is a
//! copy of them at rest that nothing can reach by its key any more. So the cache's writer tells
//! this task ([`WriterEvent`]) once each such change has committed, and the task deletes:
//!
//! - for a message deleted upstream, every summary of it filed under the policy in force;
//! - for a message whose text changed, every summary of it filed under the policy in force that is
//!   of other text;
//! - for an erased channel, its summaries and thread sentences, as taking a channel off the list
//!   does, unless the same channel id is still on the list under another provider. The store keys
//!   summaries by channel id alone, so once a provider key is renamed, the channel the cache erases
//!   under the old key is the very one still listed under the new.
//!
//! None of this runs while the cache is off: with no cache installed nothing is told, and turning
//! the cache off never deletes a summary.
//!
//! Every write here is a deletion, and every event comes from an observation made under a
//! write-scope or server grant ([`super::grant`]): a read-scope request never causes one.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};

use tokio::sync::mpsc::Receiver;

use super::WriterEvent;
use crate::model::ChannelId;
use crate::store::{StateStore, StoreError};
use crate::summarize::threads::ThreadNames;

/// How many events the writer may have sent that this task has not yet taken. Past it, the writer
/// drops what does not fit and counts it ([`super::CacheStats::orphans_dropped`]); the store's own
/// retention then collects what was not deleted here.
pub const EVENT_QUEUE: usize = 1_024;

/// Where the task deletes: the store, under the summary policy in force, and the thread sentences
/// the server remembers in memory.
#[derive(Clone)]
pub struct Summaries {
    store: Arc<dyn StateStore>,
    version: Arc<str>,
    thread_names: Arc<ThreadNames>,
}

impl std::fmt::Debug for Summaries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Summaries")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Summaries {
    /// The server's store, its summary policy, and the thread sentences it remembers.
    #[must_use]
    pub fn of(state: &crate::state::AppState) -> Self {
        Self {
            store: Arc::clone(&state.store),
            version: Arc::clone(&state.summary_version),
            thread_names: Arc::clone(&state.thread_names),
        }
    }

    /// Delete what one event says is stale.
    async fn follow(&self, event: &WriterEvent, listed: &RwLock<HashSet<ChannelId>>) {
        match event {
            WriterEvent::SummaryOrphan {
                channel_id,
                message_id,
                live_hash,
            } => {
                let forgotten = self
                    .store
                    .forget_summaries_for(&self.version, channel_id, message_id, *live_hash)
                    .await;
                match forgotten {
                    Ok(_) | Err(StoreError::Unavailable(_)) => {}
                    Err(error) => tracing::warn!(
                        %error,
                        "a cached summary of an edited or deleted message was kept"
                    ),
                }
            }
            WriterEvent::ChannelErased { channel_id } => {
                let still_listed = listed
                    .read()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(channel_id);
                if !still_listed {
                    crate::ops::forget_channel_summaries(
                        self.store.as_ref(),
                        &self.thread_names,
                        channel_id,
                    )
                    .await;
                }
            }
        }
    }
}

/// The task: follow every event the cache's writer sends, until the cache that sends them is gone.
/// `listed` is every channel id on the server's list, whatever provider it is under; `handled`
/// counts the events followed.
pub(crate) async fn run(
    mut events: Receiver<WriterEvent>,
    summaries: Summaries,
    listed: Arc<RwLock<HashSet<ChannelId>>>,
    handled: Arc<AtomicU64>,
) {
    while let Some(event) = events.recv().await {
        summaries.follow(&event, &listed).await;
        handled.fetch_add(1, Ordering::SeqCst);
    }
}
