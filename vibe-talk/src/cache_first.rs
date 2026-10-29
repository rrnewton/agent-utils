//! A page's reads answered from this server's own copy first, with the provider read behind the
//! answer. `#622 cache-first-reads`.
//!
//! # Why this exists
//!
//! The owner, 2026-10-28: "I expect that there's really no situation where my client directly
//! cares about upstream slack at all. I just get a fast view of the server. The server worries
//! about pulling eagerly or lazily information from upstream." Until then a timeline read the
//! message cache's serve rule could not prove current went to the provider first, and the cache
//! answered only once the provider had failed it or kept it waiting: 1.5 s for a page entering a
//! view (`#617 cold-load-first-paint`), `timeline_fallback_ms` (10 s) for every other read. Slack's
//! All is never proven, so every read of it waited.
//!
//! # What it does
//!
//! A page's newest or catch-up read ([`eligible`]) of a view the cache holds anything of is
//! answered from the cache at once (`crate::ops`), as the stand-in of `#611
//! timeline-read-failures` answered it after the wait: `refresh_failed` set, so the page lets it
//! answer for nothing it lacks and reads again soon (3 s, then 10 s). Its `as_of` is when the
//! cache's copy was last brought up to date: the later of its latest listing of the view's newest
//! end and the latest upstream read of the view that answered ([`Landed::read_ms`]), or, for a
//! view neither has reached (a thread nobody has opened), the oldest observation it holds; so the
//! page says "Refresh failed" once that is two minutes old, provider outage or not. `#688
//! vt-timeline-latency`: it used to need a listing, and a thread live ingestion had brought but
//! nobody had opened went to the provider first and waited out its rate limits on the request.
//!
//! The upstream read it stands for is begun behind the answer, one per view at a time (the
//! refresh claim, as every read of a view shares it), and fills the cache. Its answer is kept here
//! for [`LANDED_KEPT`] ([`Landed`]): the next read of the same view from the same cursor, the
//! page's own soon read, is answered with it, once, as if it had joined that read. So a cold load
//! paints from the cache in one round trip to this server, and a few seconds later lands the
//! provider's own page, with everything that page proves (what is gone, where the view's cursors
//! and cover are). A failure the cache may not hide (an expired cursor, an access answer) is kept
//! the same way and is that next read's answer, so the page learns of it as it did before.
//!
//! Slack goes first only for what the cache holds nothing of, with the fallback as before; and
//! for the reads that exist to ask the provider what this server does not know: placing a live
//! message's thread (`place`, `reply-place`).
//!
//! `#650 cache-first-older-reads`: All's counts read (`counts`) is answered the same way; the page
//! asks once more ten seconds after such an answer, which takes the read that landed behind it, so
//! a read it trusts still ends its asking. A step back from a page answered from the cache is
//! answered from the file when its claims prove the step whole (`crate::ops`), and a page the file
//! holds from the view's start says there is nothing older (`crate::cache::below`).
//!
//! # Seeing it in the log
//!
//! Every request's access line says where its read was answered from (`served_from`, [`Source`]),
//! beside how long it took.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::cache::sink::RefreshKey;
use crate::read_reason::ReadReason;
use crate::threads::{TimelinePage, TimelineRequest, TimelineView};
use crate::timeline_forward::GenericPosition;

/// How long an upstream read begun behind an answer from the cache answers the next read after it.
/// The page reads again 3 s and then 10 s after such an answer, so the second of those finds a
/// read that landed at any time after the first.
pub const LANDED_KEPT: Duration = Duration::from_secs(15);

/// Whether a timeline read is answered from the cache first when it holds anything of the view:
/// the page's own reads of a view's newest end, or of what changed since a cursor, including the
/// newest page a step back reads for its cursor (`older`, `search`) and All's counts read
/// (`counts`, `#650 cache-first-older-reads`: the page counts a counts read done only when it is
/// not the server's copy, so it asks once more soon after one, and takes what landed behind it).
/// Not a step back itself (`before`), which `crate::ops` answers from the file when it can; not
/// the reads that ask the provider what this server does not know, where to place a live message
/// (`place`, `reply-place`); not the server's own reads (naming, a tool, the live poller), which
/// keep their own rules. "Older threads…" (`threads`) only when the cache's answer knows the way
/// back ([`answers`]).
#[must_use]
pub fn eligible(request: &TimelineRequest) -> bool {
    use ReadReason as R;
    request.before.is_none()
        && matches!(
            ReadReason::current(),
            R::Enter
                | R::Poll
                | R::Live
                | R::Mutation
                | R::Visible
                | R::Reset
                | R::Replay
                | R::Pull
                | R::Expired
                | R::Side
                | R::Threads
                | R::Older
                | R::Search
                | R::Counts
        )
}

