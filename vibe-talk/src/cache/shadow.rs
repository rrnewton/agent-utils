//! What the message cache serves, or would have served, measured beside the reads that go
//! upstream. `#208 persistent-cache`, milestones `cache-fill` and `cache-serve` (`#384
//! cache-serve`).
//!
//! Every read the cache could answer is evaluated by the rules below: the cache is asked what
//! it holds for the same question, the rule decides whether it would answer, and once upstream
//! has answered, the two are compared. Counts only, in memory, from the moment the process
//! starts. With `[message_cache] serve` off that is all that happens — a shadow. With it on, two
//! families of write-scope reads are answered from the snapshot the evaluation read (see
//! [`serve_eligible`]), upstream is read behind the answer, and the comparison then checks the
//! answer that was actually handed back, counted `served` beside `would_serve`.
//!
//! This module is the pure part: the rules, per consumer, and the comparison. The evaluation
//! runs off the request's path ([`super::Sink::shadow`]): its reads on the blocking pool while
//! upstream is answering, its decision and comparison in a detached task once upstream has. The
//! one exception is serving ([`super::Sink::try_serve`]), which awaits the same evaluation on
//! the request's path, briefly, because the answer depends on it. Neither writes, whoever
//! asked: a read-scope caller is shadowed as much as any other, and leaves the file exactly as
//! it was; serving its reads is for a later milestone.
//!
//! # The rules
//!
//! | Consumer | Would serve when |
//! |---|---|
//! | `messages`, `digest`, `todo`, `bankruptcy`, `speech`: the callers of `ops::messages` | the scope's coverage is contiguous from the head down to `limit` rows, the band is vouched for (the stretch served at or above the head's settled top, which no claim holds, lies wholly in what the latest read of the scope's newest end listed, from where its listing began up; every row served there was listed by that read, whatever else has seen it since; and none lies above the head), the latest read of the scope's newest end began at most [`FRESH_UNFED_MS`] before the ask ([`FRESH_LIVE_FED_MS`] when a live producer feeds the channel), and so was every claim under the window below the settled top (edits and deletions reflected as of then), unless the channel's feed carries edits and deletions, an adapter pushes them (not the live poller, which publishes creations alone), and they have reached the file since that claim was read, nothing served is in doubt (see below) or held only as a push brought it (see `gap`), and the barrier passed |
//! | `message_by_id`, `summary` | the message is held whole (not as a push brought it: see `gap`) and not in doubt (an offer dropped for its channel since it was seen may have been its edit or deletion, whatever its feed carries), and the barrier passed, and: on a bridge, whose feed carries deletions, the coverage of its home scope was revalidated within [`REVALIDATED_BRIDGE_MS`], or the channel is fed by an adapter's pushes (the live poller publishes creations alone) and the message was seen, or that coverage revalidated, since the feed's changes have reached the file: since this process's file could take them and the route list first reached it (a deletion made before then, while the server was down or the list unknown, reached nothing; a channel put back on the list, or added to it, is in doubt until read again, since nothing of it reached the file while it was off); on Slack and Discord, whose feeds carry only additions, within [`REVALIDATED_ADDITIONS_ONLY_MS`], and otherwise it would take one existence check upstream first (`needs_existence_check`) |
//! | `page` | the span asked for lies inside complete coverage (up to the page's last row, when it is full), the band is vouched for as for `messages`, no row of it is in doubt or held only as a push brought it, and the barrier passed; an open-ended page needs a fresh head, not in doubt, as well |
//! | `count` | complete coverage spans `[since, head − 120 s]` (so never on Slack, whose coverage is never complete), the band is vouched for as for `messages`, the head and the claims counted are as fresh as for `messages`, nothing counted is in doubt or held only as a push brought it (the comparison checks the text too), and the barrier passed |
//! | `timeline_fallback` | coverage holds `limit` rows down from the head, the band is vouched for as for `messages` (so a row pushed or posted above the head is never served), nothing served is in doubt or held only as a push brought it, and the barrier passed: a newest page answered from the cache at once, however old its head, because the read behind it corrects it. Slack's Flat view never |
//!
//! # What a miss says
//!
//! Checked in this order, and counted under the first that applies:
//!
//! - `off`: no cache file is open, though the configuration has a `[message_cache]` table;
//! - `blocked`: the cache would not read the channel or the scope at all: it is not on the route
//!   snapshot, an erase of it is waiting, access to it is in doubt, its provider is suspended, or
//!   the read failed;
//! - `suspect`: something the answer rests on is in doubt. An offer for the channel was dropped
//!   (the queue was full, or its provider's account was not checked against the file), the file
//!   forgot or never took what the feed delivered for it (an erase of the whole channel, a change
//!   such an erase discarded, a change made while the channel was off the route list), in this
//!   process or, as the file records, an earlier one; and a row the answer would hold has not been
//!   seen by a read that began after the latest drop, or the answer needs the scope's head and no
//!   read of that scope's newest end began after it. A read lifts the doubt about exactly what it
//!   saw: a thread's listing says nothing of Main, nor a two-row page of the rows below it;
//! - `gap`: the cache does not hold what would be served: no head, a hole in the coverage, a stretch
//!   of the unsettled band below where the latest read of the scope's newest end began listing, a
//!   row in the band that read did not list, a row above the head, or not the message; or it holds
//!   a row's text only as a push brought it, which may be a snippet (a bridge sends one when its
//!   live event has no body), no read having proven it whole since (merge rule 5);
//! - `incomplete`: covered, but by reads that may have left a message out (Slack's);
//! - `stale_head`: what the cache holds was confirmed too long ago: the head, the claims under a
//!   window or a count, or the coverage of a message's home scope on a bridge;
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
//! An answer resting on a scope's head claims every message written before the end of its claim
//! ([`claim_end`]): the later of the head itself (the newest message the latest read of the newest
//! end listed: that read began at the answer's version, [`version`], which a served page reports,
//! `#473 replication-c7-server-versions`, and may have listed messages written while it ran) and
//! that version, plus [`CLOCK_MARGIN_MS`] for the provider's clock against the cache's. Upstream
//! is read after the ask, and its read takes seconds, so it may list messages written past that
//! end: ones the rule's freshness allowed the answer not to hold, and ones written after the ask,
//! while upstream was being read. Neither is a row the answer claimed, and each is counted apart
//! (`#497 cache-shadow-missing-row`).
//!
//! - `compared` and `agreed`: comparisons made, and those that found no difference;
//! - `missing_rows`: messages upstream listed that the cache's answer lacked, written before the
//!   end of its claim (or with no time to tell): rows the answer claimed to hold, each a wrong
//!   answer had it been served;
//! - `stale_rows`: messages upstream listed that the cache's answer lacked, written past the end
//!   of its claim but before the ask. The answer never claimed them, so they are no difference,
//!   but they existed when the reader asked: what the rule's freshness let a served answer leave
//!   out until the next read brings them;
//! - `newer_rows`: messages upstream listed that the cache's answer lacked, written past the end
//!   of its claim and after the ask: no difference at all;
//! - `extra_rows`: messages the cache's answer held that upstream no longer lists. Served, each
//!   would have brought a deleted message back;
//! - `content_differs`: messages both listed whose text differs.
//!
//! The row counts are summed over comparisons. A comparison that finds any of them is written to
//! the log, ids and times only ([`super::sink`]).

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
/// How recently an adapter must have pushed a change for a provider for that provider's feed to
/// vouch for what it has not reported (`#384 cache-serve`, requirement (g) of `#297
/// cache-coverage-races`). The ingest protocol has no heartbeat, so a push for any of the
/// provider's channels is the sign of life; an hour of silence is taken as no adapter, and the
/// channel is left to the freshness rules alone.
pub const FED_PUSH_RECENT_MS: i64 = 60 * 60 * 1000;

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
    /// `/timeline`'s newest page, as a page answered from the cache at once and refreshed
    /// behind. Kept under its shadow-era name in the report.
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

