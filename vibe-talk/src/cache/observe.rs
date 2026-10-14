//! What one upstream read tells the message cache, by provider and view. `#208 persistent-cache`.
//!
//! Pure: a read's answer in, [`Observation`]s out, with nothing read from the server's state. The
//! read sites in `crate::ops` and `crate::recent` call these through a [`super::Fill`], which
//! supplies the observation time it noted before the read began.
//!
//! # The one rule: a claim needs a proof
//!
//! A [`Listing`] with a [`Proof`] lets the writer stamp coverage ("every message of this scope
//! over this span was listed") and delete the rows of that scope the listing did not carry. A
//! wrong proof is how the cache would come to say a channel is complete when it is not, or erase a
//! message that still exists. So a proof is given only where the provider gave one:
//!
//! - **The start of a scope** only on the provider's own word: a newest Slack Main page, or a newest
//!   Discord page, whose `has_more` is false (computed by the client before it drops rows it cannot
//!   show), or a recent read marked `exhausted`. Never because a read came back shorter than it
//!   asked: Slack omits a message that has no author, so fifty asked can be forty-nine returned
//!   with the channel's start nowhere near.
//! - **Never the start of a Slack thread**, nor of a Slack channel narrowed to one, whose every
//!   view is a thread read. A thread's `has_more` is only whether the client found more rows than
//!   the page holds. A thread longer than the client's window of pages is narrowed to one probe
//!   page, gap-free from the newest end but possibly shorter than asked, and then `has_more` is
//!   false with older replies still upstream. Its newest page proves `[oldest row, newest]`, as any
//!   newest page does.
//! - **The newest end** from a read of the newest page or window, `[oldest row, newest]`.
//! - **A `/page` walk forward from `since`** that was not cut short and returned at least one row:
//!   `[since, min(until, newest row + 1))`. An empty one proves nothing, because a provider may
//!   answer a stretch with an empty page that is not its last. A step a provider's own range read
//!   answered (`#287 bulk-reads`) is not cut short only when the provider said it reached the
//!   span's end ([`crate::chat::RangeRead::exhausted`]); and a step that resumes after a message
//!   (`next_since`) proves its span from the millisecond after that message's, since it leaves out
//!   the messages of that millisecond the step before returned.
//! - **A `/count` walk**, which follows its own continuation from the newest page down: the union
//!   of its pages, `[oldest row seen, newest]`. A count made of a provider's own range reads,
//!   newest first on its own cursor ([`range_walk`]), is such a walk only when the provider said
//!   it reached the count's `since`.
//! - **The live poller's walk forward from its cursor**, when it ended on a page shorter than it
//!   asked for rather than at its page bound, and found a row: `(cursor, newest row seen]`, and
//!   on Discord's own API, where that shorter page is the channel's newest end, up to the newest
//!   end. Its first read, which seeds the cursor, is a recent window like any other.
//!
//! Everything else gives rows only: a page a caller stepped `before` to (nothing joins it to the
//! page above it), a page carrying a `notice` (the provider says it left something out), a delta
//! (it lists changes, not a span; a complete one's deletions are offered as such), a change pushed
//! by the live feed, a message this server posted, a message looked up by id ([`looked_up`]: an id
//! the provider proved absent is not taken as deleted either), and an empty read. Rows only still
//! tells the cache what each message says and where it belongs.
//!
//! # Scopes, per provider
//!
//! Each row has a HOME, the one scope whose exhaustive listing would show it is gone when it is
//! absent: `main` for a top-level message, `thread:<id>` for a reply, `flat` for everything on a
//! bridge. A listing of any other scope that lacks a row only takes away that membership.
//!
//! | Shape | Listed with a proof | Rows only |
//! |---|---|---|
//! | Slack | Main newest; Thread newest, from its oldest row; Flat newest only when it holds no thread at all; the poller's walk | Flat otherwise; the thread list |
//! | Slack, narrowed to one thread | Main (every view) newest, as `main` of that channel, from its oldest row; the poller's walk | the thread list |
//! | Bridge | Flat newest; a recent read through Flat | Main, Thread, `/messages` windows, a read of several channels at once, `/page`, `/count`, the poller's walk |
//! | Discord | Main newest; Thread newest; the poller's walk | Flat; the thread list |
//!
//! **Slack Flat.** It is the Main page plus the replies of at most ten of its threads, merged, and
//! nothing in the merged rows tells a reply also sent to the channel (which IS in Main) from one
//! that is only in its thread. So a Flat page is taken as a Main page only when it holds no thread
//! at all — no root with replies, and no reply — which is when it IS the Main page. Otherwise every
//! row is offered with its own home and no claim, and a Flat page never claims a thread: it holds
//! only the newest replies of each, so a claim would erase the older ones.
//!
//! **Bridge windows.** A recent read through the bridge's Flat view is the newest stretch of the
//! channel's whole list, and is listed as `flat`. One through `/messages` is not proven to be that
//! list, so a message it omits must not be taken as deleted: rows only. A bridge's `has_more` is not
//! taken as the start of a channel either: the copy it answers from has a size limit.
//!
//! Coverage on Slack is never `complete`: the client omits messages that have no author.

use super::coverage::Source;
use super::rows::created_ms;
use super::{Incoming, ListEnd, ListStart, Listing, Observation, Proof, Push, Scope, ThreadRecord};
use crate::chat::RecentRead;
use crate::config::{ProviderConfig, ProviderKind};
use crate::live::{LiveKind, LiveMessage};
use crate::model::Message;
use crate::threads::{ThreadApi, TimelinePage, TimelineRequest, TimelineView};

/// How a provider lists a channel's messages, which decides what each of its reads proves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Shape {
    /// Slack: Main is the conversation's own history (thread roots, and replies also sent to the
    /// channel), a thread is its replies, and Flat is Main plus a bounded set of replies.
    Slack,
    /// A Slack channel narrowed to one thread (`C…~ts`): every view is that thread, root and
    /// replies, as the `main` of a channel of its own.
    SlackThread,
    /// A compatible bridge: Flat is the channel's whole list, replies included, complete by time;
    /// Main and each thread are views onto it.
    Bridge,
    /// Discord's own API: Main is a channel's own messages, and a thread's messages belong to the
    /// parent channel's scope for that thread.
    DiscordNative,
}