/// Whether what the cache holds answers `request`: anything of its view, except that "Older
/// threads…" (`reason=threads`) asks for the thread list's way back, which only an upstream list
/// gives.
#[must_use]
pub fn answers(request: &TimelineRequest, next_before: Option<&str>) -> bool {
    !(request.view == TimelineView::Threads
        && ReadReason::current() == ReadReason::Threads
        && next_before.is_none())
}

/// Where a request's read was answered from, for its access line (`served_from`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The cache, by its serve rule: proven what the provider would say now (`#384 cache-serve`).
    Cache,
    /// The cache, first, with the upstream read behind the answer (this module).
    CacheFirst,
    /// The cache, for a live-fed channel's thread list between upstream reads (`#614`).
    LiveFed,
    /// An upstream read begun behind an earlier answer from the cache ([`Landed`]).
    Landed,
    /// The cache, for a step back from a page it answered, its claims proving the step whole,
    /// with the provider's step read behind it when it could correct anything. `#650
    /// cache-first-older-reads` (`crate::ops`).
    Older,
    /// The provider, read for this request.
    Upstream,
    /// The cache, after the provider failed the read or kept it waiting (`#611`).
    StandIn,
    /// Speech preparation: every message from this server (cache or recent memory), no fetch.
    Held,
    /// Speech preparation: at least one message fetched from the provider.
    Fetched,
}

impl Source {
    /// The word the access line uses.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cache => "cache",
            Self::CacheFirst => "cache-first",
            Self::LiveFed => "live-fed",
            Self::Landed => "landed",
            Self::Older => "cache-older",
            Self::Upstream => "upstream",
            Self::StandIn => "stand-in",
            Self::Held => "held",
            Self::Fetched => "fetched",
        }
    }
}

tokio::task_local! {
    static SOURCE: Arc<Mutex<Option<Source>>>;
}

/// Run `work`, and hand back where the read it made was answered from, as [`note`] noted it.
pub async fn noting<F: Future>(work: F) -> (F::Output, Option<Source>) {
    let slot = Arc::new(Mutex::new(None));
    let output = SOURCE.scope(Arc::clone(&slot), work).await;
    let source = *slot.lock().unwrap_or_else(PoisonError::into_inner);
    (output, source)
}

/// Note where the request being served had its read answered from; the last note stands. Outside
/// [`noting`], nothing.
pub fn note(source: Source) {
    let _ = SOURCE.try_with(|slot| {
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(source);
    });
}

/// An upstream read that answered after its request had been answered from the cache.
#[derive(Clone, Debug)]
pub struct LandedRead {
    /// Its page, raw as the provider answered it, already offered to the cache.
    pub page: TimelinePage,
    /// The generic position a catch-up read reached.
    pub position: Option<GenericPosition>,
    /// When the read began, on this server's clock: the page's version.
    pub began_ms: i64,
    /// When the read began, for what speech preparation remembers of it.
    pub fetch_began: Instant,
}

/// What an upstream read begun behind an answer from the cache ended with, for the next read.
#[derive(Debug)]
pub enum LandedAnswer {
    /// Its page.
    Page(Box<LandedRead>),
    /// A failure the cache may not stand in for: an expired cursor, an access answer, a
    /// provider's own refusal. The next read answers with it, as it would have read it itself.
    Refused(crate::ops::OpError),
}

/// Upstream reads begun behind answers from the cache, each kept for [`LANDED_KEPT`] for the one
/// next read of the same view from the same cursor; and when each view was last read upstream.
/// Memory only, one entry per view read: a channel taken off the list is refused before this is
/// asked, and the storage purge clears it, and refuses what a read running across the purge would
/// keep.
#[derive(Debug, Default)]
pub struct Landed {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    /// Moved by every purge: a read begun before it keeps nothing.
    generation: u64,
    reads: HashMap<RefreshKey, Kept>,
    /// By [`RefreshKey::view`]: when the latest upstream read of the view that answered began, by
    /// the message cache's clock.
    read_ms: HashMap<RefreshKey, i64>,
}

/// One landed read: when it landed, the cursor it continued from, and how it ended.
#[derive(Debug)]
struct Kept {
    at: Instant,
    after: Option<String>,
    answer: LandedAnswer,
}

