//! What the message cache would have served, measured beside the reads that go upstream. `#208
//! persistent-cache`, milestone `cache-fill`.
//!
//! Nothing is answered from the cache yet; a later milestone serves from it, by the rules of plan
//! section 5.4. Until then, every read that milestone could answer is SHADOWED: while the read goes
//! upstream, the cache is asked what it holds for the same question, the rule decides whether the
//! cache would have answered, and once upstream has answered, the two are compared. Counts only,
//! in memory, from the moment the process starts.
//!
//! This module is the pure part: the rules, per consumer, and the comparison. The evaluation runs
//! off the request's path ([`super::Sink::shadow`]): its reads on the blocking pool while upstream
//! is answering, its decision and comparison in a detached task once upstream has. It never delays
//! a response and never writes, whoever asked: a read-scope caller is shadowed as much as any
//! other, since the later milestone serves read scope too, and leaves the file exactly as it was.
//!
//! # The rules
//!
//! | Consumer | Would serve when |
//! |---|---|
//! | `messages`, `digest`, `todo`, `bankruptcy`, `speech`: the callers of `ops::messages` | the scope's coverage is contiguous from the head down to `limit` rows, the head was listed at most [`FRESH_UNFED_MS`] before the ask ([`FRESH_LIVE_FED_MS`] when a live producer feeds the channel), the channel is not suspect, and the barrier passed |
//! | `message_by_id`, `summary` | the message is held, the channel is not suspect (an offer dropped for it may have been this message's edit or deletion, whatever its feed carries), and the barrier passed, and: on a bridge, whose feed carries deletions, the coverage of its home scope was revalidated within [`REVALIDATED_BRIDGE_MS`] or the channel is live-fed; on Slack and Discord, whose feeds carry only additions, within [`REVALIDATED_ADDITIONS_ONLY_MS`], and otherwise it would take one existence check upstream first (`needs_existence_check`) |
//! | `page` | the span asked for lies inside complete coverage (up to the page's last row, when it is full), and the barrier passed; an open-ended page needs a fresh head as well |
//! | `count` | complete coverage spans `[since, head − 120 s]` (so never on Slack, whose coverage is never complete), the head is as fresh as for `messages`, and the barrier passed |
//! | `timeline_fallback` | coverage holds `limit` rows down from the head, the channel is not suspect, and the barrier passed: what a newest page answered from the cache would be, were upstream to fail. Slack's Flat view never |
//!
//! # What a miss says
//!
//! Checked in this order, and counted under the first that applies:
//!
//! - `off`: no cache file is open, though the configuration has a `[message_cache]` table;
//! - `blocked`: the cache would not read the channel or the scope at all: it is not on the route
//!   snapshot, an erase of it is waiting, access to it is in doubt, its provider is suspended, or
//!   the read failed;
//! - `suspect`: an offer for the channel was dropped (the queue was full, or its provider's account
//!   was not checked against the file), and no read of its newest end that began since has landed;
//! - `gap`: the cache does not hold what would be served: no head, a hole in the coverage, or not
//!   the message;
//! - `incomplete`: covered, but by reads that may have left a message out (Slack's);
//! - `stale_head`: what the cache holds was confirmed too long ago: the head, or the coverage of a
//!   message's home scope on a bridge;
//! - `barrier`: what was queued before the ask was not applied within [`BARRIER`].
//!
//! And one that is no verdict of a rule: `late`, an evaluation that found what a read begun after
//! the ask had written, such as this request's own: the cache it read is not the cache the ask
//! would have found, so it is neither decided nor compared.
//!
//! # The comparison
//!
//! When the cache would have served and upstream answered, the two answers are compared over the
//! stretch both speak for: from the newer of their oldest messages up, for a newest window whose
//! older end either one cut short; up to the older of their last messages, for a page either one
//! cut short. Messages are matched by id, and their text by the same content hash the summary
//! cache keys on; no text is kept.
//!
//! - `compared` and `agreed`: comparisons made, and those that found no difference;
//! - `missing_rows`: messages upstream listed that the cache's answer lacked;
//! - `extra_rows`: messages the cache's answer held that upstream no longer lists. Served, each
//!   would have brought a deleted message back; a comparison that finds any is warned about, with
//!   its counts and nothing else;
//! - `content_differs`: messages both listed whose text differs.
//!
//! The three row counts are summed over comparisons.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Serialize;

use super::coverage::{self, Head, Interval};
use super::observe::Shape;
use super::rows::{created_ms, CachedMessage, Scope};
use crate::model::{Message, MessageId};
use crate::threads::TimelineView;

/// How recently a head must have been listed to serve `messages` or `count` when nothing feeds the
/// channel: the recent windows' own reuse period ([`crate::recent::REUSE_UNFED`]).
pub const FRESH_UNFED_MS: i64 = 15_000;
/// The same, when a live producer feeds the channel ([`crate::recent::REUSE_LIVE_FED`]).
pub const FRESH_LIVE_FED_MS: i64 = 60_000;
/// How recently a message's home scope must have been revalidated to serve it by id on a bridge
/// whose channel no live producer feeds.
pub const REVALIDATED_BRIDGE_MS: i64 = 24 * 60 * 60 * 1000;
/// How recently a message's home scope must have been revalidated to serve it by id on Slack or
/// Discord without asking upstream whether it still exists.
pub const REVALIDATED_ADDITIONS_ONLY_MS: i64 = 2 * 60 * 60 * 1000;
/// How long an evaluation waits for what was queued before the ask to be applied.
pub const BARRIER: Duration = Duration::from_millis(100);

/// Who asked: each caller the later milestone would answer from the cache, counted apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Consumer {
    /// `GET /messages`, read straight through `ops::messages`.
    Messages,
    /// A digest: `GET /digest` and MCP `digest_channel`.
    Digest,
    /// A channel's to-do list.
    Todo,
    /// Declaring bankruptcy on a channel.
    Bankruptcy,
    /// Speech preparation's window of a channel.
    Speech,
    /// One message by id: `GET /messages/{id}`, MCP `read_message`, and speech preparation's
    /// lookup of a reply.
    MessageById,
    /// The message a summary is of, when the summary route reads upstream.
    Summary,
    /// `/page` and MCP `read_page`, walked forward from an instant.
    Page,
    /// `/count` and MCP `count_messages`.
    Count,
    /// `/timeline`'s newest page, as a page answered from the cache if upstream failed.
    TimelineFallback,
}

