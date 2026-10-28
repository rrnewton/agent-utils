//! A thread the cache holds whole, handed to a read that lists every thread's replies in place of
//! a call upstream. `#603 server-side-search`; see [`crate::held_replies`] for why.
//!
//! The cache hands a thread out only when, in one snapshot:
//!
//! - it holds exactly as many replies as the root just read upstream counts, the newest of them
//!   the latest reply the root names, and none of them only as a push brought it (merge rule 5);
//! - the whole thread, from its root, was listed by reads: the stretch below the head's settled
//!   top lies in the thread's coverage, followed down from the head ([`coverage::horizon`]) to the
//!   root, which is no older than the age limit, and every reply in the unsettled band above it, which no
//!   claim enters, was listed by the latest read of the thread's newest end, whose listing began at
//!   or below the band's lowest instant the thread reaches (the band rule the shadow's `messages`
//!   rule applies). So no reply is missing that the count could hide behind another one: a deleted
//!   reply held, and a live one evicted, add up to the right count;
//! - each claim used, and that read when the band is used, began within [`HELD_FRESH_MS`], the
//!   period Flat's own memory of a thread's replies is trusted for: Slack's feed carries
//!   additions, so an edit is reflected only as of the latest listing, and All showed an edited
//!   reply at most that late before the cache stood in for it;
//! - and after the latest offer dropped for the channel, in this process or as the file records:
//!   a dropped offer may have been an edit or a deletion, and only a listing that began after it
//!   vouches for what it saw.
//!
//! The thread's scope must not be blocked (in doubt, being erased, its provider suspended), which
//! the read itself checks. [`Sink`] adds the gates [`Sink::try_serve`] applies before and after its
//! snapshot: serving on, the channel on the server's list, its provider's account checked against
//! the file. Each reply handed out carries the observation that set it (`observed_ms`), as a
//! served page's do, so it is never taken for a copy this read fetched.

use async_trait::async_trait;

use super::coverage::{self, Interval};
use super::reader::HeadRecord;
use super::rows::CachedMessage;
use super::sink::{Sink, Verification};
use super::Read;
use crate::held_replies::{HeldReplies, RootSays};
use crate::model::{ChannelId, Message};

/// How long ago the listings a thread's copy rests on may have begun: the period Slack's Flat
/// read trusts its own memory of a thread's replies for (`FLAT_RECONCILE`, fifteen minutes).
pub const HELD_FRESH_MS: i64 = 15 * 60 * 1000;

/// What one snapshot of a thread's scope found, judged against what its root says.
pub(crate) struct Snapshot<'a> {
    /// The scope's newest rows, oldest first, at least two more than the root counts (the root,
    /// and one more to tell a longer thread).
    pub(crate) rows: Vec<CachedMessage>,
    pub(crate) intervals: &'a [Interval],
    pub(crate) head: Option<HeadRecord>,
    /// The channel's age cutoff, provider time.
    pub(crate) cutoff_ms: i64,
    /// The latest offer dropped for the channel, local time.
    pub(crate) dropped_ms: Option<i64>,
    /// The oldest local time a claim, or the read that vouched for the band, may have begun at.
    pub(crate) fresh_after_ms: i64,
}

/// The replies, oldest first, when the snapshot is the thread `root` describes and may be handed
/// out; see the module documentation.
pub(crate) fn whole(snapshot: Snapshot<'_>, root: &RootSays) -> Option<Vec<Message>> {
    let replies: Vec<CachedMessage> = snapshot
        .rows
        .into_iter()
        .filter(|row| row.message.id != root.root_id)
        .collect();
    let count = u64::try_from(replies.len()).ok()?;
    if count != root.reply_count || count == 0 {
        return None;
    }
    let last = replies.last()?;
    if last.message.id != root.latest_reply
        || replies.iter().any(|row| row.content_partial)
        || replies.first()?.created_ms < root.root_ms
    {
        return None;
    }
    let record = snapshot.head?;
    let head = record.head;
    let top = head.settled_top();
    let trusted = |began_ms: i64| {
        began_ms >= snapshot.fresh_after_ms
            && snapshot.dropped_ms.is_none_or(|dropped| began_ms > dropped)
    };
    // Below the settled top: the claims, down to the root.
    if root.root_ms < top {
        let horizon = coverage::horizon(snapshot.intervals, Some(&head), snapshot.cutoff_ms)?;
        // `from_ms` is never below the age limit: a root older than that may have lost replies to
        // eviction.
        if horizon.from_ms > root.root_ms || !trusted(horizon.as_of_ms) {
            return None;
        }
    }
    // In the band: what the latest read of the newest end listed, from at or below where the
    // thread enters it.
    if last.created_ms >= top {
        let lowest = root.root_ms.max(top);
        let listed_from = record.checked_from_ms?;
        let vouched = replies
            .iter()
            .filter(|row| row.created_ms >= top)
            .all(|row| {
                row.created_ms <= head.head_ms
                    && row
                        .vouched_ms
                        .is_some_and(|vouched| vouched >= record.checked_ms)
            });
        if listed_from > lowest || !vouched || !trusted(record.checked_ms) {
            return None;
        }
    }
    Some(
        replies
            .into_iter()
            .map(|row| Message {
                observed_ms: Some(row.observed_ms),
                ..row.message
            })
            .collect(),
    )
}