/// Whether a serving build may answer `ask` of a channel of `shape` from the cache at all,
/// before the rule is consulted (`#384 cache-serve`). An ineligible ask is still shadowed and
/// counted; it is simply never the answer. In this milestone:
///
/// - a newest **window**, except on Slack: a Slack window upstream carries thread replies the
///   cache holds only in their threads, so it may be answered from Main plus those threads'
///   scopes or not at all (requirement (c) of `#297 cache-coverage-races`), and M9 does not;
/// - a newest **timeline page** of Main, or of Flat on a bridge: the page-load path. One
///   thread's page carries its root and summary, which rows alone cannot rebuild, and its rows
///   on Slack may be replies Flat's memory dated (requirement (e)); the thread list is never a
///   scope's listing;
/// - nothing else: a message by id, a summary's message, a page and a count keep their
///   upstream answers (and their shadow counts) for a later milestone.
#[must_use]
pub fn serve_eligible(ask: &Ask, shape: Shape) -> bool {
    match ask {
        Ask::Window { .. } => shape != Shape::Slack,
        Ask::Timeline { view, .. } => match view {
            TimelineView::Main => true,
            TimelineView::Flat => shape == Shape::Bridge,
            TimelineView::Thread | TimelineView::Threads => false,
        },
        Ask::Message { .. } | Ask::Page { .. } | Ask::Count { .. } => false,
    }
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
    /// When the latest read of that scope's newest end the file records began: the head's own
    /// listing time, or later.
    pub head_checked_ms: Option<i64>,
    /// Where that read's listing began, provider time: it listed every message it saw from there
    /// up. `None` when the file does not say, and then it vouches for no stretch of the band.
    pub head_checked_from_ms: Option<i64>,
    /// The coverage of the scope read, or of the message's home scope.
    pub coverage: Vec<Interval>,
    /// The messages read, oldest first.
    pub rows: Vec<CachedMessage>,
    /// The message's home scope, for a query of one message the cache holds.
    pub home: Option<Scope>,
    /// Whether the file holds any thread of the channel: what a served newest page answers for
    /// `has_threads`. `#384 cache-serve`.
    pub has_threads: bool,
    /// The latest local time an offer for the channel was dropped, as the file records it.
    pub dropped_ms: Option<i64>,
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
    /// Since when every change the live feed offered for the channel reached the file or was
    /// recorded as left out (a pushed edit whose text the file keeps out among them): from when
    /// this process's file could take offers and the route list first reached it
    /// ([`super::MessageCache::fed_since_ms`]). A live feed vouches for nothing seen before it,
    /// and for nothing but that a message exists and its text is unchanged: never that a text a
    /// push brought, which may be a snippet, is whole. `None` when not known, or when nothing
    /// feeding the channel ever delivers a deletion or an edit (its provider is polled: the poller
    /// publishes creations alone, and the ingest route refuses an adapter's pushes for it): then
    /// for nothing.
    pub fed_since_ms: Option<i64>,
    /// The latest local time an offer for the channel was dropped in this process, if one was.
    /// The rule takes the later of this and the file's [`View::dropped_ms`].
    pub dropped_ms: Option<i64>,
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
            .head_checked_ms
            .is_some_and(|checked| checked >= asked_ms)
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

/// The latest drop of an offer for the channel: by this process's memory, or by the file.
fn dropped(view: &View, facts: &Facts) -> Option<i64> {
    match (view.dropped_ms, facts.dropped_ms) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Whether a row is vouched for after the latest drop: seen by a read that began after it. Anything
/// older may be the row whose edit or deletion was the offer dropped.
fn trusted(row: &CachedMessage, dropped: Option<i64>) -> bool {
    dropped.is_none_or(|dropped| row.observed_ms > dropped)
}

/// Whether the cache holds a row's text whole: no push brought it since a read last proved it
/// (merge rule 5). A text a push brought may be a snippet, and is never handed back as the message.
fn whole(row: &CachedMessage) -> bool {
    !row.content_partial
}

/// Whether the scope's head is trusted after the latest drop: a read of the scope's newest end
/// began after it, and would have listed a creation the drop left out. No head is not a doubt.
fn head_trusted(view: &View, dropped: Option<i64>) -> bool {
    match (view.head, dropped) {
        (Some(head), Some(dropped)) => {
            view.head_checked_ms.unwrap_or(head.head_listed_ms) > dropped
        }
        _ => true,
    }
}

/// How long what a read saw stands without being read again: the recent windows' own reuse
/// periods, the longer when a live producer feeds the channel.
fn reuse_ms(facts: &Facts) -> i64 {
    if facts.live_fed {
        FRESH_LIVE_FED_MS
    } else {
        FRESH_UNFED_MS
    }
}

/// Whether the scope's newest end was read within the reuse period: no creation is missing from
/// its top. Judged by the latest read of the newest end the file records, not by the head's own
/// listing time: while a band is open and nothing newer has come, the head keeps the time of the
/// read that opened the band, which is what the band's ten minutes are counted from, and a quiet
/// channel read every few seconds would otherwise look stale for those ten minutes.
fn fresh(view: &View, head: &Head, facts: &Facts) -> bool {
    facts.asked_ms.saturating_sub(checked(view, head)) <= reuse_ms(facts)
}

/// When the latest read of the scope's newest end the file records began, local time.
fn checked(view: &View, head: &Head) -> i64 {
    view.head_checked_ms
        .unwrap_or(head.head_listed_ms)
        .max(head.head_listed_ms)
}

/// What an answer from `view` stands as of, local time: when the latest read of the scope's newest
/// end began, which listed every message it could see from where its listing began up, and below
/// which claims hold the rest. `None` for an answer resting on no head: a message by id. `#497
/// cache-shadow-missing-row`.
#[must_use]
pub fn version(view: &View) -> Option<i64> {
    view.head.map(|head| checked(view, &head))
}

/// How far the provider's clock may run ahead of the cache's, for [`claim_end`]: a message written
/// this much after the answer's version, by the provider's clock, may have been written before it.
pub const CLOCK_MARGIN_MS: i64 = 1_000;

/// Where what an answer from `view` claims to hold ends, provider time, exclusive: past its head,
/// the newest message the latest read of the newest end listed, which that read may have listed
/// while it ran, after it began; and past its version, by [`CLOCK_MARGIN_MS`] more. Every message
/// written before it that the answer lacks is one it claimed to hold. `None` with no head.
#[must_use]
pub fn claim_end(view: &View) -> Option<i64> {
    let head = view.head?;
    Some(
        checked(view, &head)
            .saturating_add(CLOCK_MARGIN_MS)
            .max(head.head_ms.saturating_add(1)),
    )
}

/// Whether the claims a window or a count rests on still stand: their edits and deletions were
/// reflected as of `full_ms` (the weakest of them), and that was within the reuse period, or the
/// live feed has carried every edit and deletion since. A fresh head says only that the newest end
/// was read; the rows below it rest on claims that may be hours old, and on a feed that carries
/// no edits or deletions (or none to a channel nothing feeds), what changed since never arrived.
fn current(full_ms: Option<i64>, facts: &Facts) -> bool {
    full_ms.is_some_and(|full_ms| {
        facts.asked_ms.saturating_sub(full_ms) <= reuse_ms(facts) || fed_since(full_ms, facts)
    })
}

/// Whether the live feed has carried every edit and deletion since local time `at_ms`: the
/// provider's feed carries them, a live producer feeds the channel and delivers them (an
/// adapter's pushes: [`Facts::fed_since_ms`] is `None` for a polled channel), and the feed's
/// current run (from when the file could take offers in this process and the route list first
/// reached it) began no later than `at_ms`. A change made before the run began (while the server
/// was down, say) reached nothing, and no drop records it. One the feed delivered and the file did
/// not take (an edit whose text it kept out, say) is a drop, which each rule checks apart
/// ([`trusted`]).
fn fed_since(at_ms: i64, facts: &Facts) -> bool {
    facts.feed == Feed::Deletions
        && facts.live_fed
        && facts.fed_since_ms.is_some_and(|since| at_ms >= since)
}

/// Whether what a consumer hands back at or above the head's settled top is vouched for: the one
/// rule each consumer that may hand back such rows applies, a newest window or page, a page and a
/// count alike. `(from, to)` is the stretch it hands back, provider time, from `from` up to `to`
/// (exclusive), or to the newest end when `to` is `None`; `rows` are the rows it read there.
///
/// No claim holds the unsettled band, nor anything above the head: only the latest read of the
/// scope's newest end does, and only for what it listed. So the part of the stretch inside the band
/// must lie wholly in what that read listed, from where its listing began up: below that, a message
/// it never saw may be missing however many rows above it are vouched for. Every row there must be
/// one that read listed, its membership's `vouched_ms` that read's start: a row it did not list was
/// deleted since an earlier read saw it (even one that landed after it), or was pushed, posted or
/// read by id before the provider listed it, and a sighting of any of those kinds says the row
/// exists, never that nothing beside it is missing. And no row may lie above the head: whatever was
/// written there since that read (a post of this server's own, an hour on) stands beside messages
/// nothing has read. Below the settled top a claim holds every row, and with no head there is no
/// band: such rows rest on the coverage, which each rule checks for itself.
fn band_vouched<'a>(
    view: &View,
    (from, to): (i64, Option<i64>),
    rows: impl IntoIterator<Item = &'a CachedMessage>,
) -> bool {
    let Some(head) = view.head else {
        return true;
    };
    let top = head.settled_top();
    let checked = view.head_checked_ms.unwrap_or(head.head_listed_ms);
    // The lowest instant of the stretch the band holds, when the stretch reaches the band at all.
    let lower = from.max(top);
    let in_band = lower <= head.head_ms && to.is_none_or(|to| lower < to);
    if in_band
        && view
            .head_checked_from_ms
            .is_none_or(|listed_from| listed_from > lower)
    {
        return false;
    }
    rows.into_iter().all(|row| {
        row.created_ms < top
            || (row.created_ms <= head.head_ms
                && row.vouched_ms.is_some_and(|vouched| vouched >= checked))
    })
}

/// A newest window, or a newest page: coverage contiguous from the head down to `limit` rows.
fn newest(view: &View, limit: usize, facts: &Facts, needs_fresh: bool) -> Result<Decision, Miss> {
    let dropped = dropped(view, facts);
    let window = &view.rows[view.rows.len().saturating_sub(limit.max(1))..];
    if !head_trusted(view, dropped) || !window.iter().all(|row| trusted(row, dropped)) {
        return Err(Miss::Suspect);
    }
    if !window.iter().all(whole) {
        return Err(Miss::Gap);
    }
    let head = view.head.ok_or(Miss::Gap)?;
    // A window of fewer rows than asked is the whole scope only from its very start, which no
    // coverage reaches: it is clipped at the age limit.
    let bottom = if view.rows.len() >= limit.max(1) {
        view.rows[view.rows.len() - limit.max(1)].created_ms
    } else {
        0
    };
    let top = head.settled_top();
    let cover = if bottom < top {
        Some(coverage::covers(&view.coverage, bottom, top).ok_or(Miss::Gap)?)
    } else {
        None
    };
    if !band_vouched(view, (bottom, None), window) {
        return Err(Miss::Gap);
    }
    // Fresh, the window as a whole: its head, and the claims under it.
    if needs_fresh
        && (!fresh(view, &head, facts) || cover.is_some_and(|cover| !current(cover.full_ms, facts)))
    {
        return Err(Miss::StaleHead);
    }
    Ok(Decision::Serve)
}

/// One message: held whole, seen since the channel's latest drop, and its home scope revalidated
/// recently enough for its provider's feed. A live feed that carries deletions vouches for a
/// message only when nothing it offered since the message was seen was dropped, and only when it
/// has been running since the message was seen, or its home scope revalidated: a deletion made
/// before then reached nothing.
fn message(view: &View, facts: &Facts) -> Result<Decision, Miss> {
    let row = view.rows.first().ok_or(Miss::Gap)?;
    if !trusted(row, dropped(view, facts)) {
        return Err(Miss::Suspect);
    }
    if !whole(row) {
        return Err(Miss::Gap);
    }
    let revalidated = view
        .coverage
        .iter()
        .find(|interval| interval.t0_ms <= row.created_ms && row.created_ms < interval.t1_ms)
        .and_then(|interval| interval.full_ms);
    let within = |limit_ms: i64| {
        revalidated.is_some_and(|full_ms| facts.asked_ms.saturating_sub(full_ms) <= limit_ms)
    };
    let fed = fed_since(
        revalidated.map_or(row.observed_ms, |full_ms| full_ms.max(row.observed_ms)),
        facts,
    );
    match facts.feed {
        Feed::Deletions if fed || within(REVALIDATED_BRIDGE_MS) => Ok(Decision::Serve),
        Feed::Deletions if revalidated.is_none() => Err(Miss::Gap),
        Feed::Deletions => Err(Miss::StaleHead),
        Feed::AdditionsOnly if within(REVALIDATED_ADDITIONS_ONLY_MS) => Ok(Decision::Serve),
        Feed::AdditionsOnly => Ok(Decision::NeedsExistenceCheck),
    }
}

/// A page walked forward from `since`: its span inside complete coverage, and every row it read
/// above that coverage (an open page reads on past the head) vouched for by the latest read of the
/// newest end.
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
    let dropped = dropped(view, facts);
    if (open && !head_trusted(view, dropped)) || !view.rows.iter().all(|row| trusted(row, dropped))
    {
        return Err(Miss::Suspect);
    }
    if !view.rows.iter().all(whole) {
        return Err(Miss::Gap);
    }
    let end = if full {
        view.rows[limit].created_ms.saturating_add(1)
    } else if let Some(until) = until {
        until
    } else {
        view.head.ok_or(Miss::Gap)?.head_ms.saturating_add(1)
    };
    let cover = coverage::covers(&view.coverage, since, end).ok_or(Miss::Gap)?;
    if !band_vouched(view, (since, Some(end)), &view.rows) {
        return Err(Miss::Gap);
    }
    if !cover.complete {
        return Err(Miss::Incomplete);
    }
    if open && !view.head.is_some_and(|head| fresh(view, &head, facts)) {
        return Err(Miss::StaleHead);
    }
    Ok(Decision::Serve)
}