impl Consumer {
    /// Every consumer, in the order the report lists them.
    pub const ALL: [Self; 10] = [
        Self::Messages,
        Self::Digest,
        Self::Todo,
        Self::Bankruptcy,
        Self::Speech,
        Self::MessageById,
        Self::Summary,
        Self::Page,
        Self::Count,
        Self::TimelineFallback,
    ];

    /// The name the report and the log use.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Digest => "digest",
            Self::Todo => "todo",
            Self::Bankruptcy => "bankruptcy",
            Self::Speech => "speech",
            Self::MessageById => "message_by_id",
            Self::Summary => "summary",
            Self::Page => "page",
            Self::Count => "count",
            Self::TimelineFallback => "timeline_fallback",
        }
    }
}

/// What a read of a channel's newest messages through `ops::messages` is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wants {
    /// The window itself, for `consumer`.
    Window(Consumer),
    /// One message of it, for `consumer`.
    Message(Consumer, MessageId),
}

impl Wants {
    /// The question a read of `limit` messages for this asks of the cache.
    #[must_use]
    pub fn ask(&self, limit: usize) -> Ask {
        match self {
            Self::Window(consumer) => Ask::Window {
                consumer: *consumer,
                limit: limit.max(1),
            },
            Self::Message(consumer, id) => Ask::Message {
                consumer: *consumer,
                id: id.clone(),
            },
        }
    }
}

/// What a request asks, in the terms the cache would answer it in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ask {
    /// A channel's newest `limit` messages.
    Window {
        /// Who asked.
        consumer: Consumer,
        /// How many.
        limit: usize,
    },
    /// One message, by id.
    Message {
        /// Who asked.
        consumer: Consumer,
        /// The message.
        id: MessageId,
    },
    /// A page of at most `limit` messages walked forward from `since_ms`, ending before
    /// `until_ms` when given; provider time.
    Page {
        /// Where the walk begins, inclusive.
        since_ms: i64,
        /// Where the span ends, exclusive.
        until_ms: Option<i64>,
        /// The page's size.
        limit: usize,
    },
    /// How many messages since `since_ms` (since the scope's start when `None`), counting at
    /// most `cap`.
    Count {
        /// Where the count begins, inclusive.
        since_ms: Option<i64>,
        /// The most it counts.
        cap: usize,
    },
    /// The newest page of `view`, of `limit` messages.
    Timeline {
        /// The view.
        view: TimelineView,
        /// The thread, for the Thread view.
        thread: Option<String>,
        /// How many.
        limit: usize,
    },
}

impl Ask {
    /// Who asked.
    #[must_use]
    pub fn consumer(&self) -> Consumer {
        match self {
            Self::Window { consumer, .. } | Self::Message { consumer, .. } => *consumer,
            Self::Page { .. } => Consumer::Page,
            Self::Count { .. } => Consumer::Count,
            Self::Timeline { .. } => Consumer::TimelineFallback,
        }
    }
}

/// What an evaluation reads of one channel, all in one snapshot of the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Query {
    /// A scope's head, its coverage, and its newest `limit` messages.
    Newest {
        /// The scope.
        scope: Scope,
        /// How many messages.
        limit: usize,
    },
    /// One message, the scope it belongs to by nature, and that scope's coverage.
    Message {
        /// The message.
        id: MessageId,
    },
    /// A scope's head, its coverage, and its oldest `limit` messages created in
    /// `[from_ms, until_ms)`, or from `from_ms` on when `until_ms` is `None`.
    Span {
        /// The scope.
        scope: Scope,
        /// Provider time, inclusive.
        from_ms: i64,
        /// Provider time, exclusive.
        until_ms: Option<i64>,
        /// How many messages.
        limit: usize,
    },
}

impl Query {
    /// The scope read, when the query names one: a message's is found by the read.
    #[must_use]
    pub fn scope(&self) -> Option<&Scope> {
        match self {
            Self::Newest { scope, .. } | Self::Span { scope, .. } => Some(scope),
            Self::Message { .. } => None,
        }
    }
}

/// What to read for `ask`, of a channel whose provider has `shape` and whose recent reads go
/// through a bridge's Flat view when `recent_via_flat`. `None` when the cache would never answer
/// the ask, so there is nothing to evaluate or count: Slack's Flat view, and the thread list.
#[must_use]
pub fn query(ask: &Ask, shape: Shape, recent_via_flat: bool) -> Option<Query> {
    Some(match ask {
        // A bridge's window through `/messages` is not proven to be any scope's listing, so it is
        // asked of `main`, which a bridge never covers: a gap, always.
        Ask::Window { limit, .. } => Query::Newest {
            scope: if shape == Shape::Bridge && recent_via_flat {
                Scope::Flat
            } else {
                Scope::Main
            },
            limit: *limit,
        },
        Ask::Message { id, .. } => Query::Message { id: id.clone() },
        // One more than a page: the row that says whether there is more.
        Ask::Page {
            since_ms,
            until_ms,
            limit,
        } => Query::Span {
            scope: Scope::Main,
            from_ms: *since_ms,
            until_ms: *until_ms,
            limit: limit.saturating_add(1),
        },
        Ask::Count { since_ms, cap } => Query::Span {
            scope: Scope::Main,
            from_ms: since_ms.unwrap_or(0),
            until_ms: None,
            limit: cap.saturating_add(1),
        },
        Ask::Timeline {
            view,
            thread,
            limit,
        } => Query::Newest {
            scope: timeline_scope(shape, *view, thread.as_deref())?,
            limit: *limit,
        },
    })
}

/// The scope a newest page of `view` would be answered from, if the cache would ever answer it.
fn timeline_scope(shape: Shape, view: TimelineView, thread: Option<&str>) -> Option<Scope> {
    match (shape, view) {
        (_, TimelineView::Threads) => None,
        (Shape::Bridge, TimelineView::Flat) => Some(Scope::Flat),
        // Slack's Flat page is never answered from the cache (plan section 5.6), and Discord's is
        // never proven.
        (_, TimelineView::Flat) => None,
        (Shape::SlackThread, _) | (_, TimelineView::Main) => Some(Scope::Main),
        (_, TimelineView::Thread) => thread
            .filter(|thread| !thread.is_empty())
            .map(|thread| Scope::Thread(thread.to_owned())),
    }
}