impl Shape {
    /// The shape of `channel_id` under `provider`.
    ///
    /// A Discord-protocol provider with threads turned off is Discord when it is Discord's own
    /// API, and is taken as a bridge otherwise: whatever its `/messages` lists is then never
    /// taken as complete.
    #[must_use]
    pub fn of(provider: &ProviderConfig, channel_id: &str) -> Self {
        match &provider.kind {
            ProviderKind::Slack(_) if channel_id.contains(crate::slack::ids::THREAD_SEPARATOR) => {
                Self::SlackThread
            }
            ProviderKind::Slack(_) => Self::Slack,
            ProviderKind::Discord(discord) => match discord.thread_api {
                ThreadApi::Bridge => Self::Bridge,
                ThreadApi::Native => Self::DiscordNative,
                ThreadApi::Off
                    if crate::config::is_official_discord_api_base(&discord.api_base) =>
                {
                    Self::DiscordNative
                }
                ThreadApi::Off => Self::Bridge,
            },
        }
    }

    /// Whether this provider's listings could not have left a message out. A Slack client omits
    /// a message that has no author.
    fn complete(self) -> bool {
        !matches!(self, Self::Slack | Self::SlackThread)
    }
}

/// Which kind of read a timeline page answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimelineRead {
    /// The newest page of the view.
    Newest,
    /// A page a caller stepped back to with `before`.
    Before,
    /// What changed since a forward cursor.
    Forward,
}

/// Which kind of read a `/page` step was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageRead {
    /// The newest page.
    Newest,
    /// A step back from a message.
    Before,
    /// A walk forward from `since_ms`, up to `until_ms` when given; both provider time.
    Since {
        /// Where the walk began, inclusive.
        since_ms: i64,
        /// Where it was to end, exclusive.
        until_ms: Option<i64>,
    },
}

/// What a `/timeline` page tells the cache.
#[must_use]
pub fn timeline(
    shape: Shape,
    request: &TimelineRequest,
    read: TimelineRead,
    page: &TimelinePage,
    observed_ms: i64,
) -> Vec<Observation> {
    let mut out = Vec::new();
    let thread = request
        .thread_id
        .as_deref()
        .filter(|thread| !thread.is_empty());
    let thread_view = (request.view == TimelineView::Thread)
        .then_some(thread)
        .flatten();
    let scope = view_scope(shape, request.view, thread_view);
    let messages = &page.messages;
    if let Some(delta) = &page.delta {
        out.extend(rows(shape, messages, thread_view, scope, observed_ms));
        if delta.complete && !delta.deleted.is_empty() {
            out.push(Observation::Deleted {
                ids: delta.deleted.clone(),
                observed_ms,
            });
        }
    } else if read != TimelineRead::Newest || page.notice.is_some() {
        out.extend(rows(shape, messages, thread_view, scope, observed_ms));
    } else {
        let proven = listed(shape, request.view, thread_view, messages);
        match (proven, oldest(messages)) {
            (Some((listed, start_from_has_more)), Some(oldest)) => {
                let from = if start_from_has_more && !page.has_more {
                    ListStart::ScopeStart
                } else {
                    ListStart::At(oldest)
                };
                out.push(listing(
                    shape,
                    listed,
                    messages,
                    thread_view,
                    observed_ms,
                    from,
                    ListEnd::Newest,
                    Source::Read,
                ));
            }
            _ => out.extend(rows(shape, messages, thread_view, scope, observed_ms)),
        }
    }
    out.extend(threads(shape, page, observed_ms));
    out
}

/// What a read of a channel's most recent messages tells the cache: `fetch_recent_dated`, which
/// reads a bridge's Flat view when `via_flat`, or `fetch_recent` (never through Flat).
#[must_use]
pub fn window(
    shape: Shape,
    read: &RecentRead,
    via_flat: bool,
    observed_ms: i64,
) -> Vec<Observation> {
    let messages = &read.messages;
    let Some(oldest) = oldest(messages) else {
        return Vec::new();
    };
    let from = if read.exhausted {
        ListStart::ScopeStart
    } else {
        ListStart::At(oldest)
    };
    let bounded = read.notice.is_some();
    let (scope, proven) = match shape {
        Shape::Bridge => (Scope::Flat, via_flat && !bounded),
        _ => (Scope::Main, !bounded),
    };
    if !proven {
        let listed_in = (shape != Shape::Bridge).then_some(scope);
        return rows(shape, messages, None, listed_in, observed_ms)
            .into_iter()
            .collect();
    }
    vec![listing(
        shape,
        scope,
        messages,
        None,
        observed_ms,
        from,
        ListEnd::Newest,
        Source::Read,
    )]
}

/// What a read of several channels at once tells the cache of one of them (`#287 bulk-reads`,
/// [`crate::chat::ChatClient::fetch_recent_many`]): its most recent messages.
///
/// What [`window`] makes of a recent read, never as one through a bridge's Flat view: nothing says
/// the copy a bridge answers several channels from is its Flat list, so on a bridge it is rows
/// only, as a `/messages` window is.
#[must_use]
pub fn window_among_several(shape: Shape, read: &RecentRead, observed_ms: i64) -> Vec<Observation> {
    window(shape, read, false, observed_ms)
}

/// What one `/page` step tells the cache. `rows` are the messages the provider returned inside
/// the requested span, before any was cut to the page's size; `truncated` says one was.
#[must_use]
pub fn page(
    shape: Shape,
    read: PageRead,
    rows_read: &[Message],
    truncated: bool,
    observed_ms: i64,
) -> Vec<Observation> {
    let (Some(oldest), Some(newest)) = (oldest(rows_read), newest(rows_read)) else {
        return Vec::new();
    };
    if shape == Shape::Bridge {
        // `/page` reads the bridge's `/messages`, which is not proven to be its Flat list.
        return rows(shape, rows_read, None, None, observed_ms)
            .into_iter()
            .collect();
    }
    let proof = match read {
        PageRead::Newest => Some((ListStart::At(oldest), ListEnd::Newest)),
        PageRead::Since { since_ms, until_ms } if !truncated => {
            let end = until_ms.unwrap_or(i64::MAX).min(newest.saturating_add(1));
            (since_ms < end).then_some((ListStart::At(since_ms), ListEnd::Before(end)))
        }
        PageRead::Since { .. } | PageRead::Before => None,
    };
    match proof {
        Some((from, until)) => vec![listing(
            shape,
            Scope::Main,
            rows_read,
            None,
            observed_ms,
            from,
            until,
            Source::Read,
        )],
        None => rows(shape, rows_read, None, Some(Scope::Main), observed_ms)
            .into_iter()
            .collect(),
    }
}