#[async_trait]
impl HeldReplies for Sink {
    async fn whole(&self, channel: &ChannelId, root: &RootSays) -> Option<Vec<Message>> {
        let gated = |sink: &Self| {
            sink.serve_enabled()
                .then(|| sink.chan_key(channel))
                .flatten()
                .filter(|chan| sink.verification(&chan.provider_key) == Verification::Verified)
        };
        let chan = gated(self)?;
        let cache = self.cache().filter(|cache| cache.is_open())?;
        let asked = (chan.clone(), root.clone());
        let found = tokio::task::spawn_blocking(move || {
            let (chan, root) = asked;
            match cache.whole_thread(&chan, &root, HELD_FRESH_MS) {
                Read::Served(found) => found,
                _ => None,
            }
        })
        .await
        .ok()
        .flatten()?;
        // Checked again after the snapshot, as a serve is: what changed under the read may not
        // slip it past them.
        (gated(self).as_ref() == Some(&chan)).then_some(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::coverage::{Head, Source};
    use crate::cache::fixtures;
    use crate::model::MessageId;

    const NOW: i64 = 10_000_000;
    /// When the latest read of the thread's newest end began.
    const CHECKED: i64 = NOW - 2_000;

    fn row(id: &str, created_ms: i64) -> CachedMessage {
        CachedMessage {
            message: fixtures::message(id, "u", "text", created_ms),
            created_ms,
            observed_ms: NOW - 1_000,
            content_partial: false,
            vouched_ms: Some(CHECKED),
        }
    }

    fn says(count: u64, latest: &str) -> RootSays {
        RootSays {
            thread_id: "t".to_owned(),
            root_id: MessageId("root".to_owned()),
            root_ms: 100,
            reply_count: count,
            latest_reply: MessageId(latest.to_owned()),
        }
    }

    fn claim(t0_ms: i64, t1_ms: i64, listed_ms: i64) -> Interval {
        Interval {
            t0_ms,
            t1_ms,
            listed_ms,
            full_ms: Some(listed_ms),
            complete: false,
            source: Source::Read,
        }
    }

    /// A head at 400 whose band begins at `unsettled_from_ms`, listed from `listed_from`.
    fn head(unsettled_from_ms: i64, listed_from: Option<i64>) -> HeadRecord {
        HeadRecord {
            head: Head {
                head_ms: 400,
                unsettled_from_ms,
                head_listed_ms: CHECKED,
            },
            checked_ms: CHECKED,
            checked_from_ms: listed_from,
        }
    }

    /// Settled: the band begins past the head, so every reply lies below the settled top.
    fn settled() -> HeadRecord {
        head(1_000, Some(0))
    }

    fn judge(
        rows: Vec<CachedMessage>,
        intervals: &[Interval],
        head: HeadRecord,
        dropped_ms: Option<i64>,
        root: &RootSays,
    ) -> Option<Vec<String>> {
        whole(
            Snapshot {
                rows,
                intervals,
                head: Some(head),
                cutoff_ms: 0,
                dropped_ms,
                fresh_after_ms: NOW - HELD_FRESH_MS,
            },
            root,
        )
        .map(|messages| messages.into_iter().map(|m| m.id.0).collect())
    }

    /// The root at 100 and replies at 200, 300 and 400.
    fn thread() -> Vec<CachedMessage> {
        vec![
            row("root", 100),
            row("a", 200),
            row("b", 300),
            row("c", 400),
        ]
    }

    #[test]
    fn a_thread_held_as_its_root_describes_it_is_handed_out_without_its_root() {
        let claims = [claim(100, 401, NOW - 5_000)];
        assert_eq!(
            judge(thread(), &claims, settled(), None, &says(3, "c")),
            Some(vec!["a".to_owned(), "b".to_owned(), "c".to_owned()])
        );
    }

    #[test]
    fn a_count_or_a_latest_reply_that_differs_is_not_the_thread() {
        let claims = [claim(100, 401, NOW - 5_000)];
        assert_eq!(
            judge(thread(), &claims, settled(), None, &says(4, "c")),
            None
        );
        assert_eq!(
            judge(thread(), &claims, settled(), None, &says(2, "c")),
            None
        );
        assert_eq!(
            judge(thread(), &claims, settled(), None, &says(3, "b")),
            None
        );
        let lone = vec![row("root", 100)];
        assert_eq!(
            judge(lone, &claims, settled(), None, &says(0, "root")),
            None
        );
    }

    #[test]
    fn a_reply_held_only_as_a_push_brought_it_is_not_handed_out() {
        let claims = [claim(100, 401, NOW - 5_000)];
        let mut rows = thread();
        rows[2].content_partial = true;
        assert_eq!(judge(rows, &claims, settled(), None, &says(3, "c")), None);
    }

    #[test]
    fn coverage_must_reach_the_root_below_the_settled_top() {
        let late = [claim(150, 401, NOW - 5_000)];
        assert_eq!(
            judge(thread(), &late, settled(), None, &says(3, "c")),
            None,
            "the claims reach the first reply held, not the root: a reply between may be missing"
        );
        let holed = [claim(100, 300, NOW - 5_000), claim(350, 401, NOW - 5_000)];
        assert_eq!(
            judge(thread(), &holed, settled(), None, &says(3, "c")),
            None,
            "a gap between the claims"
        );
        assert_eq!(judge(thread(), &[], settled(), None, &says(3, "c")), None);
    }

    #[test]
    fn a_thread_reaching_past_the_age_limit_is_not_handed_out() {
        // A reply older than the cutoff may have been evicted while a newer one, deleted
        // upstream, is still held: the count would add up.
        let claims = [claim(0, 401, NOW - 5_000)];
        let at = |cutoff_ms| {
            whole(
                Snapshot {
                    rows: thread(),
                    intervals: &claims,
                    head: Some(settled()),
                    cutoff_ms,
                    dropped_ms: None,
                    fresh_after_ms: NOW - HELD_FRESH_MS,
                },
                &says(3, "c"),
            )
        };
        assert!(at(50).is_some());
        assert!(at(150).is_none(), "the cutoff lies above the root");
    }

    #[test]
    fn each_reply_carries_the_observation_that_set_it() {
        let claims = [claim(100, 401, NOW - 5_000)];
        let mut rows = thread();
        rows[1].observed_ms = NOW - 9_000;
        let held = whole(
            Snapshot {
                rows,
                intervals: &claims,
                head: Some(settled()),
                cutoff_ms: 0,
                dropped_ms: None,
                fresh_after_ms: NOW - HELD_FRESH_MS,
            },
            &says(3, "c"),
        )
        .expect("held");
        assert_eq!(held[0].observed_ms, Some(NOW - 9_000));
        assert_eq!(held[1].observed_ms, Some(NOW - 1_000));
    }

    #[test]
    fn claims_listed_too_long_ago_or_before_a_dropped_offer_are_not_trusted() {
        let stale = [claim(100, 401, NOW - HELD_FRESH_MS - 1)];
        assert_eq!(
            judge(thread(), &stale, settled(), None, &says(3, "c")),
            None
        );
        let claims = [claim(100, 401, NOW - 5_000)];
        assert_eq!(
            judge(
                thread(),
                &claims,
                settled(),
                Some(NOW - 5_000),
                &says(3, "c")
            ),
            None,
            "dropped as the listing began"
        );
        assert!(judge(
            thread(),
            &claims,
            settled(),
            Some(NOW - 6_000),
            &says(3, "c")
        )
        .is_some());
    }

    #[test]
    fn replies_in_the_band_must_be_listed_by_the_latest_read_of_the_newest_end() {
        // The band begins at 250: replies at 300 and 400 are in it, 200 below it.
        let claims = [claim(100, 250, NOW - 5_000)];
        assert!(
            judge(thread(), &claims, head(250, Some(250)), None, &says(3, "c")).is_some(),
            "claims below the band, the read's listing over it"
        );
        assert_eq!(
            judge(thread(), &claims, head(250, Some(260)), None, &says(3, "c")),
            None,
            "the read's listing began above the band's lowest reply"
        );
        assert_eq!(
            judge(thread(), &claims, head(250, None), None, &says(3, "c")),
            None,
            "the file does not say where that read's listing began"
        );
        let mut rows = thread();
        rows[2].vouched_ms = Some(CHECKED - 1);
        assert_eq!(
            judge(rows, &claims, head(250, Some(250)), None, &says(3, "c")),
            None,
            "a band reply an earlier read listed, not the latest"
        );
        assert_eq!(
            judge(
                thread(),
                &claims,
                head(250, Some(250)),
                Some(CHECKED),
                &says(3, "c")
            ),
            None,
            "an offer dropped as the latest read began"
        );
    }

    #[test]
    fn a_thread_wholly_in_the_band_needs_no_claim() {
        assert!(judge(thread(), &[], head(100, Some(100)), None, &says(3, "c")).is_some());
        assert_eq!(
            judge(thread(), &[], head(100, Some(150)), None, &says(3, "c")),
            None,
            "the read's listing began above the root"
        );
        assert_eq!(
            judge(thread(), &[], head(150, Some(100)), None, &says(3, "c")),
            None,
            "the root lies below the band, where no claim holds it"
        );
    }
}