/// What an evaluation found, in one snapshot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// The scope's head, when the query read one and a read of the newest end has landed.
    pub head: Option<Head>,
    /// The coverage of the scope read, or of the message's home scope.
    pub coverage: Vec<Interval>,
    /// The messages read, oldest first.
    pub rows: Vec<CachedMessage>,
    /// The message's home scope, for a query of one message the cache holds.
    pub home: Option<Scope>,
}

/// Whether a provider's live feed carries deletions, so a message it has not reported deleted
/// still exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    /// It does: a bridge's.
    Deletions,
    /// Only additions: Slack's and Discord's (`complete: false`).
    AdditionsOnly,
}

impl Feed {
    /// The feed of a provider of `shape`.
    #[must_use]
    pub fn of(shape: Shape) -> Self {
        if shape == Shape::Bridge {
            Self::Deletions
        } else {
            Self::AdditionsOnly
        }
    }
}

/// Everything beside the snapshot a rule decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Facts {
    /// Local time, by the cache's clock, when the request asked: what the rule decides at.
    pub asked_ms: i64,
    /// Whether a live producer feeds the channel: its provider is polled, or adapters push.
    pub live_fed: bool,
    /// Whether the channel was suspect.
    pub suspect: bool,
    /// Whether what was queued before the ask was applied within [`BARRIER`].
    pub barrier: bool,
    /// What the provider's feed carries.
    pub feed: Feed,
}

/// Why the cache would not have served.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Miss {
    /// No cache file is open.
    Off,
    /// The cache would not read the channel or the scope at all.
    Blocked,
    /// The channel's head is not trusted.
    Suspect,
    /// What would be served is not held.
    Gap,
    /// Covered by reads that may have left a message out.
    Incomplete,
    /// Confirmed too long ago.
    StaleHead,
    /// What was queued before the ask was not applied in time.
    Barrier,
}

impl Miss {
    /// Every reason, in the order they are checked.
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Blocked,
        Self::Suspect,
        Self::Gap,
        Self::Incomplete,
        Self::StaleHead,
        Self::Barrier,
    ];

    /// The name the report uses.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Blocked => "blocked",
            Self::Suspect => "suspect",
            Self::Gap => "gap",
            Self::Incomplete => "incomplete",
            Self::StaleHead => "stale_head",
            Self::Barrier => "barrier",
        }
    }
}

/// What the cache would have done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Answered from what it holds.
    Serve,
    /// Answered from what it holds after one call upstream said the message still exists.
    NeedsExistenceCheck,
    /// Gone upstream, for this reason.
    Miss(Miss),
    /// No verdict: the evaluation read what a read begun after the ask had written.
    Late,
}

/// Whether the snapshot holds anything a read begun at or after `asked_ms` wrote: a head or a
/// claim listed then, or a message seen then. Such a snapshot is not what the ask would have
/// found, and is neither decided on nor compared.
#[must_use]
pub fn late(view: &View, asked_ms: i64) -> bool {
    view.head
        .is_some_and(|head| head.head_listed_ms >= asked_ms)
        || view
            .coverage
            .iter()
            .any(|interval| interval.listed_ms >= asked_ms)
        || view.rows.iter().any(|row| row.observed_ms >= asked_ms)
}

/// Whether the cache would have answered `ask` from `view`. See the module documentation.
#[must_use]
pub fn decide(ask: &Ask, view: &View, facts: &Facts) -> Decision {
    let verdict = match ask {
        Ask::Window { limit, .. } => newest(view, *limit, facts, true),
        Ask::Timeline { limit, .. } => newest(view, *limit, facts, false),
        Ask::Message { .. } => message(view, facts),
        Ask::Page {
            since_ms,
            until_ms,
            limit,
        } => page(view, *since_ms, *until_ms, *limit, facts),
        Ask::Count { since_ms, .. } => count(view, since_ms.unwrap_or(0), facts),
    };
    match verdict {
        Err(miss) => Decision::Miss(miss),
        Ok(_) if !facts.barrier => Decision::Miss(Miss::Barrier),
        Ok(decision) => decision,
    }
}

/// The top of what is settled below a head: the unsettled band's start, or just past the head
/// once the band has settled.
fn settled_top(head: &Head) -> i64 {
    head.unsettled_from_ms.min(head.head_ms.saturating_add(1))
}

fn fresh(head: &Head, facts: &Facts) -> bool {
    let within = if facts.live_fed {
        FRESH_LIVE_FED_MS
    } else {
        FRESH_UNFED_MS
    };
    facts.asked_ms.saturating_sub(head.head_listed_ms) <= within
}

/// A newest window, or a newest page: coverage contiguous from the head down to `limit` rows.
fn newest(view: &View, limit: usize, facts: &Facts, needs_fresh: bool) -> Result<Decision, Miss> {
    if facts.suspect {
        return Err(Miss::Suspect);
    }
    let head = view.head.ok_or(Miss::Gap)?;
    // A window of fewer rows than asked is the whole scope only from its very start, which no
    // coverage reaches: it is clipped at the age limit.
    let bottom = if view.rows.len() >= limit.max(1) {
        view.rows[view.rows.len() - limit.max(1)].created_ms
    } else {
        0
    };
    let top = settled_top(&head);
    // Rows inside the unsettled band were listed by the read that set the head, and need no claim.
    if bottom < top && coverage::covers(&view.coverage, bottom, top).is_none() {
        return Err(Miss::Gap);
    }
    if needs_fresh && !fresh(&head, facts) {
        return Err(Miss::StaleHead);
    }
    Ok(Decision::Serve)
}