/// A count from `since`: complete coverage up to the unsettled band, every row counted at or above
/// it vouched for by the latest read of the newest end, current, under a fresh head.
fn count(view: &View, since: i64, facts: &Facts) -> Result<Decision, Miss> {
    let dropped = dropped(view, facts);
    if !head_trusted(view, dropped) || !view.rows.iter().all(|row| trusted(row, dropped)) {
        return Err(Miss::Suspect);
    }
    if !view.rows.iter().all(whole) {
        return Err(Miss::Gap);
    }
    let head = view.head.ok_or(Miss::Gap)?;
    let cover = coverage::covers(&view.coverage, since, head.settled_top()).ok_or(Miss::Gap)?;
    if !band_vouched(view, (since, None), &view.rows) {
        return Err(Miss::Gap);
    }
    if !cover.complete {
        return Err(Miss::Incomplete);
    }
    if !fresh(view, &head, facts) || !current(cover.full_ms, facts) {
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
    /// Messages upstream listed that the cache's answer lacked, written before the end of its claim
    /// ([`claim_end`]), or with no time to tell: rows it claimed to hold.
    pub missing: usize,
    /// Messages upstream listed that the cache's answer lacked, written past the end of its claim
    /// but before the ask: rows it never claimed, which existed when the reader asked. `#497
    /// cache-shadow-missing-row`.
    pub stale: usize,
    /// Messages upstream listed that the cache's answer lacked, written past the end of its claim
    /// and after the ask: rows it never claimed.
    pub newer: usize,
    /// Messages the cache's answer held that upstream no longer lists.
    pub extra: usize,
    /// Messages both listed whose text differs.
    pub differs: usize,
}

impl Comparison {
    /// Whether the two answers agreed: nothing missing, extra or different. A message written
    /// past the end of the answer's claim is no difference.
    #[must_use]
    pub fn agreed(&self) -> bool {
        self.missing == 0 && self.extra == 0 && self.differs == 0
    }
}

/// One message a comparison found the two answers differ on: its id and times, never its text.
/// `#497 cache-shadow-missing-row`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Differing {
    /// The message's id.
    pub id: String,
    /// When it was written, provider time, when known.
    pub created_ms: Option<i64>,
    /// What the cache's answer held of it: `None` for a message the answer lacked.
    pub held: Option<HeldRow>,
}

