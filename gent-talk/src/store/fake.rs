//! An in-memory [`StateStore`] for tests.
//!
//! # It must be able to say no
//!
//! A fake that accepts every write and answers every read with a success is a yes-man: a test
//! written against it is self-certifying, green here and worth nothing about the shipped
//! [`super::sqlite::SqliteStore`]. So this one reproduces the refusals rather than the happy path
//! alone —
//!
//! * a conversation that was never written answers [`StoreError::NotFound`], and so does
//!   forgetting one twice;
//! * the same [`Retention`] bounds apply, including the per-conversation turn ceiling, the
//!   [`super::MAX_TURN_CHARS`] limit on one turn, and the count and age bounds on the cached
//!   summaries;
//! * a read mark refuses an id it cannot order, and never moves backwards;
//! * [`FakeStore::fail_next`] makes the very next operation fail, so a caller's error path can be
//!   exercised instead of assumed.
//!
//! What it deliberately cannot show is durability: it is destroyed with the process. The test
//! that a transcript survives a restart lives in [`super::sqlite`] against a real file, and it
//! has to.

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::{
    now_ms, ConversationId, ConversationSummary, ReadMark, Retention, StateStore, StoreError,
    SummaryKey, Turn, MAX_TURN_CHARS,
};
use crate::model::{ChannelId, MessageId};

/// One cached summary as the fake holds it.
///
/// `made_at_ms` and `seq` are not decoration: the real store bounds this table by age and by
/// count, and a fake that kept only the text could not reproduce either — which is exactly how a
/// fake stops being able to say no.
#[derive(Clone, Debug)]
struct FakeSummary {
    text: String,
    made_at_ms: i64,
    /// Insertion order, standing in for SQLite's `rowid`: it breaks a tie between two entries
    /// made in the same millisecond, and it survives an overwrite, as a rowid does.
    seq: u64,
}

#[derive(Debug, Default)]
struct State {
    conversations: BTreeMap<String, Vec<Turn>>,
    marks: BTreeMap<String, (MessageId, u64, i64)>,
    summaries: BTreeMap<(String, String, String, String), FakeSummary>,
    next_summary_seq: u64,
    fail_next: Option<String>,
    appended: usize,
    purges: usize,
}

/// In-memory durable state, for tests.
#[derive(Debug)]
pub struct FakeStore {
    state: Mutex<State>,
    retention: Retention,
}

impl Default for FakeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeStore {
    /// An empty store with the default retention bounds.
    #[must_use]
    pub fn new() -> Self {
        Self::with_retention(Retention::default())
    }

    /// An empty store with retention bounds a test chose.
    #[must_use]
    pub fn with_retention(retention: Retention) -> Self {
        Self {
            state: Mutex::new(State::default()),
            retention,
        }
    }

    /// Make the next operation — any operation — fail with a backend error saying `why`.
    ///
    /// One-shot, so a test can prove the *recovery* as well as the failure.
    pub fn fail_next(&self, why: &str) {
        self.lock().fail_next = Some(why.to_owned());
    }

    /// How many turns have been appended, refusals excluded.
    #[must_use]
    pub fn appended(&self) -> usize {
        self.lock().appended
    }

    /// How many times the store has been purged.
    #[must_use]
    pub fn purges(&self) -> usize {
        self.lock().purges
    }