/// What a `/count` walk tells the cache: every row it read, from the newest page down along its
/// own continuation.
#[must_use]
pub fn walk(shape: Shape, rows_read: &[Message], observed_ms: i64) -> Vec<Observation> {
    let Some(oldest) = oldest(rows_read) else {
        return Vec::new();
    };
    if shape == Shape::Bridge {
        return rows(shape, rows_read, None, None, observed_ms)
            .into_iter()
            .collect();
    }
    vec![listing(
        shape,
        Scope::Main,
        rows_read,
        None,
        observed_ms,
        ListStart::At(oldest),
        ListEnd::Newest,
        Source::Walk,
    )]
}

/// What a count made of a provider's own range reads tells the cache (`#287 bulk-reads`): every
/// row its reads returned, newest first from the channel's newest message, each read starting
/// where the one before it ended on the provider's own cursor.
///
/// Such a count is a walk ([`walk`]) only when `exhausted` says the provider proved its last read
/// reached the count's `since`: a count the cap or the provider's step budget stopped lists what it
/// read, but the provider never said its reads joined up, so it is rows only. A bridge's is rows
/// only either way, as its walk is.
#[must_use]
pub fn range_walk(
    shape: Shape,
    rows_read: &[Message],
    exhausted: bool,
    observed_ms: i64,
) -> Vec<Observation> {
    if exhausted {
        return walk(shape, rows_read, observed_ms);
    }
    let listed_in = (shape != Shape::Bridge).then_some(Scope::Main);
    rows(shape, rows_read, None, listed_in, observed_ms)
        .into_iter()
        .collect()
}

/// What a lookup by id tells the cache (`#287 bulk-reads`,
/// [`crate::chat::ChatClient::fetch_messages`]): the messages it found, each at its home, listed in
/// the thread it looked in when it looked in one. Rows only: a lookup lists no span of anything.
/// Nor is an id the provider proved absent offered as a deletion: that is the provider's word about
/// the one place it looked, and a message looked for in the wrong thread is absent from it while it
/// still exists.
#[must_use]
pub fn looked_up(
    shape: Shape,
    messages: &[Message],
    thread_id: Option<&str>,
    observed_ms: i64,
) -> Vec<Observation> {
    let listed_in = match (shape, thread_id) {
        (Shape::SlackThread, _) | (_, None) => None,
        (_, Some(thread)) => Some(Scope::Thread(thread.to_owned())),
    };
    rows(shape, messages, thread_id, listed_in, observed_ms)
        .into_iter()
        .collect()
}

/// What one message read by id from a thread tells the cache.
#[must_use]
pub fn single(
    shape: Shape,
    message: &Message,
    thread_id: &str,
    observed_ms: i64,
) -> Vec<Observation> {
    let listed_in = match shape {
        Shape::SlackThread => None,
        _ => Some(Scope::Thread(thread_id.to_owned())),
    };
    rows(
        shape,
        std::slice::from_ref(message),
        Some(thread_id),
        listed_in,
        observed_ms,
    )
    .into_iter()
    .collect()
}

/// What one change from the live feed tells the cache: a creation or an edit carries the message,
/// at its home by the provider's shape; a deletion carries its id. A push is never a claim about
/// any scope, and a creation of a message the cache does not hold is kept as possibly a snippet
/// until a read replaces it (the writer's rule, not this one's).
#[must_use]
pub fn pushed(shape: Shape, live: &LiveMessage, observed_ms: i64) -> Observation {
    let push = match live.kind {
        LiveKind::Create | LiveKind::Update => Push::Upsert {
            message: Box::new(Incoming::new(
                live.message.clone(),
                home(shape, &live.message, None),
            )),
            update: live.kind == LiveKind::Update,
            partial: false,
        },
        LiveKind::Delete => Push::Delete {
            id: live.message.id.clone(),
            created_ms: None,
        },
    };
    Observation::Push { push, observed_ms }
}

/// What a message this server posted tells the cache: that row, at its home, listed in the thread
/// it was posted into, if any. A post proves nothing about the rest of the channel.
#[must_use]
pub fn posted(
    shape: Shape,
    message: &Message,
    thread_id: Option<&str>,
    observed_ms: i64,
) -> Vec<Observation> {
    let listed_in = match (shape, thread_id) {
        (Shape::SlackThread, _) | (_, None) => None,
        (_, Some(thread)) => Some(Scope::Thread(thread.to_owned())),
    };
    rows(
        shape,
        std::slice::from_ref(message),
        thread_id,
        listed_in,
        observed_ms,
    )
    .into_iter()
    .collect()
}

/// What the live poller's walk forward from its cursor tells the cache: every message its pages
/// returned, all of them newer than `after_ms`, the provider time of the cursor it walked from.
///
/// A walk forward reads the oldest messages after its cursor, page after page, each page starting
/// after the last message the one before it returned, so what it read is every message between
/// the cursor and the newest one it saw: a listing of `main` over `(after_ms, newest]`, when
/// `reached_end` says the walk ended on a page shorter than it asked for rather than at its page
/// bound, and at least one message came back. On Discord's own API that shorter page is the
/// channel's newest end, so the listing runs to it. A Slack client drops a message that has no
/// author, so its shorter page is no proof of the end and the listing stops at the newest message
/// seen. A bridge's walk reads its `/messages`, which is not proven to be its Flat list: rows only,
/// as is a walk that ended for any other reason.
#[must_use]
pub fn forward(
    shape: Shape,
    rows_read: &[Message],
    after_ms: i64,
    reached_end: bool,
    observed_ms: i64,
) -> Vec<Observation> {
    let Some(newest) = newest(rows_read) else {
        return Vec::new();
    };
    if shape == Shape::Bridge || !reached_end {
        let listed_in = (shape != Shape::Bridge).then_some(Scope::Main);
        return rows(shape, rows_read, None, listed_in, observed_ms)
            .into_iter()
            .collect();
    }
    let until = if shape == Shape::DiscordNative {
        ListEnd::Newest
    } else {
        ListEnd::Before(newest.saturating_add(1))
    };
    vec![listing(
        shape,
        Scope::Main,
        rows_read,
        None,
        observed_ms,
        ListStart::At(after_ms.saturating_add(1)),
        until,
        Source::Poll,
    )]
}