/// What the cache's answer held of a message, times and flags only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldRow {
    /// When a read last saw it, local time.
    pub observed_ms: i64,
    /// When the latest read of the scope's newest end to list it began, when one did.
    pub vouched_ms: Option<i64>,
    /// Whether its text is held only as a push brought it.
    pub partial: bool,
}

/// What a comparison found, message by message, and the stretch it compared: what the shadow's
/// diagnostics write beside the counts. `#497 cache-shadow-missing-row`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    /// Upstream listed them, the answer lacked them, and they were written before the end of its
    /// claim.
    pub missing: Vec<Differing>,
    /// Upstream listed them, the answer lacked them, and they were written past the end of its
    /// claim, before the ask.
    pub stale: Vec<Differing>,
    /// Upstream listed them, the answer lacked them, and they were written past the end of its
    /// claim, after the ask.
    pub newer: Vec<Differing>,
    /// The answer held them, and upstream no longer lists them.
    pub extra: Vec<Differing>,
    /// Both listed them, with different text.
    pub differs: Vec<Differing>,
    /// The oldest instant compared, provider time; `None` from the scope's start.
    pub from_ms: Option<i64>,
    /// The newest instant compared, provider time, inclusive; `None` to the newest end.
    pub until_ms: Option<i64>,
    /// What the answer stands as of ([`version`]), local time.
    pub version_ms: Option<i64>,
    /// Where what it claims ends ([`claim_end`]), provider time.
    pub claim_end_ms: Option<i64>,
    /// How many messages upstream answered with, its oldest and newest times, and whether its
    /// answer stopped short of the scope's far end.
    pub upstream_rows: usize,
    /// The oldest of them, provider time.
    pub upstream_oldest_ms: Option<i64>,
    /// The newest of them, provider time.
    pub upstream_newest_ms: Option<i64>,
    /// Whether upstream's answer stopped short of its scope's far end.
    pub upstream_bounded: bool,
}

/// Compare what the cache would have answered `ask`, asked at local time `asked_ms`, with, from
/// `view`, against what upstream answered, over the stretch both speak for. `None` when they share
/// none: a message by id that upstream's window does not reach, or a count either side stopped at
/// its cap.
#[must_use]
pub fn compare(ask: &Ask, view: &View, upstream: &Upstream, asked_ms: i64) -> Option<Comparison> {
    compare_detail(ask, view, upstream, asked_ms).map(|(comparison, _)| comparison)
}

/// [`compare`], and what it found message by message. `#497 cache-shadow-missing-row`.
#[must_use]
pub fn compare_detail(
    ask: &Ask,
    view: &View,
    upstream: &Upstream,
    asked_ms: i64,
) -> Option<(Comparison, Detail)> {
    let version_ms = version(view);
    let past = Past {
        claim_end_ms: claim_end(view),
        asked_ms,
    };
    let (comparison, mut detail) = match ask {
        Ask::Window { limit, .. } | Ask::Timeline { limit, .. } => {
            // From the newer of the two oldest rows, for each answer whose older end was cut: a
            // row the cache misses at the top pushes an older one into its window that upstream's
            // has pushed out, and that older one is not a difference.
            let cache_cut = (view.rows.len() >= (*limit).max(1))
                .then(|| view.rows.first().map(|row| row.created_ms))
                .flatten();
            let upstream_cut = upstream.bounded.then(|| upstream.oldest()).flatten();
            let lower = cache_cut.max(upstream_cut);
            let from = lower.unwrap_or(i64::MIN);
            let mut detail = differences(
                view.rows.iter().filter(|row| row.created_ms >= from),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms >= from)),
                past,
            );
            detail.from_ms = lower;
            (detail.counts(), detail)
        }
        Ask::Message { .. } => {
            let row = view.rows.first()?;
            let found = upstream
                .rows
                .iter()
                .find(|seen| seen.id == row.message.id.as_str());
            let mut detail = Detail::default();
            match found {
                Some(seen) if seen.hash != hash(row) => detail.differs.push(held(row)),
                Some(_) => {}
                // Not listed: deleted, if upstream's answer reaches back to when it was written.
                None if !upstream.bounded
                    || upstream
                        .oldest()
                        .is_some_and(|oldest| row.created_ms >= oldest) =>
                {
                    detail.extra.push(held(row));
                }
                None => return None,
            }
            (detail.counts(), detail)
        }
        Ask::Page { limit, .. } => {
            // Up to the older of the two last rows, for each answer that was cut at its newer end.
            let cache_cut = (view.rows.len() > *limit)
                .then(|| view.rows.last().map(|row| row.created_ms))
                .flatten();
            let upstream_cut = upstream.bounded.then(|| upstream.newest()).flatten();
            let upper = match (cache_cut, upstream_cut) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            };
            let until = upper.unwrap_or(i64::MAX);
            let mut detail = differences(
                view.rows.iter().filter(|row| row.created_ms <= until),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms <= until)),
                past,
            );
            detail.until_ms = upper;
            (detail.counts(), detail)
        }
        Ask::Count { since_ms, cap } => {
            if upstream.bounded || view.rows.len() > *cap {
                return None;
            }
            let since = since_ms.unwrap_or(i64::MIN);
            let mut detail = differences(
                view.rows.iter(),
                upstream
                    .rows
                    .iter()
                    .filter(|seen| seen.created_ms.is_none_or(|ms| ms >= since)),
                past,
            );
            detail.from_ms = *since_ms;
            (detail.counts(), detail)
        }
    };
    detail.version_ms = version_ms;
    detail.claim_end_ms = past.claim_end_ms;
    detail.upstream_rows = upstream.rows.len();
    detail.upstream_oldest_ms = upstream.oldest();
    detail.upstream_newest_ms = upstream.newest();
    detail.upstream_bounded = upstream.bounded;
    Some((comparison, detail))
}