    /// How many cached summaries are held right now.
    ///
    /// A count rather than the entries themselves: what tests need to know is whether a caller
    /// caused a durable write, and "how many rows are there" is the question that answers it
    /// without a test having to reconstruct the four-part key.
    #[must_use]
    pub fn cached_summaries(&self) -> usize {
        self.lock().summaries.len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The same four-part key the real store uses, so the fake cannot be more forgiving than it is.
fn fake_key(key: &SummaryKey) -> (String, String, String, String) {
    (
        key.version.clone(),
        key.channel.as_str().to_owned(),
        key.message.as_str().to_owned(),
        format!("{:016x}", key.content_hash),
    )
}

fn armed(state: &mut State) -> Result<(), StoreError> {
    match state.fail_next.take() {
        Some(why) => Err(StoreError::Backend(why)),
        None => Ok(()),
    }
}

#[async_trait]
impl StateStore for FakeStore {
    fn describe(&self) -> String {
        "an in-memory store that is destroyed with the process".to_owned()
    }

    async fn append_turn(
        &self,
        conversation: &ConversationId,
        turn: &Turn,
    ) -> Result<(), StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        if turn.text.chars().count() > MAX_TURN_CHARS {
            return Err(StoreError::TooLarge(format!(
                "one turn may be at most {MAX_TURN_CHARS} characters"
            )));
        }
        let held = state
            .conversations
            .get(conversation.as_str())
            .map_or(0, Vec::len);
        if held >= usize::from(self.retention.max_turns_per_conversation) {
            return Err(StoreError::TooLarge(format!(
                "this conversation already holds its limit of {} turns",
                self.retention.max_turns_per_conversation
            )));
        }
        state
            .conversations
            .entry(conversation.as_str().to_owned())
            .or_default()
            .push(turn.clone());
        state.appended += 1;

        if self.retention.retain_days > 0 {
            let cutoff = now_ms() - i64::from(self.retention.retain_days) * 86_400_000;
            state
                .conversations
                .retain(|_, turns| turns.last().is_some_and(|t| t.at_ms >= cutoff));
        }
        while state.conversations.len() > usize::from(self.retention.max_conversations) {
            let oldest = state
                .conversations
                .iter()
                .min_by_key(|(id, turns)| (turns.last().map_or(0, |t| t.at_ms), (*id).clone()))
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    state.conversations.remove(&id);
                }
                None => break,
            }
        }
        Ok(())
    }

    async fn conversations(&self) -> Result<Vec<ConversationSummary>, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        let mut out: Vec<ConversationSummary> = state
            .conversations
            .iter()
            .map(|(id, turns)| ConversationSummary {
                id: ConversationId::parse(id).unwrap_or_else(|_| unreachable!("stored id")),
                started_at_ms: turns.first().map_or(0, |t| t.at_ms),
                last_at_ms: turns.last().map_or(0, |t| t.at_ms),
                turns: u32::try_from(turns.len()).unwrap_or(u32::MAX),
                preview: turns
                    .first()
                    .map_or_else(String::new, |t| super::preview_of(&t.text)),
            })
            .collect();
        out.sort_by(|a, b| {
            b.last_at_ms
                .cmp(&a.last_at_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(out)
    }

    async fn turns(&self, conversation: &ConversationId) -> Result<Vec<Turn>, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        state
            .conversations
            .get(conversation.as_str())
            .cloned()
            .ok_or(StoreError::NotFound)
    }

    async fn forget_conversation(&self, conversation: &ConversationId) -> Result<(), StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        state
            .conversations
            .remove(conversation.as_str())
            .map(|_| ())
            .ok_or(StoreError::NotFound)
    }

    async fn forget_all_conversations(&self) -> Result<u64, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        let removed = state.conversations.len() as u64;
        state.conversations.clear();
        Ok(removed)
    }

    async fn read_mark(&self, channel: &ChannelId) -> Result<Option<ReadMark>, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        Ok(state
            .marks
            .get(channel.as_str())
            .map(|(last_read, _, at)| ReadMark {
                channel: channel.clone(),
                last_read: last_read.clone(),
                marked_at_ms: *at,
            }))
    }

    async fn read_marks(&self) -> Result<Vec<ReadMark>, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        Ok(state
            .marks
            .iter()
            .map(|(channel, (last_read, _, at))| ReadMark {
                channel: ChannelId(channel.clone()),
                last_read: last_read.clone(),
                marked_at_ms: *at,
            })
            .collect())
    }

    async fn mark_read(
        &self,
        channel: &ChannelId,
        upto: &MessageId,
    ) -> Result<ReadMark, StoreError> {
        let numeric = upto.numeric().ok_or_else(|| {
            StoreError::BadId(format!(
                "{upto:?} is not a message snowflake, so this server cannot order it"
            ))
        })?;
        let mut state = self.lock();
        armed(&mut state)?;
        let at = now_ms();
        let entry = state
            .marks
            .entry(channel.as_str().to_owned())
            .or_insert_with(|| (upto.clone(), numeric, at));
        if numeric > entry.1 {
            *entry = (upto.clone(), numeric, at);
        }
        Ok(ReadMark {
            channel: channel.clone(),
            last_read: entry.0.clone(),
            marked_at_ms: entry.2,
        })
    }

    async fn forget_read_mark(&self, channel: &ChannelId) -> Result<(), StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        state
            .marks
            .remove(channel.as_str())
            .map(|_| ())
            .ok_or(StoreError::NotFound)
    }

    async fn cached_summary(&self, key: &SummaryKey) -> Result<Option<String>, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        Ok(state
            .summaries
            .get(&fake_key(key))
            .map(|entry| entry.text.clone()))
    }

    async fn cache_summary(&self, key: &SummaryKey, summary: &str) -> Result<(), StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        let slot = fake_key(key);
        let seq = match state.summaries.get(&slot) {
            // An overwrite keeps its place in the queue, as it keeps its rowid in SQLite.
            Some(existing) => existing.seq,
            None => {
                let next = state.next_summary_seq;
                state.next_summary_seq += 1;
                next
            }
        };
        state.summaries.insert(
            slot,
            FakeSummary {
                text: summary.to_owned(),
                made_at_ms: now_ms(),
                seq,
            },
        );

        // The same two bounds the SQLite store applies, in the same order. See
        // `super::sqlite::prune_summaries` for why the age one is the only thing that ever
        // collects an orphan.
        if self.retention.retain_days > 0 {
            let cutoff = now_ms() - i64::from(self.retention.retain_days) * 86_400_000;
            state.summaries.retain(|_, held| held.made_at_ms >= cutoff);
        }
        while state.summaries.len() > self.retention.max_summaries as usize {
            let oldest = state
                .summaries
                .iter()
                .min_by_key(|(_, held)| (held.made_at_ms, held.seq))
                .map(|(slot, _)| slot.clone());
            match oldest {
                Some(slot) => {
                    state.summaries.remove(&slot);
                }
                None => break,
            }
        }
        Ok(())
    }

    async fn forget_summaries_except(&self, version: &str) -> Result<u64, StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        let before = state.summaries.len();
        state.summaries.retain(|(held, _, _, _), _| held == version);
        Ok((before - state.summaries.len()) as u64)
    }

    async fn purge_everything(&self) -> Result<(), StoreError> {
        let mut state = self.lock();
        armed(&mut state)?;
        state.conversations.clear();
        state.marks.clear();
        state.summaries.clear();
        state.purges += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Speaker;
    use super::*;

    fn id(text: &str) -> ConversationId {
        ConversationId::parse(text).expect("valid id")
    }

    #[tokio::test]
    async fn the_fake_refuses_a_conversation_it_was_never_given() {
        let store = FakeStore::new();
        assert!(matches!(
            store.turns(&id("nope")).await,
            Err(StoreError::NotFound)
        ));
        assert!(matches!(
            store.forget_conversation(&id("nope")).await,
            Err(StoreError::NotFound)
        ));
    }

    #[tokio::test]
    async fn the_fake_can_be_made_to_fail_exactly_once() {
        let store = FakeStore::new();
        store.fail_next("the disk is full");
        let error = store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
            .await
            .expect_err("the armed failure must fire");
        assert_eq!(error.code(), "storage_error", "{error}");
        assert_eq!(store.appended(), 0, "a failed append must write nothing");
        store
            .append_turn(&id("conv"), &Turn::now(Speaker::You, "hello"))
            .await
            .expect("the failure is one-shot, so recovery is testable");
        assert_eq!(store.appended(), 1);
    }

    #[tokio::test]
    async fn the_fake_enforces_the_same_retention_bounds_as_the_real_store() {
        let store = FakeStore::with_retention(Retention {
            max_conversations: 2,
            max_turns_per_conversation: 2,
            retain_days: 0,
            ..Retention::default()
        });
        for (n, at) in [("a", 1_000), ("b", 2_000), ("c", 3_000)] {
            store
                .append_turn(
                    &id(n),
                    &Turn {
                        speaker: Speaker::You,
                        text: n.to_owned(),
                        at_ms: at,
                    },
                )
                .await
                .expect("append");
        }
        let kept: Vec<String> = store
            .conversations()
            .await
            .expect("list")
            .into_iter()
            .map(|c| c.id.as_str().to_owned())
            .collect();
        assert_eq!(kept, vec!["c".to_owned(), "b".to_owned()]);

        store
            .append_turn(&id("c"), &Turn::now(Speaker::Agent, "second"))
            .await
            .expect("append");
        let error = store
            .append_turn(&id("c"), &Turn::now(Speaker::Agent, "third"))
            .await
            .expect_err("the turn ceiling must apply to the fake too");
        assert_eq!(error.code(), "too_large", "{error}");
    }

    #[tokio::test]
    async fn the_fake_read_mark_is_monotonic_and_refuses_an_unorderable_id() {
        let store = FakeStore::new();
        let channel = ChannelId("1111111111".to_owned());
        store
            .mark_read(&channel, &MessageId("1000000000000000200".to_owned()))
            .await
            .expect("mark");
        let back = store
            .mark_read(&channel, &MessageId("1000000000000000100".to_owned()))
            .await
            .expect("mark");
        assert_eq!(back.last_read.as_str(), "1000000000000000200");
        assert_eq!(
            store
                .mark_read(&channel, &MessageId("latest".to_owned()))
                .await
                .expect_err("must refuse")
                .code(),
            "bad_id"
        );
    }

    #[tokio::test]
    async fn the_fake_bounds_the_summary_cache_exactly_as_the_real_store_does() {
        // Parity, and it is load-bearing: every test of the summary path runs against this store,
        // so a fake with an unbounded cache would let an unbounded cache ship.
        let store = FakeStore::with_retention(Retention {
            max_summaries: 2,
            retain_days: 0,
            ..Retention::default()
        });
        let key = |n: u64| SummaryKey {
            channel: ChannelId("1111111111".to_owned()),
            message: MessageId(format!("100000000000000000{n}")),
            content_hash: n,
            version: "current".to_owned(),
        };
        for n in 0..3_u64 {
            store
                .cache_summary(&key(n), &format!("summary {n}"))
                .await
                .expect("cache");
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        assert_eq!(
            store.cached_summaries(),
            2,
            "the fake's summary cache grew past the ceiling the real one enforces"
        );
        assert_eq!(
            store.cached_summary(&key(0)).await.expect("read"),
            None,
            "the oldest entry must be the one evicted"
        );
        assert_eq!(
            store
                .cached_summary(&key(2))
                .await
                .expect("read")
                .as_deref(),
            Some("summary 2")
        );
    }
}