/// The scope a row belongs to by nature. `thread_view` is the thread a Thread view read, when the
/// read was one.
fn home(shape: Shape, message: &Message, thread_view: Option<&str>) -> Scope {
    match shape {
        Shape::Bridge => Scope::Flat,
        Shape::SlackThread => Scope::Main,
        Shape::Slack | Shape::DiscordNative => match &message.thread {
            Some(thread) if !thread.is_root => Scope::Thread(thread.id.clone()),
            Some(_) => Scope::Main,
            // A row of a thread view that names no thread of its own is that thread's, unless it
            // is the message the thread is named after.
            None => match thread_view {
                Some(thread) if message.id.as_str() != thread => Scope::Thread(thread.to_owned()),
                _ => Scope::Main,
            },
        },
    }
}

/// The scope a view's rows are listed in, beyond their homes, when the view is one.
fn view_scope(shape: Shape, view: TimelineView, thread: Option<&str>) -> Option<Scope> {
    match (shape, view) {
        (_, TimelineView::Threads) => None,
        (Shape::SlackThread, _) => Some(Scope::Main),
        (_, TimelineView::Main) => Some(Scope::Main),
        (_, TimelineView::Thread) => thread.map(|thread| Scope::Thread(thread.to_owned())),
        (Shape::Bridge, TimelineView::Flat) => Some(Scope::Flat),
        (Shape::Slack | Shape::DiscordNative, TimelineView::Flat) => None,
    }
}

/// The scope a newest page of this view proves, and whether its `has_more` speaks for the start
/// of that scope; `None` when the page proves nothing.
///
/// A Slack thread's `has_more` never does, nor that of a Slack channel narrowed to one thread:
/// see the module documentation.
fn listed(
    shape: Shape,
    view: TimelineView,
    thread: Option<&str>,
    messages: &[Message],
) -> Option<(Scope, bool)> {
    match (shape, view) {
        (_, TimelineView::Threads) => None,
        (Shape::SlackThread, _) => Some((Scope::Main, false)),
        (Shape::Slack | Shape::DiscordNative, TimelineView::Main) => Some((Scope::Main, true)),
        (Shape::Slack, TimelineView::Thread) => {
            thread.map(|thread| (Scope::Thread(thread.to_owned()), false))
        }
        (Shape::DiscordNative, TimelineView::Thread) => {
            thread.map(|thread| (Scope::Thread(thread.to_owned()), true))
        }
        (Shape::Slack, TimelineView::Flat) => {
            holds_no_thread(messages).then_some((Scope::Main, true))
        }
        (Shape::DiscordNative, TimelineView::Flat) => None,
        (Shape::Bridge, TimelineView::Flat) => Some((Scope::Flat, false)),
        (Shape::Bridge, TimelineView::Main | TimelineView::Thread) => None,
    }
}

/// Whether a page holds no thread at all: no root with replies and no reply. A Slack Flat page
/// that holds none is exactly its Main page.
fn holds_no_thread(messages: &[Message]) -> bool {
    messages.iter().all(|message| {
        message
            .thread
            .as_ref()
            .is_none_or(|thread| thread.is_root && thread.reply_count.unwrap_or(0) == 0)
    })
}

fn oldest(messages: &[Message]) -> Option<i64> {
    messages.iter().filter_map(created_ms).min()
}

fn newest(messages: &[Message]) -> Option<i64> {
    messages.iter().filter_map(created_ms).max()
}

fn incoming(shape: Shape, messages: &[Message], thread_view: Option<&str>) -> Vec<Incoming> {
    messages
        .iter()
        .map(|message| Incoming::new(message.clone(), home(shape, message, thread_view)))
        .collect()
}

/// The rows of a read that proves nothing, each with its own home, listed in `scope` as well when
/// given; `None` when there are none.
fn rows(
    shape: Shape,
    messages: &[Message],
    thread_view: Option<&str>,
    scope: Option<Scope>,
    observed_ms: i64,
) -> Option<Observation> {
    (!messages.is_empty()).then(|| Observation::Rows {
        rows: incoming(shape, messages, thread_view),
        scope,
        observed_ms,
    })
}

#[allow(clippy::too_many_arguments)]
fn listing(
    shape: Shape,
    scope: Scope,
    messages: &[Message],
    thread_view: Option<&str>,
    observed_ms: i64,
    from: ListStart,
    until: ListEnd,
    source: Source,
) -> Observation {
    Observation::Listing(Listing {
        scope,
        rows: incoming(shape, messages, thread_view),
        observed_ms,
        proof: Some(Proof {
            from,
            until,
            complete: shape.complete(),
            full: true,
            source,
        }),
    })
}