/// One message: held, its channel not suspect, and its home scope revalidated recently enough for
/// its provider's feed. A live feed that carries deletions vouches for a message only while
/// nothing it offered was dropped.
fn message(view: &View, facts: &Facts) -> Result<Decision, Miss> {
    if facts.suspect {
        return Err(Miss::Suspect);
    }
    let row = view.rows.first().ok_or(Miss::Gap)?;
    let revalidated = view
        .coverage
        .iter()
        .find(|interval| interval.t0_ms <= row.created_ms && row.created_ms < interval.t1_ms)
        .and_then(|interval| interval.full_ms);
    let within = |limit_ms: i64| {
        revalidated.is_some_and(|full_ms| facts.asked_ms.saturating_sub(full_ms) <= limit_ms)
    };
    match facts.feed {
        Feed::Deletions if facts.live_fed || within(REVALIDATED_BRIDGE_MS) => Ok(Decision::Serve),
        Feed::Deletions if revalidated.is_none() => Err(Miss::Gap),
        Feed::Deletions => Err(Miss::StaleHead),
        Feed::AdditionsOnly if within(REVALIDATED_ADDITIONS_ONLY_MS) => Ok(Decision::Serve),
        Feed::AdditionsOnly => Ok(Decision::NeedsExistenceCheck),
    }
}

/// A page walked forward from `since`: its span inside complete coverage.
fn page(
    view: &View,
    since: i64,
    until: Option<i64>,
    limit: usize,
    facts: &Facts,
) -> Result<Decision, Miss> {
    let full = view.rows.len() > limit;
    // A page that is not full runs to `until`, or, with none, to the newest end: that needs the
    // head, and the head trusted and fresh.
    let open = !full && until.is_none();
    if open && facts.suspect {
        return Err(Miss::Suspect);
    }
    let end = if full {
        view.rows[limit].created_ms.saturating_add(1)
    } else if let Some(until) = until {
        until
    } else {
        view.head.ok_or(Miss::Gap)?.head_ms.saturating_add(1)
    };
    let cover = coverage::covers(&view.coverage, since, end).ok_or(Miss::Gap)?;
    if !cover.complete {
        return Err(Miss::Incomplete);
    }
    if open && !view.head.is_some_and(|head| fresh(&head, facts)) {
        return Err(Miss::StaleHead);
    }
    Ok(Decision::Serve)
}

/// A count from `since`: complete coverage up to the unsettled band, under a fresh head.
fn count(view: &View, since: i64, facts: &Facts) -> Result<Decision, Miss> {
    if facts.suspect {
        return Err(Miss::Suspect);
    }
    let head = view.head.ok_or(Miss::Gap)?;
    let cover = coverage::covers(&view.coverage, since, settled_top(&head)).ok_or(Miss::Gap)?;
    if !cover.complete {
        return Err(Miss::Incomplete);
    }
    if !fresh(&head, facts) {
        return Err(Miss::StaleHead);
    }
    Ok(Decision::Serve)
}

/// One message as upstream listed it: its id, its creation time, and a hash of its text. Never
/// the text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    id: String,
    created_ms: Option<i64>,
    hash: u64,
}

/// What upstream answered, as much of it as a comparison needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Upstream {
    rows: Vec<Seen>,
    bounded: bool,
}

impl Upstream {
    /// Upstream's answer: `messages`, raw, as the provider returned them, and `bounded` when the
    /// answer stops short of its scope's far end: older messages exist below a newest window or
    /// page, newer ones above a page walked forward, or a count stopped at its cap.
    #[must_use]
    pub fn of(messages: &[Message], bounded: bool) -> Self {
        Self {
            rows: messages
                .iter()
                .map(|message| Seen {
                    id: message.id.as_str().to_owned(),
                    created_ms: created_ms(message),
                    hash: crate::summarize::content_hash(&message.content),
                })
                .collect(),
            bounded,
        }
    }

    fn oldest(&self) -> Option<i64> {
        self.rows.iter().filter_map(|row| row.created_ms).min()
    }

    fn newest(&self) -> Option<i64> {
        self.rows.iter().filter_map(|row| row.created_ms).max()
    }
}

/// How the cache's answer differed from upstream's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Comparison {
    /// Messages upstream listed that the cache's answer lacked.
    pub missing: usize,
    /// Messages the cache's answer held that upstream no longer lists.
    pub extra: usize,
    /// Messages both listed whose text differs.
    pub differs: usize,
}

impl Comparison {
    /// Whether the two answers agreed.
    #[must_use]
    pub fn agreed(&self) -> bool {
        self.missing == 0 && self.extra == 0 && self.differs == 0
    }
}

/// Compare what the cache would have answered `ask` with, from `view`, against what upstream
/// answered, over the stretch both speak for. `None` when they share none: a message by id that
/// upstream's window does not reach, or a count either side stopped at its cap.
#[must_use]
pub fn compare(ask: &Ask, view: &View, upstream: &Upstream) -> Option<Comparison> {
    match ask {
        Ask::Window { limit, .. } | Ask::Timeline { limit, .. } => {
            // From the newer of the two oldest rows, for each answer whose older end was cut: a
            // row the cache misses at the top pushes an older one into its window that upstream's
            // has pushed out, and that older one is not a difference.
            let cache_cut = (view.rows.len() >= (*limit).max(1))
                .then(|| view.rows.first().map(|row| row.created_ms))
                .flatten();
            let upstream_cut = upstream.bounded.then(|| upstream.oldest()).flatten();
            let lower = cache_cut.max(upstream_cut).unwrap_or(i64::MIN);
            Some(differences(
                view.rows.iter().filter(|row| row.created_ms >= lower),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms >= lower)),
            ))
        }
        Ask::Message { .. } => {
            let row = view.rows.first()?;
            match upstream
                .rows
                .iter()
                .find(|seen| seen.id == row.message.id.as_str())
            {
                Some(seen) => Some(Comparison {
                    differs: usize::from(seen.hash != hash(row)),
                    ..Comparison::default()
                }),
                // Not listed: deleted, if upstream's answer reaches back to when it was written.
                None => (!upstream.bounded
                    || upstream
                        .oldest()
                        .is_some_and(|oldest| row.created_ms >= oldest))
                .then_some(Comparison {
                    extra: 1,
                    ..Comparison::default()
                }),
            }
        }
        Ask::Page { limit, .. } => {
            // Up to the older of the two last rows, for each answer that was cut at its newer end.
            let cache_cut = (view.rows.len() > *limit)
                .then(|| view.rows.last().map(|row| row.created_ms))
                .flatten();
            let upstream_cut = upstream.bounded.then(|| upstream.newest()).flatten();
            let upper = match (cache_cut, upstream_cut) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => i64::MAX,
            };
            Some(differences(
                view.rows.iter().filter(|row| row.created_ms <= upper),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms <= upper)),
            ))
        }
        Ask::Count { since_ms, cap } => {
            if upstream.bounded || view.rows.len() > *cap {
                return None;
            }
            let since = since_ms.unwrap_or(i64::MIN);
            Some(differences(
                view.rows.iter(),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms >= since)),
            ))
        }
    }
}