impl Detail {
    fn counts(&self) -> Comparison {
        Comparison {
            missing: self.missing.len(),
            stale: self.stale.len(),
            newer: self.newer.len(),
            extra: self.extra.len(),
            differs: self.differs.len(),
        }
    }
}

fn hash(row: &CachedMessage) -> u64 {
    crate::summarize::content_hash(&row.message.content)
}

fn held(row: &CachedMessage) -> Differing {
    Differing {
        id: row.message.id.as_str().to_owned(),
        created_ms: Some(row.created_ms),
        held: Some(HeldRow {
            observed_ms: row.observed_ms,
            vouched_ms: row.vouched_ms,
            partial: row.content_partial,
        }),
    }
}

/// Where a message the answer lacked falls: before the end of its claim, or past it and before or
/// after the ask.
#[derive(Clone, Copy, Debug)]
struct Past {
    claim_end_ms: Option<i64>,
    asked_ms: i64,
}

/// The differences between what the cache held and what upstream listed, over the same stretch.
/// A message upstream listed and the cache lacked is `missing` when it was written before the end
/// of the answer's claim (or has no time, or the answer no head); past it, `stale` when written
/// before the ask, and `newer` from then on.
fn differences<'a>(
    held_rows: impl Iterator<Item = &'a CachedMessage>,
    listed: impl Iterator<Item = &'a Seen>,
    past: Past,
) -> Detail {
    let held_rows: HashMap<&str, &CachedMessage> = held_rows
        .map(|row| (row.message.id.as_str(), row))
        .collect();
    let listed: HashMap<&str, &Seen> = listed.map(|seen| (seen.id.as_str(), seen)).collect();
    let mut detail = Detail::default();
    for (id, seen) in &listed {
        match held_rows.get(id) {
            None => {
                let lacked = Differing {
                    id: (*id).to_owned(),
                    created_ms: seen.created_ms,
                    held: None,
                };
                match seen.created_ms.zip(past.claim_end_ms) {
                    Some((created, end)) if created >= end => {
                        if created < past.asked_ms {
                            detail.stale.push(lacked);
                        } else {
                            detail.newer.push(lacked);
                        }
                    }
                    _ => detail.missing.push(lacked),
                }
            }
            Some(row) if hash(row) != seen.hash => detail.differs.push(held(row)),
            Some(_) => {}
        }
    }
    detail.extra = held_rows
        .iter()
        .filter(|(id, _)| !listed.contains_key(*id))
        .map(|(_, row)| held(row))
        .collect();
    for list in [
        &mut detail.missing,
        &mut detail.stale,
        &mut detail.newer,
        &mut detail.extra,
        &mut detail.differs,
    ] {
        list.sort_by(|a, b| (a.created_ms, &a.id).cmp(&(b.created_ms, &b.id)));
    }
    detail
}

/// One consumer's counters.
#[derive(Debug, Default)]
struct Tally {
    asked: AtomicU64,
    would_serve: AtomicU64,
    served: AtomicU64,
    needs_existence_check: AtomicU64,
    misses: [AtomicU64; 7],
    late: AtomicU64,
    compared: AtomicU64,
    agreed: AtomicU64,
    missing_rows: AtomicU64,
    stale_rows: AtomicU64,
    newer_rows: AtomicU64,
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

    /// Count one answer actually served from the cache for `consumer` (`#384 cache-serve`): a
    /// subset of `would_serve`, counted when the answer is handed back, so the two can be told
    /// apart on a server with serving on.
    pub fn served(&self, consumer: Consumer) {
        bump(&self.tally(consumer).served, 1);
    }