/// The thread records a page carries, and their roots as rows. A narrowed Slack channel has no
/// threads of its own.
fn threads(shape: Shape, page: &TimelinePage, observed_ms: i64) -> Vec<Observation> {
    let summaries: Vec<_> = page.threads.iter().chain(page.thread.iter()).collect();
    let mut out = Vec::new();
    if shape != Shape::SlackThread {
        let records: Vec<ThreadRecord> = summaries
            .iter()
            .filter_map(|summary| {
                let last_activity_ms = crate::clock::instant_ms(&summary.updated_at)
                    .or_else(|| summary.root.as_ref().and_then(created_ms))?;
                Some(ThreadRecord {
                    thread_id: summary.id.clone(),
                    root_id: summary.root.as_ref().map(|root| root.id.clone()),
                    reply_count: summary.reply_count,
                    reply_count_exact: summary.reply_count_exact,
                    last_activity_ms,
                })
            })
            .collect();
        if !records.is_empty() {
            out.push(Observation::Threads {
                threads: records,
                observed_ms,
            });
        }
    }
    let roots: Vec<Message> = summaries
        .iter()
        .filter_map(|summary| summary.root.clone())
        .collect();
    out.extend(rows(shape, &roots, None, None, observed_ms));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::fixtures::{message, reply};
    use crate::model::MessageId;
    use crate::threads::{MessageThread, ThreadSummary, TimelineDelta};

    const T: i64 = 1_787_054_400_000;

    /// One row of the timeline table: the read, and what it must tell the cache.
    type TimelineCase<'a> = (
        Shape,
        TimelineView,
        Option<&'a str>,
        TimelineRead,
        TimelinePage,
        Vec<&'a str>,
    );

    /// One row of the `/page` table.
    type PageCase<'a> = (Shape, PageRead, Vec<Message>, bool, Vec<&'a str>);
    const OBSERVED: i64 = T + 3_600_000;

    /// A top-level message, the root of its own thread with `replies` replies.
    fn root(id: &str, at: i64, replies: u64) -> Message {
        Message {
            thread: Some(MessageThread {
                id: id.to_owned(),
                root_message_id: Some(MessageId(id.to_owned())),
                is_root: true,
                reply_count: Some(replies),
                reply_count_exact: true,
            }),
            ..message(id, "a", "root", at)
        }
    }

    fn request(view: TimelineView, thread: Option<&str>) -> TimelineRequest {
        TimelineRequest {
            view,
            thread_id: thread.map(str::to_owned),
            ..TimelineRequest::default()
        }
    }

    /// One line per observation: its kind, its scope or proof, and each row's home.
    fn describe(observations: &[Observation]) -> Vec<String> {
        let homes = |rows: &[Incoming]| {
            rows.iter()
                .map(|row| format!("{}={}", row.message.id.as_str(), row.home.key()))
                .collect::<Vec<_>>()
                .join(",")
        };
        observations
            .iter()
            .map(|observation| match observation {
                Observation::Listing(listing) => {
                    let proof = listing.proof.expect("a listing here always has a proof");
                    let from = match proof.from {
                        ListStart::ScopeStart => "start".to_owned(),
                        ListStart::At(ms) => format!("+{}", ms - T),
                    };
                    let until = match proof.until {
                        ListEnd::Newest => "newest".to_owned(),
                        ListEnd::Before(ms) => format!("+{}", ms - T),
                    };
                    format!(
                        "listing {} [{from},{until}) complete={} full={} {:?} {}",
                        listing.scope.key(),
                        proof.complete,
                        proof.full,
                        proof.source,
                        homes(&listing.rows)
                    )
                }
                Observation::Rows { rows, scope, .. } => format!(
                    "rows in={} {}",
                    scope.as_ref().map_or("-".to_owned(), Scope::key),
                    homes(rows)
                ),
                Observation::Deleted { ids, .. } => format!(
                    "deleted {}",
                    ids.iter()
                        .map(MessageId::as_str)
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Observation::Threads { threads, .. } => format!(
                    "threads {}",
                    threads
                        .iter()
                        .map(|t| t.thread_id.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Observation::Push { push, .. } => match push {
                    Push::Upsert {
                        message,
                        update,
                        partial,
                    } => format!(
                        "push {} {}={} partial={partial}",
                        if *update { "update" } else { "create" },
                        message.message.id.as_str(),
                        message.home.key()
                    ),
                    Push::Delete { id, created_ms } => {
                        format!("push delete {} created={created_ms:?}", id.as_str())
                    }
                },
            })
            .collect()
    }

    fn every_observation_is_at(observations: &[Observation], at: i64) {
        for observation in observations {
            let observed = match observation {
                Observation::Listing(listing) => listing.observed_ms,
                Observation::Rows { observed_ms, .. }
                | Observation::Deleted { observed_ms, .. }
                | Observation::Threads { observed_ms, .. }
                | Observation::Push { observed_ms, .. } => *observed_ms,
            };
            assert_eq!(observed, at);
        }
    }

    /// A page of `messages`, with `has_more`, and a notice when `notice`.
    fn page_of(messages: Vec<Message>, has_more: bool, notice: bool) -> TimelinePage {
        TimelinePage {
            messages,
            has_more,
            notice: notice.then(|| "bounded".to_owned()),
            ..TimelinePage::default()
        }
    }

    fn delta(messages: Vec<Message>, complete: bool, deleted: &[&str]) -> TimelinePage {
        TimelinePage {
            messages,
            delta: Some(TimelineDelta {
                more: false,
                complete,
                deleted: deleted
                    .iter()
                    .map(|id| MessageId((*id).to_owned()))
                    .collect(),
                removed_threads: Vec::new(),
            }),
            ..TimelinePage::default()
        }
    }

    /// Main and Flat rows: an unanswered root, a root with replies, and a reply also sent to the
    /// channel.
    fn main_rows() -> Vec<Message> {
        vec![
            root("m100", T, 0),
            root("m200", T + 60_000, 2),
            reply("m300", "m200", "also sent to the channel", T + 120_000),
        ]
    }

    #[test]
    fn the_table_shape_by_view_by_read() {
        use TimelineRead::{Before, Forward, Newest};
        use TimelineView::{Flat, Main, Thread, Threads};
        let quiet = || vec![root("m100", T, 0), root("m150", T + 30_000, 0)];
        let thread_rows = || {
            vec![
                root("m200", T + 60_000, 2),
                reply("m210", "m200", "first", T + 90_000),
                reply("m220", "m200", "second", T + 150_000),
            ]
        };
        let bridge_rows = || {
            vec![
                message("m100", "a", "top", T),
                reply("m210", "m200", "a reply", T + 90_000),
            ]
        };
        #[rustfmt::skip]
        let cases: Vec<TimelineCase<'_>> = vec![
            // Slack Main: proven by `has_more`; a reply sent to the channel is home in its thread.
            (Shape::Slack, Main, None, Newest, page_of(main_rows(), false, false),
             vec!["listing main [start,newest) complete=false full=true Read m100=main,m200=main,m300=thread:m200"]),
            (Shape::Slack, Main, None, Newest, page_of(main_rows(), true, false),
             vec!["listing main [+0,newest) complete=false full=true Read m100=main,m200=main,m300=thread:m200"]),
            (Shape::Slack, Main, None, Before, page_of(main_rows(), false, false),
             vec!["rows in=main m100=main,m200=main,m300=thread:m200"]),
            (Shape::Slack, Main, None, Newest, page_of(main_rows(), false, true),
             vec!["rows in=main m100=main,m200=main,m300=thread:m200"]),
            (Shape::Slack, Main, None, Forward, delta(main_rows(), false, &[]),
             vec!["rows in=main m100=main,m200=main,m300=thread:m200"]),
            (Shape::Slack, Main, None, Newest, page_of(Vec::new(), false, false), vec![]),
            // Slack Thread: the root stays home in Main. Its `has_more` is no proof of the start:
            // a long thread narrows to a short page that says there is no more.
            (Shape::Slack, Thread, Some("m200"), Newest, page_of(thread_rows(), false, false),
             vec!["listing thread:m200 [+60000,newest) complete=false full=true Read m200=main,m210=thread:m200,m220=thread:m200"]),
            (Shape::Slack, Thread, Some("m200"), Newest, page_of(thread_rows()[2..].to_vec(), false, false),
             vec!["listing thread:m200 [+150000,newest) complete=false full=true Read m220=thread:m200"]),
            (Shape::Slack, Thread, Some("m200"), Newest, page_of(thread_rows(), true, false),
             vec!["listing thread:m200 [+60000,newest) complete=false full=true Read m200=main,m210=thread:m200,m220=thread:m200"]),
            (Shape::Slack, Thread, Some("m200"), Before, page_of(thread_rows(), true, false),
             vec!["rows in=thread:m200 m200=main,m210=thread:m200,m220=thread:m200"]),
            // Slack Flat: Main only when it holds no thread; otherwise rows, never a thread claim.
            (Shape::Slack, Flat, None, Newest, page_of(quiet(), false, false),
             vec!["listing main [start,newest) complete=false full=true Read m100=main,m150=main"]),
            (Shape::Slack, Flat, None, Newest, page_of(thread_rows(), false, false),
             vec!["rows in=- m200=main,m210=thread:m200,m220=thread:m200"]),
            (Shape::Slack, Flat, None, Newest, page_of(vec![root("m100", T, 0), reply("m300", "m200", "x", T + 1)], false, false),
             vec!["rows in=- m100=main,m300=thread:m200"]),
            (Shape::Slack, Flat, None, Newest, page_of(quiet(), false, true),
             vec!["rows in=- m100=main,m150=main"]),
            (Shape::Slack, Flat, None, Before, page_of(quiet(), false, false),
             vec!["rows in=- m100=main,m150=main"]),
            // A channel narrowed to one thread: everything is its own Main, read as a thread is.
            (Shape::SlackThread, Main, None, Newest, page_of(thread_rows(), false, false),
             vec!["listing main [+60000,newest) complete=false full=true Read m200=main,m210=main,m220=main"]),
            (Shape::SlackThread, Main, None, Newest, page_of(thread_rows()[2..].to_vec(), false, false),
             vec!["listing main [+150000,newest) complete=false full=true Read m220=main"]),
            (Shape::SlackThread, Thread, Some("m200"), Newest, page_of(thread_rows(), true, false),
             vec!["listing main [+60000,newest) complete=false full=true Read m200=main,m210=main,m220=main"]),
            (Shape::SlackThread, Flat, None, Before, page_of(thread_rows(), true, false),
             vec!["rows in=main m200=main,m210=main,m220=main"]),
            // Bridge: Flat is the whole list, complete, but its `has_more` is no start.
            (Shape::Bridge, Flat, None, Newest, page_of(bridge_rows(), false, false),
             vec!["listing flat [+0,newest) complete=true full=true Read m100=flat,m210=flat"]),
            (Shape::Bridge, Flat, None, Newest, page_of(bridge_rows(), false, true),
             vec!["rows in=flat m100=flat,m210=flat"]),
            (Shape::Bridge, Flat, None, Before, page_of(bridge_rows(), false, false),
             vec!["rows in=flat m100=flat,m210=flat"]),
            (Shape::Bridge, Main, None, Newest, page_of(bridge_rows(), false, false),
             vec!["rows in=main m100=flat,m210=flat"]),
            (Shape::Bridge, Thread, Some("m200"), Newest, page_of(bridge_rows(), false, false),
             vec!["rows in=thread:m200 m100=flat,m210=flat"]),
            (Shape::Bridge, Flat, None, Forward, delta(bridge_rows(), true, &["90", "91"]),
             vec!["rows in=flat m100=flat,m210=flat", "deleted 90,91"]),
            (Shape::Bridge, Flat, None, Forward, delta(Vec::new(), true, &["90"]),
             vec!["deleted 90"]),
            (Shape::Bridge, Flat, None, Forward, delta(bridge_rows(), false, &[]),
             vec!["rows in=flat m100=flat,m210=flat"]),
            (Shape::Bridge, Flat, None, Newest, page_of(Vec::new(), false, false), vec![]),
            // Discord: Main and Thread proven, complete; Flat never.
            (Shape::DiscordNative, Main, None, Newest, page_of(quiet(), false, false),
             vec!["listing main [start,newest) complete=true full=true Read m100=main,m150=main"]),
            (Shape::DiscordNative, Thread, Some("m200"), Newest, page_of(vec![message("m200", "a", "starter", T), message("m210", "a", "in it", T + 1)], false, false),
             vec!["listing thread:m200 [start,newest) complete=true full=true Read m200=main,m210=thread:m200"]),
            (Shape::DiscordNative, Flat, None, Newest, page_of(thread_rows(), false, false),
             vec!["rows in=- m200=main,m210=thread:m200,m220=thread:m200"]),
            (Shape::DiscordNative, Main, None, Newest, page_of(quiet(), false, true),
             vec!["rows in=main m100=main,m150=main"]),
            // The thread list proves nothing about messages.
            (Shape::Slack, Threads, None, Newest, page_of(Vec::new(), false, false), vec![]),
        ];
        for (shape, view, thread, read, page, expected) in cases {
            let got = timeline(shape, &request(view, thread), read, &page, OBSERVED);
            assert_eq!(
                describe(&got),
                expected,
                "{shape:?} {view:?} {thread:?} {read:?}"
            );
            every_observation_is_at(&got, OBSERVED);
        }
    }

    #[test]
    fn a_thread_list_gives_thread_records_and_roots_as_rows() {
        let summary = |id: &str, replies: u64| ThreadSummary {
            id: id.to_owned(),
            root: Some(root(id, T, replies)),
            title: "t".to_owned(),
            reply_count: Some(replies),
            reply_count_exact: true,
            updated_at: crate::clock::iso_from_ms(T + 600_000),
            display_name: None,
            summary: None,
        };
        let page = TimelinePage {
            threads: vec![summary("m100", 3), summary("m200", 1)],
            ..TimelinePage::default()
        };
        let got = timeline(
            Shape::Slack,
            &request(TimelineView::Threads, None),
            TimelineRead::Newest,
            &page,
            OBSERVED,
        );
        assert_eq!(
            describe(&got),
            ["threads m100,m200", "rows in=- m100=main,m200=main"]
        );
        let Observation::Threads { threads, .. } = &got[0] else {
            panic!("thread records first");
        };
        assert_eq!(threads[0].last_activity_ms, T + 600_000);
        assert_eq!(threads[0].reply_count, Some(3));
        // A narrowed channel has no threads of its own: its summary names the channel itself.
        let narrowed = timeline(
            Shape::SlackThread,
            &request(TimelineView::Threads, None),
            TimelineRead::Newest,
            &page,
            OBSERVED,
        );
        assert_eq!(describe(&narrowed), ["rows in=- m100=main,m200=main"]);
    }

    #[test]
    fn a_bounded_flat_page_never_lists_or_tombstones_a_thread() {
        // Eleven threads on the page, one with sixty replies of which Flat carries the newest
        // fifty: the bound Slack's Flat applies, and says so.
        let mut messages = Vec::new();
        for n in 0..11_i64 {
            let id = format!("t{}", 1_000 + n);
            messages.push(root(&id, T + n * 1_000, if n == 0 { 60 } else { 1 }));
            let replies = if n == 0 { 50 } else { 1 };
            for r in 0..replies {
                messages.push(reply(
                    &format!("{id}-{r}"),
                    &id,
                    "reply",
                    T + n * 1_000 + 100 + r,
                ));
            }
        }
        for notice in [true, false] {
            let page = page_of(messages.clone(), false, notice);
            let got = timeline(
                Shape::Slack,
                &request(TimelineView::Flat, None),
                TimelineRead::Newest,
                &page,
                OBSERVED,
            );
            assert_eq!(got.len(), 1, "rows only, notice {notice}");
            assert!(matches!(&got[0], Observation::Rows { scope: None, .. }));
        }
    }

    #[test]
    fn a_window_proves_the_start_only_when_the_provider_says_so() {
        let read = |messages: Vec<Message>, exhausted: bool, notice: bool| RecentRead {
            messages,
            as_of: None,
            exhausted,
            notice: notice.then(|| "bounded".to_owned()),
        };
        // Slack drops a row with no author: two returned of the twenty asked is not the start.
        let short = vec![root("m100", T, 0), root("m200", T + 180_000, 0)];
        #[rustfmt::skip]
        let cases: Vec<(Shape, RecentRead, bool, Vec<&str>)> = vec![
            (Shape::Slack, read(short.clone(), false, false), false,
             vec!["listing main [+0,newest) complete=false full=true Read m100=main,m200=main"]),
            (Shape::Slack, read(short.clone(), true, false), false,
             vec!["listing main [start,newest) complete=false full=true Read m100=main,m200=main"]),
            (Shape::DiscordNative, read(short.clone(), false, false), false,
             vec!["listing main [+0,newest) complete=true full=true Read m100=main,m200=main"]),
            (Shape::SlackThread, read(short.clone(), false, false), false,
             vec!["listing main [+0,newest) complete=false full=true Read m100=main,m200=main"]),
            (Shape::Bridge, read(short.clone(), false, false), true,
             vec!["listing flat [+0,newest) complete=true full=true Read m100=flat,m200=flat"]),
            (Shape::Bridge, read(short.clone(), false, true), true,
             vec!["rows in=- m100=flat,m200=flat"]),
            (Shape::Bridge, read(short.clone(), false, false), false,
             vec!["rows in=- m100=flat,m200=flat"]),
            (Shape::Slack, read(short.clone(), false, true), false,
             vec!["rows in=main m100=main,m200=main"]),
            (Shape::Slack, read(Vec::new(), true, false), false, vec![]),
        ];
        for (shape, read, via_flat, expected) in cases {
            assert_eq!(
                describe(&window(shape, &read, via_flat, OBSERVED)),
                expected,
                "{shape:?} via_flat={via_flat}"
            );
        }
    }

    #[test]
    fn a_page_walk_proves_its_span_only_when_it_was_not_cut_short_and_found_a_row() {
        let rows = vec![
            message("m100", "a", "x", T + 60_000),
            message("m200", "a", "y", T + 300_000),
        ];
        let since = PageRead::Since {
            since_ms: T,
            until_ms: Some(T + 3_600_000),
        };
        #[rustfmt::skip]
        let cases: Vec<PageCase<'_>> = vec![
            (Shape::Slack, since, rows.clone(), false,
             vec!["listing main [+0,+300001) complete=false full=true Read m100=main,m200=main"]),
            (Shape::DiscordNative, PageRead::Since { since_ms: T, until_ms: Some(T + 120_000) }, rows[..1].to_vec(), false,
             vec!["listing main [+0,+60001) complete=true full=true Read m100=main"]),
            (Shape::DiscordNative, PageRead::Since { since_ms: T, until_ms: Some(T + 60_000 + 1) }, rows[..1].to_vec(), false,
             vec!["listing main [+0,+60001) complete=true full=true Read m100=main"]),
            (Shape::Slack, PageRead::Since { since_ms: T, until_ms: None }, rows.clone(), false,
             vec!["listing main [+0,+300001) complete=false full=true Read m100=main,m200=main"]),
            (Shape::Slack, since, rows.clone(), true, vec!["rows in=main m100=main,m200=main"]),
            (Shape::Slack, since, Vec::new(), false, vec![]),
            (Shape::Slack, PageRead::Before, rows.clone(), false, vec!["rows in=main m100=main,m200=main"]),
            (Shape::Slack, PageRead::Newest, rows.clone(), true,
             vec!["listing main [+60000,newest) complete=false full=true Read m100=main,m200=main"]),
            (Shape::Bridge, since, rows.clone(), false, vec!["rows in=- m100=flat,m200=flat"]),
        ];
        for (shape, read, rows, truncated, expected) in cases {
            assert_eq!(
                describe(&page(shape, read, &rows, truncated, OBSERVED)),
                expected,
                "{shape:?} {read:?} truncated={truncated}"
            );
        }
    }

    #[test]
    fn a_count_walk_is_the_union_of_its_pages_and_a_lookup_is_one_row() {
        let rows = vec![
            message("m100", "a", "x", T),
            message("m200", "a", "y", T + 300_000),
        ];
        assert_eq!(
            describe(&walk(Shape::DiscordNative, &rows, OBSERVED)),
            ["listing main [+0,newest) complete=true full=true Walk m100=main,m200=main"]
        );
        assert_eq!(
            describe(&walk(Shape::Bridge, &rows, OBSERVED)),
            ["rows in=- m100=flat,m200=flat"]
        );
        assert!(walk(Shape::Slack, &[], OBSERVED).is_empty());
        let one = reply("m210", "m200", "in it", T);
        assert_eq!(
            describe(&single(Shape::Slack, &one, "m200", OBSERVED)),
            ["rows in=thread:m200 m210=thread:m200"]
        );
        assert_eq!(
            describe(&single(Shape::Slack, &root("m200", T, 1), "m200", OBSERVED)),
            ["rows in=thread:m200 m200=main"]
        );
        assert_eq!(
            describe(&single(Shape::Bridge, &one, "m200", OBSERVED)),
            ["rows in=thread:m200 m210=flat"]
        );
        assert_eq!(
            describe(&single(Shape::SlackThread, &one, "m200", OBSERVED)),
            ["rows in=- m210=main"]
        );
    }

    fn live(kind: LiveKind, message: Message) -> LiveMessage {
        LiveMessage {
            event_id: message.id.as_str().to_owned(),
            kind,
            historical: false,
            message,
            self_posted: false,
        }
    }

    #[test]
    fn a_push_is_the_message_at_its_home_or_the_id_it_deletes_and_never_a_claim() {
        let one = reply("m210", "m200", "in it", T);
        #[rustfmt::skip]
        let cases: Vec<(Shape, LiveKind, Message, &str)> = vec![
            (Shape::Slack, LiveKind::Create, root("m100", T, 0), "push create m100=main partial=false"),
            (Shape::Slack, LiveKind::Update, one.clone(), "push update m210=thread:m200 partial=false"),
            (Shape::DiscordNative, LiveKind::Create, one.clone(), "push create m210=thread:m200 partial=false"),
            (Shape::SlackThread, LiveKind::Create, one.clone(), "push create m210=main partial=false"),
            (Shape::Bridge, LiveKind::Update, one.clone(), "push update m210=flat partial=false"),
            (Shape::Bridge, LiveKind::Delete, one.clone(), "push delete m210 created=None"),
            (Shape::Slack, LiveKind::Delete, root("m100", T, 0), "push delete m100 created=None"),
        ];
        for (shape, kind, message, expected) in cases {
            let got = [pushed(shape, &live(kind, message), OBSERVED)];
            assert_eq!(describe(&got), [expected], "{shape:?} {kind:?}");
            every_observation_is_at(&got, OBSERVED);
        }
    }

    #[test]
    fn a_post_is_its_own_row_listed_in_the_thread_it_went_to() {
        let top = message("m100", "a", "posted", T);
        let into = reply("m210", "m200", "posted in a thread", T);
        #[rustfmt::skip]
        let cases: Vec<(Shape, &Message, Option<&str>, &str)> = vec![
            (Shape::Slack, &top, None, "rows in=- m100=main"),
            (Shape::Slack, &into, Some("m200"), "rows in=thread:m200 m210=thread:m200"),
            (Shape::Slack, &top, Some("m200"), "rows in=thread:m200 m100=thread:m200"),
            (Shape::DiscordNative, &top, None, "rows in=- m100=main"),
            (Shape::Bridge, &into, Some("m200"), "rows in=thread:m200 m210=flat"),
            (Shape::SlackThread, &into, Some("m200"), "rows in=- m210=main"),
        ];
        for (shape, message, thread, expected) in cases {
            let got = posted(shape, message, thread, OBSERVED);
            assert_eq!(describe(&got), [expected], "{shape:?} {thread:?}");
            every_observation_is_at(&got, OBSERVED);
        }
    }

    #[test]
    fn the_pollers_walk_proves_its_span_only_when_it_ended_short_and_found_a_row() {
        let rows = vec![
            root("m100", T + 60_000, 0),
            reply("m300", "m200", "sent to the channel too", T + 300_000),
        ];
        #[rustfmt::skip]
        let cases: Vec<(Shape, Vec<Message>, bool, Vec<&str>)> = vec![
            // Discord's shorter page is the newest end.
            (Shape::DiscordNative, rows.clone(), true,
             vec!["listing main [+1,newest) complete=true full=true Poll m100=main,m300=thread:m200"]),
            // A Slack client drops a message with no author: up to the newest row seen.
            (Shape::Slack, rows.clone(), true,
             vec!["listing main [+1,+300001) complete=false full=true Poll m100=main,m300=thread:m200"]),
            (Shape::SlackThread, rows.clone(), true,
             vec!["listing main [+1,+300001) complete=false full=true Poll m100=main,m300=main"]),
            // Stopped by its page bound: rows only.
            (Shape::DiscordNative, rows.clone(), false, vec!["rows in=main m100=main,m300=thread:m200"]),
            (Shape::Slack, rows.clone(), false, vec!["rows in=main m100=main,m300=thread:m200"]),
            // A bridge's `/messages` is not its Flat list.
            (Shape::Bridge, rows.clone(), true, vec!["rows in=- m100=flat,m300=flat"]),
            // Nothing new: nothing to say.
            (Shape::DiscordNative, Vec::new(), true, vec![]),
        ];
        for (shape, rows, reached_end, expected) in cases {
            let got = forward(shape, &rows, T, reached_end, OBSERVED);
            assert_eq!(
                describe(&got),
                expected,
                "{shape:?} reached_end={reached_end}"
            );
            every_observation_is_at(&got, OBSERVED);
        }
    }

    #[test]
    fn the_shape_comes_from_the_provider_kind_and_its_thread_api() {
        let config = |provider: &str| {
            crate::config::Config::from_toml_and_env(
                &format!(
                    "[auth]\nread_token = \"read-token-that-is-long-enough\"\n\
                     write_token = \"write-token-that-is-long-enough\"\n\n\
                     [[providers]]\nkey = \"p\"\n{provider}\n\n\
                     [[channels]]\nid = \"C0123ABCDE\"\nlabel = \"x\"\nprovider = \"p\"\n"
                ),
                &std::collections::BTreeMap::new(),
            )
            .expect("valid")
        };
        let slack = config("kind = \"slack\"\ntoken = \"xoxb-slack-token\"");
        let p = slack.provider("p").expect("provider");
        assert_eq!(Shape::of(p, "C0123ABCDE"), Shape::Slack);
        assert_eq!(
            Shape::of(p, "C0123ABCDE~1700000000.000100"),
            Shape::SlackThread
        );
        for (settings, shape) in [
            ("", Shape::DiscordNative),
            ("thread_api = \"off\"", Shape::DiscordNative),
            (
                "api_base = \"http://127.0.0.1:9/api\"\nthread_api = \"bridge\"",
                Shape::Bridge,
            ),
            (
                "api_base = \"http://127.0.0.1:9/api\"\nthread_api = \"off\"",
                Shape::Bridge,
            ),
        ] {
            let discord = config(&format!("kind = \"discord\"\ntoken = \"t\"\n{settings}"));
            assert_eq!(
                Shape::of(discord.provider("p").expect("provider"), "1"),
                shape,
                "{settings:?}"
            );
        }
    }
}