fn hash(row: &CachedMessage) -> u64 {
    crate::summarize::content_hash(&row.message.content)
}

fn differences<'a>(
    held: impl Iterator<Item = &'a CachedMessage>,
    listed: impl Iterator<Item = &'a Seen>,
) -> Comparison {
    let held: HashMap<&str, u64> = held
        .map(|row| (row.message.id.as_str(), hash(row)))
        .collect();
    let listed: HashMap<&str, u64> = listed.map(|seen| (seen.id.as_str(), seen.hash)).collect();
    let mut comparison = Comparison::default();
    for (id, listed_hash) in &listed {
        match held.get(id) {
            None => comparison.missing += 1,
            Some(held_hash) if held_hash != listed_hash => comparison.differs += 1,
            Some(_) => {}
        }
    }
    comparison.extra = held.keys().filter(|id| !listed.contains_key(*id)).count();
    comparison
}

/// One consumer's counters.
#[derive(Debug, Default)]
struct Tally {
    asked: AtomicU64,
    would_serve: AtomicU64,
    needs_existence_check: AtomicU64,
    misses: [AtomicU64; 7],
    late: AtomicU64,
    compared: AtomicU64,
    agreed: AtomicU64,
    missing_rows: AtomicU64,
    extra_rows: AtomicU64,
    content_differs: AtomicU64,
}

/// What the shadow has counted since the process started, per consumer. Memory only.
#[derive(Debug, Default)]
pub struct Tallies {
    consumers: [Tally; 10],
}

fn bump(counter: &AtomicU64, by: u64) {
    counter.fetch_add(by, Ordering::Relaxed);
}