    /// Count one comparison for `consumer`.
    pub fn compared(&self, consumer: Consumer, comparison: &Comparison) {
        let tally = self.tally(consumer);
        bump(&tally.compared, 1);
        if comparison.agreed() {
            bump(&tally.agreed, 1);
        }
        bump(&tally.missing_rows, widen(comparison.missing));
        bump(&tally.stale_rows, widen(comparison.stale));
        bump(&tally.newer_rows, widen(comparison.newer));
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
                    served: read(&tally.served),
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
                    stale_rows: read(&tally.stale_rows),
                    newer_rows: read(&tally.newer_rows),
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
    /// Of those, the ones it did answer (`[message_cache] serve`). `#384 cache-serve`.
    pub served: u64,
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
    /// Messages upstream listed that the cache's answers lacked, written before the end of each
    /// answer's claim: rows those answers claimed to hold.
    pub missing_rows: u64,
    /// Messages upstream listed that the cache's answers lacked, written past the end of each
    /// answer's claim and before its ask: no difference, but what a served answer would have left
    /// out. `#497 cache-shadow-missing-row`.
    pub stale_rows: u64,
    /// Messages upstream listed that the cache's answers lacked, written past the end of each
    /// answer's claim and after its ask: no difference.
    pub newer_rows: u64,
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
            served: self.served + other.served,
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
            stale_rows: self.stale_rows + other.stale_rows,
            newer_rows: self.newer_rows + other.newer_rows,
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
    /// When a comparison's request asked, local time.
    const ASKED: i64 = LISTED + 5_000;

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
            vouched_ms: Some(LISTED),
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

    /// What a proven newest read of rows `0..n`, listing from an hour before the first, leaves in
    /// the cache, `complete` or not.
    fn listed(n: i64, complete: bool) -> View {
        let head = head(n);
        View {
            head: Some(head),
            head_checked_from_ms: Some(T - HOUR),
            coverage: vec![interval(T - HOUR, head.unsettled_from_ms, complete)],
            rows: rows(0..n),
            ..View::default()
        }
    }

    fn facts(asked_ms: i64) -> Facts {
        Facts {
            asked_ms,
            live_fed: false,
            fed_since_ms: None,
            dropped_ms: None,
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

    /// What the latest read of the newest end leaves, `later`: it began listing at `listed_from`,
    /// and listed every row of the band but `missing`.
    fn relisted(view: &View, later: i64, listed_from: i64, missing: &str) -> View {
        let mut view = view.clone();
        view.head_checked_ms = Some(later);
        view.head_checked_from_ms = Some(listed_from);
        let top = view.head.expect("a head").settled_top();
        for row in &mut view.rows {
            if row.created_ms >= top
                && row.created_ms >= listed_from
                && row.message.id.as_str() != missing
            {
                row.observed_ms = later;
                row.vouched_ms = Some(later);
            }
        }
        view
    }

    fn fallback(limit: usize) -> Ask {
        Ask::Timeline {
            view: TimelineView::Main,
            thread: None,
            limit,
        }
    }

    /// A later read of the newest end listed every row of the band but m10: m10 was deleted since
    /// the head's read saw it, or a read that saw it landed late, or it was pushed and not listed
    /// yet. Nothing vouches for it, and no window holding it is served; nor one holding a row
    /// pushed above the head, whenever it was seen.
    #[test]
    fn a_band_row_the_latest_read_of_the_newest_end_did_not_list_is_never_served() {
        let later = LISTED + MINUTE;
        let at = facts(later + 1_000);
        let view = relisted(&newest(&listed(12, false), 5), later, T - HOUR, "m10");
        assert_eq!(
            decide(&fallback(5), &view, &at),
            Decision::Miss(Miss::Gap),
            "m10 was not listed by the latest read"
        );
        // A window above it, all listed by that read, is.
        assert_eq!(
            decide(&fallback(1), &newest(&view, 1), &at),
            Decision::Serve
        );
        // Below the band, claims hold the rows, whenever they were seen.
        let all = relisted(&newest(&listed(12, false), 5), later, T - HOUR, "");
        assert_eq!(decide(&fallback(5), &all, &at), Decision::Serve);
        // A row pushed above the head is held by nothing that says what else was written beside
        // it: not vouched for, seen before the latest read began or after it.
        let mut pushed = all.clone();
        let mut above = row(12);
        above.observed_ms = later - 1;
        above.vouched_ms = None;
        pushed.rows.push(above.clone());
        pushed.rows.remove(0);
        assert_eq!(
            decide(&fallback(5), &pushed, &at),
            Decision::Miss(Miss::Gap)
        );
        pushed.rows.last_mut().expect("a row").observed_ms = later + 1;
        assert_eq!(
            decide(&fallback(5), &pushed, &at),
            Decision::Miss(Miss::Gap)
        );
        // Nor an hour on, as a newest page answered from the cache would be.
        assert_eq!(
            decide(&fallback(5), &pushed, &facts(later + HOUR)),
            Decision::Miss(Miss::Gap)
        );
    }

    /// A row of the band seen after the latest read of the newest end began, by anything but that
    /// read (a read of it by id, a short page, a post), says that it exists, and nothing of what
    /// lies beside it: a message that read never listed may be missing below it. And a stretch of
    /// the band below where that read began listing is vouched for by nothing, however many rows
    /// above it are.
    #[test]
    fn a_band_row_seen_since_the_latest_read_but_not_listed_by_it_never_closes_a_gap() {
        let later = LISTED + 5_000;
        let at = facts(later + 1_000);
        // The band is the head's last two minutes: m8 lies below it, m9, m10 and m11 in it.
        let view = newest(&listed(12, false), 4);
        let top = view.head.expect("a head").settled_top();
        assert!(row(8).created_ms < top && row(9).created_ms >= top);
        // The latest read listed only m11, from m11 up; m10 was read alone afterwards.
        let mut short = relisted(&view, later, row(11).created_ms, "");
        let m10 = short
            .rows
            .iter_mut()
            .find(|row| row.message.id.as_str() == "m10")
            .expect("m10");
        m10.observed_ms = later + 500;
        assert_eq!(m10.vouched_ms, Some(LISTED), "only an older read listed it");
        for ask in [window(2), fallback(2)] {
            assert_eq!(
                decide(&ask, &newest(&short, 2), &at),
                Decision::Miss(Miss::Gap),
                "{ask:?}: m10 was not listed by the latest read"
            );
        }
        // Were every row vouched for by it after all, the stretch of the band below m11 is still
        // not one that read listed: a message written between m8 and m11 may be missing, whether
        // the window's oldest row lies in the band or below it.
        for row in &mut short.rows {
            row.vouched_ms = Some(later);
        }
        for (ask, limit) in [
            (window(3), 3),
            (fallback(3), 3),
            (window(4), 4),
            (fallback(4), 4),
        ] {
            assert_eq!(
                decide(&ask, &newest(&short, limit), &at),
                Decision::Miss(Miss::Gap),
                "{ask:?}: the band below where the latest read began listing"
            );
        }
        // A window inside what it listed is served.
        assert_eq!(decide(&window(1), &newest(&short, 1), &at), Decision::Serve);
        // A head whose read's start is not recorded (written before cache schema 3) vouches for no
        // stretch of the band at all.
        let unrecorded = View {
            head_checked_from_ms: None,
            ..short.clone()
        };
        assert_eq!(
            decide(&window(1), &newest(&unrecorded, 1), &at),
            Decision::Miss(Miss::Gap)
        );
        // Listed from below the band, the whole window is served.
        let whole = relisted(&view, later, T - HOUR, "");
        for ask in [window(4), fallback(4)] {
            assert_eq!(decide(&ask, &whole, &at), Decision::Serve, "{ask:?}");
        }
    }

    /// The band rule is one rule: a count and a page hand back the rows of the band, and the rows
    /// above the head, as a newest window does, and none of them counts or pages a row there the
    /// latest read of the newest end did not list. A count reads on past the band to the newest
    /// end; an open page past the head, even once the band has settled and the coverage reaches
    /// the head itself.
    #[test]
    fn a_count_or_a_page_never_hands_back_a_band_row_the_latest_read_did_not_list() {
        let later = LISTED + 5_000;
        let at = facts(later + 1_000);
        let count = Ask::Count {
            since_ms: Some(T),
            cap: 100,
        };
        // Twelve rows; the latest read of the newest end, five seconds on, listed all but m10.
        let mut view = listed(12, true);
        view.coverage[0].full_ms = Some(later);
        let view = relisted(&view, later, T - HOUR, "m10");
        assert_eq!(decide(&count, &view, &at), Decision::Miss(Miss::Gap));
        let listed_all = relisted(&view, later, T - HOUR, "");
        assert_eq!(decide(&count, &listed_all, &at), Decision::Serve);
        // A row pushed above the head, before that read began or after: never counted.
        let mut pushed = listed_all.clone();
        pushed.rows.push(CachedMessage {
            observed_ms: later - 1,
            vouched_ms: None,
            ..row(12)
        });
        assert_eq!(decide(&count, &pushed, &at), Decision::Miss(Miss::Gap));
        pushed.rows.last_mut().expect("a row").observed_ms = later;
        assert_eq!(decide(&count, &pushed, &at), Decision::Miss(Miss::Gap));
        // Nor one that read listed from above where the band begins, every row of it vouched for.
        let mut short = relisted(&listed_all, later + 1, T + 11 * MINUTE, "");
        for row in &mut short.rows {
            row.vouched_ms = Some(later + 1);
        }
        assert_eq!(
            decide(&count, &short, &facts(later + 1_001)),
            Decision::Miss(Miss::Gap)
        );

        // The band settled by time: the settled top is just past the head, and the coverage
        // reaches it. A read that began before the latest one lands late, with a message deleted
        // since, above the head: an open page from m8 holds it, and is not served.
        let head_ms = T + 11 * MINUTE;
        let settled = View {
            head: Some(Head {
                head_ms,
                unsettled_from_ms: head_ms + 1,
                head_listed_ms: later,
            }),
            head_checked_ms: Some(later),
            head_checked_from_ms: Some(T - HOUR),
            coverage: vec![Interval {
                full_ms: Some(later),
                listed_ms: later,
                ..interval(T - HOUR, head_ms + 1, true)
            }],
            rows: rows(8..12),
            ..View::default()
        };
        let open = Ask::Page {
            since_ms: T + 8 * MINUTE,
            until_ms: None,
            limit: 20,
        };
        assert_eq!(decide(&open, &settled, &at), Decision::Serve);
        let mut resurrected = settled.clone();
        resurrected.rows.push(CachedMessage {
            observed_ms: later - 30_000,
            vouched_ms: None,
            ..row(12)
        });
        assert_eq!(decide(&open, &resurrected, &at), Decision::Miss(Miss::Gap));
        assert_eq!(decide(&count, &resurrected, &at), Decision::Miss(Miss::Gap));
        // A page bounded below the head never reads it, and stands on the coverage alone.
        let bounded = Ask::Page {
            since_ms: T + 8 * MINUTE,
            until_ms: Some(head_ms + 1),
            limit: 20,
        };
        assert_eq!(decide(&bounded, &settled, &at), Decision::Serve);
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

    /// A short read of the newest end makes the head fresh; the rows below its band rest on claims,
    /// whose edits and deletions are reflected only as of their own reads. A window or a count
    /// that reaches a claim older than the reuse period is stale, unless a live feed that carries
    /// edits and deletions has run since that claim was read.
    #[test]
    fn a_window_or_a_count_resting_on_a_claim_older_than_the_reuse_period_is_stale() {
        let claimed_at = |full_ms: i64| {
            let mut view = listed(12, true);
            view.coverage[0].listed_ms = full_ms;
            view.coverage[0].full_ms = Some(full_ms);
            view
        };
        let at = facts(LISTED + 1_000);
        let stale = Decision::Miss(Miss::StaleHead);
        let count = Ask::Count {
            since_ms: Some(T),
            cap: 100,
        };
        let hour_old = claimed_at(LISTED - HOUR);
        assert_eq!(decide(&window(5), &newest(&hour_old, 5), &at), stale);
        assert_eq!(decide(&count, &hour_old, &at), stale);
        // A window inside the band rests on no claim: the head's read listed it.
        assert_eq!(
            decide(&window(2), &newest(&hour_old, 2), &at),
            Decision::Serve
        );
        // A newest page answered from the cache when upstream fails says how old it is instead.
        let fallback = Ask::Timeline {
            view: TimelineView::Main,
            thread: None,
            limit: 5,
        };
        assert_eq!(
            decide(&fallback, &newest(&hour_old, 5), &at),
            Decision::Serve
        );
        // Within the reuse period, a claim stands.
        let just = claimed_at(LISTED + 1_000 - FRESH_UNFED_MS);
        assert_eq!(decide(&window(5), &newest(&just, 5), &at), Decision::Serve);
        assert_eq!(decide(&count, &just, &at), Decision::Serve);
        let past = claimed_at(LISTED + 1_000 - FRESH_UNFED_MS - 1);
        assert_eq!(decide(&window(5), &newest(&past, 5), &at), stale);
        // A bridge's live feed has carried every edit and deletion since it began: a claim read
        // after that stands, however old.
        let fed = Facts {
            live_fed: true,
            fed_since_ms: Some(LISTED - 2 * HOUR),
            feed: Feed::Deletions,
            ..at
        };
        assert_eq!(
            decide(&window(5), &newest(&hour_old, 5), &fed),
            Decision::Serve
        );
        assert_eq!(decide(&count, &hour_old, &fed), Decision::Serve);
        // Not one read before it began, nor on a feed that carries only additions.
        let restarted = Facts {
            fed_since_ms: Some(LISTED - HOUR + 1),
            ..fed
        };
        let additions = Facts {
            feed: Feed::AdditionsOnly,
            ..fed
        };
        for facts in [restarted, additions] {
            assert_eq!(
                decide(&window(5), &newest(&hour_old, 5), &facts),
                stale,
                "{facts:?}"
            );
            assert_eq!(decide(&count, &hour_old, &facts), stale, "{facts:?}");
        }
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
            dropped_ms: Some(LISTED),
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
    fn a_drop_leaves_in_doubt_only_the_rows_and_heads_no_read_has_seen_since() {
        let at = facts(LISTED + 1_000);
        let view = newest(&listed(12, true), 5);
        let after = |dropped_ms: i64| Facts {
            dropped_ms: Some(dropped_ms),
            ..at
        };
        let suspect = Decision::Miss(Miss::Suspect);
        // A drop before everything was seen leaves nothing in doubt; one after, everything.
        assert_eq!(
            decide(&window(5), &view, &after(LISTED - 1)),
            Decision::Serve
        );
        assert_eq!(decide(&window(5), &view, &after(LISTED)), suspect);
        // The file's record of an earlier process's drop counts as this process's own does.
        let recorded = View {
            dropped_ms: Some(LISTED),
            ..view.clone()
        };
        assert_eq!(decide(&window(5), &recorded, &at), suspect);
        // A read of the newest two rows, begun after the drop, vouches for those two and for the
        // head, and for nothing below them.
        let seen = |mut view: View, newest: usize| {
            let from = view.rows.len().saturating_sub(newest);
            let listed_from = view.rows[from].created_ms;
            for row in &mut view.rows[from..] {
                row.observed_ms = LISTED + 20;
                row.vouched_ms = Some(LISTED + 20);
            }
            View {
                head_checked_ms: Some(LISTED + 20),
                head_checked_from_ms: Some(listed_from),
                ..view
            }
        };
        let two = seen(recorded.clone(), 2);
        assert_eq!(decide(&window(2), &newest(&two, 2), &at), Decision::Serve);
        assert_eq!(decide(&window(5), &two, &at), suspect);
        // Every row seen again, but the head not: a creation the drop left out may be missing.
        let rows_only = View {
            head_checked_ms: Some(LISTED),
            ..seen(recorded.clone(), 5)
        };
        assert_eq!(decide(&window(5), &rows_only, &at), suspect);
        assert_eq!(
            decide(&window(5), &seen(recorded.clone(), 5), &at),
            Decision::Serve
        );
        // One message is judged by itself: m7, below the band, seen again or not.
        let ask = Ask::Message {
            consumer: Consumer::MessageById,
            id: MessageId("m7".to_owned()),
        };
        let message = |view: &View| View {
            rows: view.rows.first().cloned().into_iter().collect(),
            head: None,
            head_checked_ms: None,
            ..view.clone()
        };
        let all = seen(recorded.clone(), 5);
        assert_eq!(decide(&ask, &message(&all), &at), Decision::Serve);
        assert_eq!(decide(&ask, &message(&two), &at), suspect);
        // A page or a count holding a row in doubt would serve it too.
        let page = Ask::Page {
            since_ms: T,
            until_ms: Some(T + 5 * MINUTE),
            limit: 20,
        };
        let span = View {
            rows: rows(0..5),
            ..listed(12, true)
        };
        assert_eq!(decide(&page, &span, &at), Decision::Serve);
        assert_eq!(decide(&page, &span, &after(LISTED)), suspect);
        let count = Ask::Count {
            since_ms: Some(T),
            cap: 100,
        };
        assert_eq!(decide(&count, &listed(12, true), &after(LISTED)), suspect);
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
            ..View::default()
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
        // A live feed that carries deletions vouches for the message, however long ago its home
        // scope was revalidated, when it has run since the message was seen.
        let fed = Facts {
            live_fed: true,
            fed_since_ms: Some(LISTED - HOUR),
            ..bridge
        };
        assert_eq!(
            decide(&ask, &held(LISTED - REVALIDATED_BRIDGE_MS - 1), &fed),
            Decision::Serve
        );
        // Not when it began after: a deletion made before (while the server was down) reached
        // nothing. Then the twenty-four hours decide.
        let restarted = Facts {
            fed_since_ms: Some(LISTED + 1),
            ..fed
        };
        assert_eq!(
            decide(&ask, &held(LISTED - REVALIDATED_BRIDGE_MS - 1), &restarted),
            Decision::Miss(Miss::StaleHead)
        );
        assert_eq!(
            decide(&ask, &held(LISTED - 3 * HOUR), &restarted),
            Decision::Serve
        );
        // A home scope revalidated since the feed began vouches as a sighting does.
        let reread = View {
            rows: vec![CachedMessage {
                observed_ms: LISTED - 2 * REVALIDATED_BRIDGE_MS,
                ..row(3)
            }],
            ..held(LISTED + 2)
        };
        assert_eq!(
            decide(
                &ask,
                &reread,
                &Facts {
                    asked_ms: LISTED + REVALIDATED_BRIDGE_MS + 3,
                    ..restarted
                }
            ),
            Decision::Serve
        );
        // Nor does a feed that does not say when it began, nor one that carries no deletions.
        for unknown in [
            Facts {
                fed_since_ms: None,
                ..fed
            },
            Facts {
                feed: Feed::AdditionsOnly,
                ..fed
            },
        ] {
            assert_ne!(
                decide(&ask, &held(LISTED - REVALIDATED_BRIDGE_MS - 1), &unknown),
                Decision::Serve,
                "{unknown:?}"
            );
        }
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
                dropped_ms: Some(LISTED),
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
        compare(ask, view, &Upstream::of(&messages, bounded), ASKED).expect("compared")
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

    /// `#497 cache-shadow-missing-row`. An answer claims every message written before the end of
    /// its claim: past its version (when the latest read of the newest end began) by the clock
    /// margin, and past its head. A message upstream lists that the answer lacks is `missing`
    /// before that end; past it, `stale` when written before the ask (the freshness the rule
    /// allows) and `newer` after it (written while upstream was being read): neither a difference.
    #[test]
    fn a_lacked_message_is_missing_up_to_the_end_of_the_claim_then_stale_then_newer() {
        let ask = window(5);
        let later = LISTED + 20_000;
        let asked = later + 10_000;
        let view = View {
            head_checked_ms: Some(later),
            ..newest(&listed(12, false), 5)
        };
        assert_eq!(version(&view), Some(later));
        let end = later + CLOCK_MARGIN_MS;
        assert_eq!(
            claim_end(&view),
            Some(end),
            "the head lies long before the read"
        );
        let lacking = |view: &View, created_ms: Option<i64>| {
            let mut new = row(12).message;
            new.timestamp = created_ms.map_or_else(String::new, crate::clock::iso_from_ms);
            let mut messages = upstream(8..12, true).0;
            messages.push(new);
            compare_detail(&ask, view, &Upstream::of(&messages, true), asked).expect("compared")
        };
        let (stale, detail) = lacking(&view, Some(end));
        assert_eq!(
            stale,
            Comparison {
                stale: 1,
                ..Comparison::default()
            },
            "written past the claim, before the ask"
        );
        assert!(stale.agreed());
        assert_eq!(detail.stale[0].id, "m12");
        assert_eq!(
            (detail.version_ms, detail.claim_end_ms),
            (Some(later), Some(end))
        );
        assert_eq!(
            detail.from_ms,
            Some(row(8).created_ms),
            "upstream's window was cut there"
        );
        assert_eq!(
            lacking(&view, Some(asked)).0,
            Comparison {
                newer: 1,
                ..Comparison::default()
            },
            "written after the ask, while upstream was read"
        );
        let (inside, detail) = lacking(&view, Some(end - 1));
        assert_eq!(
            inside,
            Comparison {
                missing: 1,
                ..Comparison::default()
            },
            "inside the clock margin: the provider's clock may run ahead"
        );
        assert!(!inside.agreed());
        assert_eq!(
            detail.missing[0].held, None,
            "the answer held nothing of it"
        );
        assert_eq!(
            lacking(&view, None).0.missing,
            1,
            "no time to tell: a row the answer may have claimed"
        );
        // The read began before messages it then listed were written: the head lies past the
        // version, and a message written between the two that it did not list (listed late) is
        // one the answer claimed, the head being newer.
        let mut ran_long = view.clone();
        ran_long.head = ran_long.head.map(|head| Head {
            head_ms: later + 5_000,
            unsettled_from_ms: later + 5_000 - SETTLE_MS,
            ..head
        });
        assert_eq!(claim_end(&ran_long), Some(later + 5_001));
        assert_eq!(
            lacking(&ran_long, Some(later + 3_000)).0.missing,
            1,
            "older than the head the answer ends with"
        );
        assert_eq!(lacking(&ran_long, Some(later + 5_001)).0.stale, 1);
        // With no head there is no claim to end: every lacked message is missing.
        let headless = View {
            head: None,
            ..view.clone()
        };
        let mut messages = upstream(8..12, true).0;
        messages.push(row(40).message);
        let (comparison, detail) =
            compare_detail(&ask, &headless, &Upstream::of(&messages, true), asked)
                .expect("compared");
        assert_eq!(
            (comparison.missing, comparison.stale, comparison.newer),
            (1, 0, 0)
        );
        assert_eq!(detail.claim_end_ms, None);
    }

    /// What a comparison found, message by message: ids and times, and for what the answer held,
    /// when it was seen, vouched for, and whether a push brought its text. Never the text.
    #[test]
    fn a_comparisons_detail_names_each_message_by_id_and_times() {
        let ask = window(5);
        let mut view = newest(&listed(12, false), 5);
        view.rows[1].content_partial = true;
        let mut messages = upstream(7..12, true).0;
        messages.retain(|m| m.id.as_str() != "m9");
        messages[0].content = "edited".to_owned();
        let (comparison, detail) =
            compare_detail(&ask, &view, &Upstream::of(&messages, true), ASKED).expect("compared");
        assert_eq!((comparison.extra, comparison.differs), (1, 1));
        assert_eq!(detail.extra[0].id, "m9");
        assert_eq!(
            detail.extra[0].held,
            Some(HeldRow {
                observed_ms: LISTED,
                vouched_ms: Some(LISTED),
                partial: false,
            })
        );
        assert_eq!(detail.differs[0].id, "m7");
        assert_eq!((detail.upstream_rows, detail.upstream_bounded), (4, true));
        assert_eq!(detail.upstream_newest_ms, Some(row(11).created_ms));
        let by_id = Ask::Message {
            consumer: Consumer::MessageById,
            id: MessageId("m8".to_owned()),
        };
        let one = View {
            rows: vec![view.rows[1].clone()],
            ..View::default()
        };
        let (_, detail) =
            compare_detail(&by_id, &one, &Upstream::of(&[], false), ASKED).expect("compared");
        assert_eq!(detail.extra[0].id, "m8");
        assert!(detail.extra[0].held.is_some_and(|held| held.partial));
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
            compare(&ask, &view, &Upstream::of(&messages, bounded), ASKED),
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
        assert_eq!(
            compare(&ask, &view, &Upstream::of(&messages, true), ASKED),
            None
        );
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
                stale: 4,
                newer: 3,
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
                page.missing_rows,
                page.stale_rows,
                page.newer_rows
            ),
            (2, 1, 2, 1, 4, 3),
            "messages past the end of the answer's claim are counted apart, and are no difference"
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