impl Landed {
    /// Nothing kept yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Now: hand it to [`Self::keep`] from a read begun now, so a purge in between keeps nothing.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.inner().generation
    }

    /// Keep `answer`, how the read `key` names from the cursor `after` ended, begun at
    /// `generation`; nothing when a purge came since.
    pub fn keep(
        &self,
        key: RefreshKey,
        after: Option<String>,
        generation: u64,
        answer: LandedAnswer,
    ) {
        let mut inner = self.inner();
        if inner.generation != generation {
            return;
        }
        inner
            .reads
            .retain(|_, kept| kept.at.elapsed() < LANDED_KEPT);
        inner.reads.insert(
            key,
            Kept {
                at: Instant::now(),
                after,
                answer,
            },
        );
    }

    /// Take how the read `key` names from the cursor `after` ended, when it landed within
    /// [`LANDED_KEPT`]: once. Another reader, a tab of its own, reads for itself.
    #[must_use]
    pub fn take(&self, key: &RefreshKey, after: Option<&str>) -> Option<LandedAnswer> {
        let mut inner = self.inner();
        let fits = inner
            .reads
            .get(key)
            .is_some_and(|kept| kept.at.elapsed() < LANDED_KEPT && kept.after.as_deref() == after);
        if fits {
            inner.reads.remove(key).map(|kept| kept.answer)
        } else {
            None
        }
    }

    /// Note that an upstream read of the view `view` ([`RefreshKey::view`]) begun at `began_ms`, by
    /// the message cache's clock, answered, begun at `generation`.
    pub fn note_read(&self, view: RefreshKey, began_ms: i64, generation: u64) {
        let mut inner = self.inner();
        if inner.generation != generation {
            return;
        }
        let at = inner.read_ms.entry(view).or_insert(began_ms);
        *at = (*at).max(began_ms);
    }

    /// When the latest upstream read of the view `view` that answered began ([`Self::note_read`]).
    #[must_use]
    pub fn read_ms(&self, view: &RefreshKey) -> Option<i64> {
        self.inner().read_ms.get(view).copied()
    }

    /// Forget everything kept, and keep nothing a read begun before now brings: the storage purge.
    pub fn clear(&self) {
        let mut inner = self.inner();
        inner.generation += 1;
        inner.reads.clear();
        inner.read_ms.clear();
    }

    /// Whether how the read `key` names from the cursor `after` ended is kept, without taking it.
    /// For tests that wait for a read behind an answer to land.
    #[doc(hidden)]
    #[must_use]
    pub fn holds(&self, key: &RefreshKey, after: Option<&str>) -> bool {
        self.inner()
            .reads
            .get(key)
            .is_some_and(|kept| kept.at.elapsed() < LANDED_KEPT && kept.after.as_deref() == after)
    }

    /// Make everything kept `by` older. For tests of what happens as it ages.
    #[doc(hidden)]
    pub fn age(&self, by: Duration) {
        for kept in self.inner().reads.values_mut() {
            kept.at = kept.at.checked_sub(by).unwrap_or(kept.at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChannelId;

    fn key() -> RefreshKey {
        RefreshKey::timeline(ChannelId("C1".to_owned()), TimelineView::Flat, None, 50)
    }

    fn view() -> RefreshKey {
        RefreshKey::view(ChannelId("C1".to_owned()), TimelineView::Flat, None)
    }

    fn read() -> LandedAnswer {
        LandedAnswer::Page(Box::new(LandedRead {
            page: TimelinePage::default(),
            position: None,
            began_ms: 7,
            fetch_began: Instant::now(),
        }))
    }

    #[test]
    fn a_landed_read_answers_the_same_cursor_once_until_it_is_too_old() {
        let landed = Landed::new();
        let now = landed.generation();
        landed.keep(key(), Some("c1".to_owned()), now, read());
        assert!(landed.take(&key(), None).is_none(), "another cursor");
        assert!(landed.take(&key(), Some("c2")).is_none(), "another cursor");
        assert!(landed.take(&key(), Some("c1")).is_some());
        assert!(landed.take(&key(), Some("c1")).is_none(), "once");
        landed.keep(key(), Some("c1".to_owned()), now, read());
        landed.age(LANDED_KEPT);
        assert!(landed.take(&key(), Some("c1")).is_none(), "too old");
    }

    #[test]
    fn the_purge_forgets_what_landed_and_refuses_what_lands_across_it() {
        let landed = Landed::new();
        let before = landed.generation();
        landed.keep(key(), None, before, read());
        landed.note_read(view(), 5, before);
        landed.clear();
        assert!(landed.take(&key(), None).is_none());
        assert_eq!(landed.read_ms(&view()), None);
        landed.keep(key(), None, before, read());
        landed.note_read(view(), 6, before);
        assert!(
            landed.take(&key(), None).is_none(),
            "begun before the purge"
        );
        assert_eq!(landed.read_ms(&view()), None);
    }

    #[test]
    fn the_latest_read_of_a_view_is_the_latest_begun() {
        let landed = Landed::new();
        let now = landed.generation();
        landed.note_read(view(), 9, now);
        landed.note_read(view(), 7, now);
        assert_eq!(landed.read_ms(&view()), Some(9));
    }

    #[tokio::test]
    async fn the_last_note_stands_and_none_outside() {
        note(Source::Upstream);
        let ((), source) = noting(async {
            note(Source::CacheFirst);
            note(Source::Landed);
        })
        .await;
        assert_eq!(source, Some(Source::Landed));
        let ((), source) = noting(async {}).await;
        assert_eq!(source, None);
    }
}