fn read(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

fn widen(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

impl Tallies {
    fn tally(&self, consumer: Consumer) -> &Tally {
        &self.consumers[consumer as usize]
    }

    /// Count one evaluation for `consumer`, and what it decided.
    pub fn decided(&self, consumer: Consumer, decision: Decision) {
        let tally = self.tally(consumer);
        bump(&tally.asked, 1);
        let counter = match decision {
            Decision::Serve => &tally.would_serve,
            Decision::NeedsExistenceCheck => &tally.needs_existence_check,
            Decision::Late => &tally.late,
            Decision::Miss(miss) => &tally.misses[miss as usize],
        };
        bump(counter, 1);
    }

    /// Count one comparison for `consumer`.
    pub fn compared(&self, consumer: Consumer, comparison: &Comparison) {
        let tally = self.tally(consumer);
        bump(&tally.compared, 1);
        if comparison.agreed() {
            bump(&tally.agreed, 1);
        }
        bump(&tally.missing_rows, widen(comparison.missing));
        bump(&tally.extra_rows, widen(comparison.extra));
        bump(&tally.content_differs, widen(comparison.differs));
    }

    /// Every consumer's counts, in [`Consumer::ALL`]'s order.
    #[must_use]
    pub fn report(&self) -> Vec<ConsumerCounts> {
        Consumer::ALL
            .iter()
            .map(|&consumer| {
                let tally = self.tally(consumer);
                let miss = |miss: Miss| read(&tally.misses[miss as usize]);
                ConsumerCounts {
                    consumer: consumer.label(),
                    asked: read(&tally.asked),
                    would_serve: read(&tally.would_serve),
                    needs_existence_check: read(&tally.needs_existence_check),
                    misses: MissCounts {
                        off: miss(Miss::Off),
                        blocked: miss(Miss::Blocked),
                        suspect: miss(Miss::Suspect),
                        gap: miss(Miss::Gap),
                        incomplete: miss(Miss::Incomplete),
                        stale_head: miss(Miss::StaleHead),
                        barrier: miss(Miss::Barrier),
                    },
                    late: read(&tally.late),
                    compared: read(&tally.compared),
                    agreed: read(&tally.agreed),
                    missing_rows: read(&tally.missing_rows),
                    extra_rows: read(&tally.extra_rows),
                    content_differs: read(&tally.content_differs),
                }
            })
            .collect()
    }
}

/// One consumer's counts, as the diagnostics report carries them. Numbers and the consumer's
/// name only.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ConsumerCounts {
    /// [`Consumer::label`].
    pub consumer: &'static str,
    /// Evaluations.
    pub asked: u64,
    /// Those the cache would have answered.
    pub would_serve: u64,
    /// Those it would have answered after one existence check upstream.
    pub needs_existence_check: u64,
    /// Those it would not have, by reason.
    pub misses: MissCounts,
    /// Those that read what a later read wrote: no verdict.
    pub late: u64,
    /// Comparisons of a would-be answer with upstream's.
    pub compared: u64,
    /// Comparisons that found no difference.
    pub agreed: u64,
    /// Messages upstream listed that the cache's answers lacked.
    pub missing_rows: u64,
    /// Messages the cache's answers held that upstream no longer lists.
    pub extra_rows: u64,
    /// Messages both listed whose text differed.
    pub content_differs: u64,
}

impl ConsumerCounts {
    /// These counts and `other`'s, added: every consumer's, for one line.
    #[must_use]
    pub fn plus(&self, other: &Self) -> Self {
        Self {
            consumer: "all",
            asked: self.asked + other.asked,
            would_serve: self.would_serve + other.would_serve,
            needs_existence_check: self.needs_existence_check + other.needs_existence_check,
            misses: MissCounts {
                off: self.misses.off + other.misses.off,
                blocked: self.misses.blocked + other.misses.blocked,
                suspect: self.misses.suspect + other.misses.suspect,
                gap: self.misses.gap + other.misses.gap,
                incomplete: self.misses.incomplete + other.misses.incomplete,
                stale_head: self.misses.stale_head + other.misses.stale_head,
                barrier: self.misses.barrier + other.misses.barrier,
            },
            late: self.late + other.late,
            compared: self.compared + other.compared,
            agreed: self.agreed + other.agreed,
            missing_rows: self.missing_rows + other.missing_rows,
            extra_rows: self.extra_rows + other.extra_rows,
            content_differs: self.content_differs + other.content_differs,
        }
    }
}

/// Misses, by reason. See the module documentation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct MissCounts {
    /// No cache file open.
    pub off: u64,
    /// The channel or scope not read at all.
    pub blocked: u64,
    /// The channel's head not trusted.
    pub suspect: u64,
    /// Not held.
    pub gap: u64,
    /// Covered by reads that may have left a message out.
    pub incomplete: u64,
    /// Confirmed too long ago.
    pub stale_head: u64,
    /// The barrier did not pass in time.
    pub barrier: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::coverage::{Source, SETTLE_MS};
    use crate::cache::fixtures::message;

    /// 2026-08-18T12:00:00Z, provider time.
    const T: i64 = 1_787_054_400_000;
    const MINUTE: i64 = 60_000;
    const HOUR: i64 = 60 * MINUTE;
    /// When the read that listed everything below started, local time.
    const LISTED: i64 = T + HOUR;

    fn row(n: i64) -> CachedMessage {
        CachedMessage {
            message: message(
                &format!("m{n}"),
                "ada",
                &format!("body {n}"),
                T + n * MINUTE,
            ),
            created_ms: T + n * MINUTE,
            observed_ms: LISTED,
            content_partial: false,
        }
    }

    fn rows(range: std::ops::Range<i64>) -> Vec<CachedMessage> {
        range.map(row).collect()
    }

    fn interval(t0_ms: i64, t1_ms: i64, complete: bool) -> Interval {
        Interval {
            t0_ms,
            t1_ms,
            listed_ms: LISTED,
            full_ms: Some(LISTED),
            complete,
            source: Source::Read,
        }
    }

    /// The head a newest read of rows `0..n` leaves: its band the last two minutes.
    fn head(n: i64) -> Head {
        let head_ms = T + (n - 1) * MINUTE;
        Head {
            head_ms,
            unsettled_from_ms: head_ms - SETTLE_MS,
            head_listed_ms: LISTED,
        }
    }

    /// What a proven newest read of rows `0..n` leaves in the cache, `complete` or not.
    fn listed(n: i64, complete: bool) -> View {
        let head = head(n);
        View {
            head: Some(head),
            coverage: vec![interval(T - HOUR, head.unsettled_from_ms, complete)],
            rows: rows(0..n),
            home: None,
        }
    }

    fn facts(asked_ms: i64) -> Facts {
        Facts {
            asked_ms,
            live_fed: false,
            suspect: false,
            barrier: true,
            feed: Feed::AdditionsOnly,
        }
    }

    fn window(limit: usize) -> Ask {
        Ask::Window {
            consumer: Consumer::Messages,
            limit,
        }
    }

    fn newest(view: &View, limit: usize) -> View {
        View {
            rows: view.rows[view.rows.len().saturating_sub(limit)..].to_vec(),
            ..view.clone()
        }
    }

    #[test]
    fn the_reuse_periods_are_the_recent_windows_own() {
        assert_eq!(
            Duration::from_millis(FRESH_UNFED_MS.unsigned_abs()),
            crate::recent::REUSE_UNFED
        );
        assert_eq!(
            Duration::from_millis(FRESH_LIVE_FED_MS.unsigned_abs()),
            crate::recent::REUSE_LIVE_FED
        );
    }

    #[test]
    fn a_covered_head_listed_moments_ago_would_serve_its_window() {
        let view = newest(&listed(12, false), 5);
        assert_eq!(
            decide(&window(5), &view, &facts(LISTED + 5_000)),
            Decision::Serve
        );
        // The rows inside the band need no claim of their own: the head's read listed them.
        let band = newest(&listed(12, false), 2);
        assert_eq!(
            decide(&window(2), &band, &facts(LISTED + 5_000)),
            Decision::Serve
        );
    }

    #[test]
    fn a_head_is_stale_past_fifteen_seconds_and_past_a_minute_when_live_fed() {
        let view = newest(&listed(12, false), 5);
        let stale = Decision::Miss(Miss::StaleHead);
        assert_eq!(
            decide(&window(5), &view, &facts(LISTED + FRESH_UNFED_MS)),
            Decision::Serve
        );
        assert_eq!(
            decide(&window(5), &view, &facts(LISTED + FRESH_UNFED_MS + 1)),
            stale
        );
        let fed = Facts {
            live_fed: true,
            ..facts(LISTED + FRESH_UNFED_MS + 1)
        };
        assert_eq!(decide(&window(5), &view, &fed), Decision::Serve);
        let fed = Facts {
            live_fed: true,
            ..facts(LISTED + FRESH_LIVE_FED_MS + 1)
        };
        assert_eq!(decide(&window(5), &view, &fed), stale);
    }

    #[test]
    fn a_gap_a_suspect_channel_a_missing_head_and_a_barrier_each_miss() {
        let at = facts(LISTED + 1_000);
        let mut holed = newest(&listed(12, false), 5);
        // A hole under the fourth-newest row.
        holed.coverage = vec![
            interval(T - HOUR, T + 8 * MINUTE, false),
            interval(T + 8 * MINUTE + 1, head(12).unsettled_from_ms, false),
        ];
        assert_eq!(decide(&window(5), &holed, &at), Decision::Miss(Miss::Gap));
        // The hole is below what a window of three needs.
        assert_eq!(decide(&window(3), &newest(&holed, 3), &at), Decision::Serve);
        let view = newest(&listed(12, false), 5);
        let suspect = Facts {
            suspect: true,
            ..at
        };
        assert_eq!(
            decide(&window(5), &view, &suspect),
            Decision::Miss(Miss::Suspect)
        );
        let headless = View {
            head: None,
            ..view.clone()
        };
        assert_eq!(
            decide(&window(5), &headless, &at),
            Decision::Miss(Miss::Gap)
        );
        let late_barrier = Facts {
            barrier: false,
            ..at
        };
        assert_eq!(
            decide(&window(5), &view, &late_barrier),
            Decision::Miss(Miss::Barrier)
        );
        // Fewer rows than asked would be the whole scope only from its start, which no claim
        // reaches.
        assert_eq!(
            decide(&window(20), &listed(12, false), &at),
            Decision::Miss(Miss::Gap)
        );
    }

    #[test]
    fn a_message_by_id_follows_its_providers_feed() {
        let held = |full_ms: i64| View {
            head: None,
            coverage: vec![Interval {
                full_ms: Some(full_ms),
                ..interval(T - HOUR, T + HOUR, false)
            }],
            rows: vec![row(3)],
            home: Some(Scope::Main),
        };
        let ask = Ask::Message {
            consumer: Consumer::MessageById,
            id: MessageId("m3".to_owned()),
        };
        let slack = facts(LISTED);
        assert_eq!(
            decide(&ask, &held(LISTED - REVALIDATED_ADDITIONS_ONLY_MS), &slack),
            Decision::Serve
        );
        assert_eq!(
            decide(
                &ask,
                &held(LISTED - REVALIDATED_ADDITIONS_ONLY_MS - 1),
                &slack
            ),
            Decision::NeedsExistenceCheck
        );
        let bridge = Facts {
            feed: Feed::Deletions,
            ..slack
        };
        assert_eq!(
            decide(&ask, &held(LISTED - 3 * HOUR), &bridge),
            Decision::Serve
        );
        assert_eq!(
            decide(&ask, &held(LISTED - REVALIDATED_BRIDGE_MS - 1), &bridge),
            Decision::Miss(Miss::StaleHead)
        );
        let fed = Facts {
            live_fed: true,
            ..bridge
        };
        assert_eq!(
            decide(&ask, &held(LISTED - REVALIDATED_BRIDGE_MS - 1), &fed),
            Decision::Serve
        );
        let unclaimed = View {
            coverage: Vec::new(),
            ..held(LISTED)
        };
        assert_eq!(decide(&ask, &unclaimed, &bridge), Decision::Miss(Miss::Gap));
        assert_eq!(
            decide(&ask, &unclaimed, &slack),
            Decision::NeedsExistenceCheck
        );
        assert_eq!(
            decide(&ask, &View::default(), &slack),
            Decision::Miss(Miss::Gap)
        );
        // A suspect channel: an offer dropped for it may have been this message's deletion or
        // edit, so neither a feed that carries deletions nor a recent check vouches for it.
        for trusting in [fed, bridge, slack] {
            let suspect = Facts {
                suspect: true,
                ..trusting
            };
            assert_eq!(
                decide(&ask, &held(LISTED), &suspect),
                Decision::Miss(Miss::Suspect),
                "{trusting:?}"
            );
        }
    }

    #[test]
    fn a_page_needs_complete_coverage_and_a_count_never_serves_from_slacks() {
        let at = facts(LISTED + 1_000);
        let page = Ask::Page {
            since_ms: T,
            until_ms: Some(T + 5 * MINUTE),
            limit: 20,
        };
        let span = |complete: bool| View {
            rows: rows(0..5),
            ..listed(12, complete)
        };
        assert_eq!(decide(&page, &span(true), &at), Decision::Serve);
        assert_eq!(
            decide(&page, &span(false), &at),
            Decision::Miss(Miss::Incomplete)
        );
        // A full page needs the coverage only up to its last row.
        let full = Ask::Page {
            since_ms: T,
            until_ms: None,
            limit: 3,
        };
        let four = View {
            rows: rows(0..4),
            ..listed(12, true)
        };
        assert_eq!(decide(&full, &four, &at), Decision::Serve);
        // One that runs to the newest end needs the band settled.
        let open = Ask::Page {
            since_ms: T,
            until_ms: None,
            limit: 20,
        };
        assert_eq!(decide(&open, &span(true), &at), Decision::Miss(Miss::Gap));

        let count = Ask::Count {
            since_ms: Some(T),
            cap: 100,
        };
        assert_eq!(decide(&count, &listed(12, true), &at), Decision::Serve);
        assert_eq!(
            decide(&count, &listed(12, false), &at),
            Decision::Miss(Miss::Incomplete)
        );
        assert_eq!(
            decide(
                &count,
                &listed(12, true),
                &facts(LISTED + FRESH_UNFED_MS + 1)
            ),
            Decision::Miss(Miss::StaleHead)
        );
        let whole = Ask::Count {
            since_ms: None,
            cap: 100,
        };
        assert_eq!(
            decide(&whole, &listed(12, true), &at),
            Decision::Miss(Miss::Gap)
        );
    }

    #[test]
    fn a_fallback_page_needs_no_fresh_head_and_slacks_flat_view_is_never_asked() {
        let ask = Ask::Timeline {
            view: TimelineView::Main,
            thread: None,
            limit: 5,
        };
        let view = newest(&listed(12, false), 5);
        assert_eq!(
            decide(&ask, &view, &facts(LISTED + 10 * HOUR)),
            Decision::Serve
        );
        let flat = Ask::Timeline {
            view: TimelineView::Flat,
            thread: None,
            limit: 5,
        };
        assert_eq!(query(&flat, Shape::Slack, false), None);
        assert_eq!(query(&flat, Shape::DiscordNative, false), None);
        assert_eq!(
            query(&flat, Shape::Bridge, false).and_then(|q| q.scope().cloned()),
            Some(Scope::Flat)
        );
        let threads = Ask::Timeline {
            view: TimelineView::Threads,
            thread: None,
            limit: 5,
        };
        assert_eq!(query(&threads, Shape::Bridge, false), None);
        let thread = Ask::Timeline {
            view: TimelineView::Thread,
            thread: Some("t1".to_owned()),
            limit: 5,
        };
        assert_eq!(
            query(&thread, Shape::Slack, false).and_then(|q| q.scope().cloned()),
            Some(Scope::Thread("t1".to_owned()))
        );
        assert_eq!(
            query(&thread, Shape::SlackThread, false).and_then(|q| q.scope().cloned()),
            Some(Scope::Main)
        );
    }

    #[test]
    fn a_bridges_window_is_asked_of_flat_only_when_it_is_read_through_flat() {
        let ask = window(5);
        let scope = |shape, via_flat| query(&ask, shape, via_flat).and_then(|q| q.scope().cloned());
        assert_eq!(scope(Shape::Bridge, true), Some(Scope::Flat));
        assert_eq!(scope(Shape::Bridge, false), Some(Scope::Main));
        assert_eq!(scope(Shape::Slack, false), Some(Scope::Main));
        assert_eq!(scope(Shape::DiscordNative, true), Some(Scope::Main));
    }

    #[test]
    fn a_snapshot_holding_what_a_later_read_wrote_is_late() {
        let view = listed(12, false);
        assert!(!late(&view, LISTED + 1));
        assert!(
            late(&view, LISTED),
            "the head was listed by a read begun at the ask"
        );
        let mut pushed = view.clone();
        pushed.head = Some(Head {
            head_listed_ms: LISTED - 1,
            ..head(12)
        });
        pushed.coverage[0].listed_ms = LISTED - 1;
        assert!(late(&pushed, LISTED), "a row was seen at the ask");
        for row in &mut pushed.rows {
            row.observed_ms = LISTED - 1;
        }
        assert!(!late(&pushed, LISTED));
    }

    fn upstream(range: std::ops::Range<i64>, bounded: bool) -> (Vec<Message>, bool) {
        (range.map(|n| row(n).message).collect(), bounded)
    }

    fn compared(ask: &Ask, view: &View, (messages, bounded): (Vec<Message>, bool)) -> Comparison {
        compare(ask, view, &Upstream::of(&messages, bounded)).expect("compared")
    }

    #[test]
    fn a_row_upstream_no_longer_lists_is_extra_and_one_the_cache_lacks_is_missing() {
        let ask = window(5);
        let view = newest(&listed(12, false), 5);
        assert!(compared(&ask, &view, upstream(7..12, true)).agreed());
        // Row 9 deleted upstream: its window reaches one row further back.
        let (mut messages, _) = upstream(6..12, true);
        messages.retain(|m| m.id.as_str() != "m9");
        let comparison = compared(&ask, &view, (messages, true));
        assert_eq!(
            comparison,
            Comparison {
                extra: 1,
                ..Comparison::default()
            },
            "row 6, which the cache's window has no room for, is no difference"
        );
        // A row newer than the cache's head pushes row 7 out of upstream's window: missing the
        // new one, and row 7 no difference.
        let mut newer = upstream(8..12, true).0;
        newer.push(row(12).message);
        assert_eq!(
            compared(&ask, &view, (newer, true)),
            Comparison {
                missing: 1,
                ..Comparison::default()
            }
        );
        // An edit.
        let mut edited = upstream(7..12, true).0;
        edited[2].content = "edited".to_owned();
        assert_eq!(
            compared(&ask, &view, (edited, true)),
            Comparison {
                differs: 1,
                ..Comparison::default()
            }
        );
    }

    #[test]
    fn a_message_by_id_is_extra_only_where_upstreams_window_reaches() {
        let ask = Ask::Message {
            consumer: Consumer::MessageById,
            id: MessageId("m3".to_owned()),
        };
        let view = View {
            rows: vec![row(3)],
            ..View::default()
        };
        assert!(compared(&ask, &view, upstream(0..10, true)).agreed());
        assert_eq!(compared(&ask, &view, upstream(4..10, false)).extra, 1);
        let (mut reached, _) = upstream(0..10, true);
        reached.retain(|m| m.id.as_str() != "m3");
        assert_eq!(compared(&ask, &view, (reached, true)).extra, 1);
        let (messages, bounded) = upstream(4..10, true);
        assert_eq!(
            compare(&ask, &view, &Upstream::of(&messages, bounded)),
            None,
            "older than the window upstream read: nothing to say"
        );
    }

    #[test]
    fn a_page_is_compared_up_to_the_older_of_the_two_last_rows() {
        let ask = Ask::Page {
            since_ms: T,
            until_ms: None,
            limit: 3,
        };
        let view = View {
            rows: rows(0..4),
            ..View::default()
        };
        assert!(compared(&ask, &view, upstream(0..4, true)).agreed());
        // Upstream lacks row 1: it reaches row 4 instead, which the cache's page had no room for.
        let (mut messages, _) = upstream(0..5, true);
        messages.remove(1);
        assert_eq!(
            compared(&ask, &view, (messages, true)),
            Comparison {
                extra: 1,
                ..Comparison::default()
            }
        );
    }

    #[test]
    fn a_count_either_side_capped_is_not_compared() {
        let ask = Ask::Count {
            since_ms: Some(T + 2 * MINUTE),
            cap: 10,
        };
        let view = View {
            rows: rows(2..6),
            ..View::default()
        };
        assert!(compared(&ask, &view, upstream(0..6, false)).agreed());
        let (messages, _) = upstream(0..6, true);
        assert_eq!(compare(&ask, &view, &Upstream::of(&messages, true)), None);
    }

    #[test]
    fn the_tallies_count_asks_verdicts_and_rows() {
        let tallies = Tallies::default();
        tallies.decided(Consumer::Page, Decision::Serve);
        tallies.decided(Consumer::Page, Decision::Miss(Miss::Gap));
        tallies.decided(Consumer::Summary, Decision::NeedsExistenceCheck);
        tallies.decided(Consumer::Summary, Decision::Late);
        tallies.compared(
            Consumer::Page,
            &Comparison {
                extra: 2,
                missing: 1,
                differs: 0,
            },
        );
        tallies.compared(Consumer::Page, &Comparison::default());
        let report = tallies.report();
        assert_eq!(report.len(), Consumer::ALL.len());
        let page = report.iter().find(|c| c.consumer == "page").expect("page");
        assert_eq!((page.asked, page.would_serve, page.misses.gap), (2, 1, 1));
        assert_eq!(
            (
                page.compared,
                page.agreed,
                page.extra_rows,
                page.missing_rows
            ),
            (2, 1, 2, 1)
        );
        let summary = report
            .iter()
            .find(|c| c.consumer == "summary")
            .expect("summary");
        assert_eq!(
            (summary.asked, summary.needs_existence_check, summary.late),
            (2, 1, 1)
        );
        let all = report
            .iter()
            .fold(ConsumerCounts::default(), |sum, c| sum.plus(c));
        assert_eq!((all.asked, all.extra_rows), (4, 2));
        let labels: Vec<&str> = Miss::ALL.iter().map(|m| m.label()).collect();
        assert_eq!(
            labels,
            [
                "off",
                "blocked",
                "suspect",
                "gap",
                "incomplete",
                "stale_head",
                "barrier"
            ]
        );
    }
}
