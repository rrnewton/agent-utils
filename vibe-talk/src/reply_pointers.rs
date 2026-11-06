//! Which message each reply this server posted answers. `#934 voice-page-reply-parent-arrow`.
//!
//! The owner: "When I reply or quote reply especially inside a thread. I'm not getting a little blue
//! arrows to blink it back to the parent. ... Even if reply is not a concept in slack, I thought that
//! we were tracking it in our own metadata so that vibe talk could render them."
//!
//! The page draws a reply's arrow from [`Message::reply_to`] (`#204 reply-arrow`). Discord carries
//! that pointer itself; Slack has none — a Slack reply is thread membership and nothing more — so a
//! reply posted there came back from every read with `reply_to` empty, in the channel and in a
//! thread alike, and no arrow was ever drawn for it.
//!
//! So this server keeps its own pointer for every reply it posts, whatever the provider: the FIRST
//! part's id against the message the reply answers, filed in the store as it is posted
//! ([`crate::store::StateStore::keep_reply_pointer`]) and read back at start
//! ([`ReplyPointers::restore`]), and put on every message served without one of its own
//! ([`ReplyPointers::fill`], from [`crate::ops::stamp`]). A provider's own pointer is never
//! overwritten. Ids only, no words: kept for [`MAX_KEPT`] replies, the oldest forgotten first.
//!
//! Keyed by the message's id alone. A Discord id is unique everywhere; a Slack id is its timestamp
//! to the microsecond within its conversation, and the same message is read under the channel and
//! under a channel narrowed to its thread, which the key must not tell apart.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, PoisonError};

use crate::model::{Message, MessageId};

/// The most replies remembered at once; the oldest is forgotten first.
pub const MAX_KEPT: usize = 50_000;

/// The pointers, in memory, in the order they were noted.
#[derive(Debug, Default)]
pub struct ReplyPointers {
    inner: Mutex<Kept>,
}

#[derive(Debug, Default)]
struct Kept {
    parents: HashMap<String, String>,
    order: VecDeque<String>,
}

impl ReplyPointers {
    /// Nothing remembered.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Kept> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Remember that `message` answers `parent`. A message noted again keeps its first parent's
    /// place in the order and takes the new parent.
    pub fn note(&self, message: &MessageId, parent: &MessageId) {
        let mut kept = self.lock();
        if kept
            .parents
            .insert(message.0.clone(), parent.0.clone())
            .is_none()
        {
            kept.order.push_back(message.0.clone());
        }
        while kept.order.len() > MAX_KEPT {
            if let Some(oldest) = kept.order.pop_front() {
                kept.parents.remove(&oldest);
            }
        }
    }

    /// Remember every `(message, parent)` the store kept, oldest first.
    pub fn restore(&self, pointers: impl IntoIterator<Item = (MessageId, MessageId)>) {
        for (message, parent) in pointers {
            self.note(&message, &parent);
        }
    }

    /// The message `message` answers, where this server posted it as a reply.
    #[must_use]
    pub fn parent(&self, message: &MessageId) -> Option<MessageId> {
        self.lock().parents.get(&message.0).cloned().map(MessageId)
    }

    /// Put each message's pointer on it where it carries none of its own.
    pub fn fill(&self, messages: &mut [Message]) {
        let kept = self.lock();
        if kept.parents.is_empty() {
            return;
        }
        for message in messages {
            if message.reply_to.is_none() {
                message.reply_to = kept.parents.get(&message.id.0).cloned().map(MessageId);
            }
        }
    }

    /// Forget every pointer: the store's purge took them.
    pub fn forget(&self) {
        let mut kept = self.lock();
        kept.parents.clear();
        kept.order.clear();
    }

    /// How many replies are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().parents.len()
    }

    /// Whether none is.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().parents.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mid(id: &str) -> MessageId {
        MessageId(id.to_owned())
    }

    #[test]
    fn a_noted_reply_is_filled_and_a_providers_own_pointer_is_kept() {
        let pointers = ReplyPointers::new();
        pointers.note(&mid("r1"), &mid("p1"));
        let mut own = crate::cache::fixtures::message("r1", "ada", "a reply", 1);
        let mut native = crate::cache::fixtures::message("r2", "ada", "a native reply", 2);
        native.reply_to = Some(mid("p2"));
        let mut other = crate::cache::fixtures::message("m3", "ada", "not a reply", 3);
        let mut messages = [own.clone(), native.clone(), other.clone()];
        pointers.fill(&mut messages);
        own.reply_to = Some(mid("p1"));
        assert_eq!(
            messages[0].reply_to, own.reply_to,
            "our own pointer was not put on the reply"
        );
        assert_eq!(
            messages[1].reply_to, native.reply_to,
            "a provider's own pointer was overwritten"
        );
        other.reply_to = None;
        assert_eq!(
            messages[2].reply_to, other.reply_to,
            "a message that answers nothing was given a parent"
        );
        assert_eq!(pointers.parent(&mid("r1")), Some(mid("p1")));
    }

    #[test]
    fn the_oldest_are_forgotten_past_the_bound_and_restore_keeps_order() {
        let pointers = ReplyPointers::new();
        pointers.restore((0..=MAX_KEPT).map(|i| (mid(&format!("r{i}")), mid(&format!("p{i}")))));
        assert_eq!(pointers.len(), MAX_KEPT);
        assert_eq!(
            pointers.parent(&mid("r0")),
            None,
            "the oldest was not forgotten first"
        );
        assert_eq!(
            pointers.parent(&mid(&format!("r{MAX_KEPT}"))),
            Some(mid(&format!("p{MAX_KEPT}")))
        );
        pointers.forget();
        assert!(pointers.is_empty());
    }
}
