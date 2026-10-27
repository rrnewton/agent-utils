//! A background reader that keeps the live feed and the message cache honest about what a
//! provider's event stream leaves out at the newest end: the relist (`#452 slack-live-ingest`).
//!
//! # The relist
//!
//! A provider's event stream carries new messages, and the server now publishes them as they
//! arrive ([`crate::live_tail`]). It does not carry edits, deletions or other bots' posts. The
//! live poller's walk forward from its cursor publishes the other bots' posts, and nothing at all
//! published an edit or a deletion of a message already out until a page read it again.
//!
//! So every `[ingest] relist_seconds`, each channel of a provider that takes pushed messages has
//! its newest page read once, and each of its threads a reply was pushed in recently has its
//! newest page read too (at most `[ingest] relist_threads` a channel). Each read is offered to
//! the message cache like any other server read, under the refresher's grant, and compared with
//! what the hub gave readers ([`crate::live::LiveHub::publish_relisted`]): a message the read
//! holds differently goes out as an update, one gone from inside the read's span as a deletion.
//! A stream that says it lost records asks for its channels' relist at once
//! ([`crate::live::LiveHub::request_relist`]).
//!
//! What it costs: one newest-page read per channel per interval, plus one per recently active
//! thread, one channel at a time. A read that fails is left for the next pass.
//!
//! # The backfill
//!
//! The walks back through older pages that fill the cache and keep what it holds checked live in
//! [`crate::backfill`].

use std::collections::BTreeSet;
use std::time::Duration;

use crate::cache::observe::TimelineRead;
use crate::cache::{FillGrant, ServerFill};
use crate::chat::{ChatClient as _, ChatError};
use crate::model::{ChannelId, ChannelInfo};
use crate::read_reason::ReadReason;
use crate::state::AppState;
use crate::threads::{TimelineRequest, TimelineView};

/// How recently a reply must have been pushed in a thread for the relist to read that thread:
/// this many relist intervals, and never less than [`ACTIVE_THREAD_FLOOR`].
pub const ACTIVE_THREAD_INTERVALS: u32 = 4;
/// The least time a thread stays recently active after a pushed reply.
pub const ACTIVE_THREAD_FLOOR: Duration = Duration::from_secs(10 * 60);
/// The pause between one channel's relist and the next's.
pub const RELIST_SPACING: Duration = Duration::from_millis(250);
/// How often a relist loop looks for a relist asked for early.
pub const RELIST_WAKE: Duration = Duration::from_secs(1);
/// How long before a relist read began a message must have been created for its absence from
/// the read to count as a deletion: a provider, or a bridge in front of it, may take a moment to
/// list a message it has just accepted.
pub const RELIST_SETTLE: Duration = Duration::from_secs(30);

/// How a relist reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelistOptions {
    /// Rows each read asks for.
    pub limit: u16,
    /// The most recently active threads of one channel a pass reads.
    pub threads: usize,
    /// The relist interval, which decides how long a thread stays recently active.
    pub interval: Duration,
    /// [`RELIST_SETTLE`], except in a test.
    pub settle: Duration,
}

/// The most pages one walk of the backfill reads of one channel.
pub const BACKFILL_PAGES: usize = 20;
/// The least time between two backfill reads.
pub const BACKFILL_SPACING: Duration = Duration::from_secs(1);
/// How long the provider must have had nothing else running before the backfill reads.
pub const BACKFILL_IDLE: Duration = Duration::from_secs(2);
/// How long a channel whose backfill read failed waits before it is read again.
pub const BACKFILL_RETRY: Duration = Duration::from_secs(10 * 60);
/// How long after a walk of every channel ends the backfill walks them all again.
pub const BACKFILL_REWALK: Duration = Duration::from_secs(6 * 60 * 60);

/// What one relist pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RelistPass {
    /// Scopes read: newest pages and threads.
    pub reads: usize,
    /// Reads that failed.
    pub failed: usize,
    /// Updates published.
    pub updates: usize,
    /// Deletions published.
    pub deletes: usize,
}

/// The channels a relist reads: the configured channels of every provider that takes pushed
/// messages.
fn relisted_channels(state: &AppState) -> Vec<ChannelInfo> {
    state
        .all_channels()
        .into_iter()
        .filter(|channel| {
            state
                .providers
                .client_for(&channel.id)
                .is_some_and(|client| client.takes_pushed_messages(&channel.id))
        })
        .collect()
}

/// Read one scope of `channel` again, offer the read to the cache, and publish what changed.
async fn relist_scope(
    state: &AppState,
    channel: &ChannelId,
    request: TimelineRequest,
    settle: Duration,
) -> Result<(usize, usize), ChatError> {
    let fill = state
        .message_cache
        .begin(&FillGrant::server(ServerFill::Refresher), channel);
    let began_ms = jiff::Timestamp::now().as_millisecond();
    let page = state.providers.fetch_timeline(channel, &request).await?;
    if let Some(fill) = fill {
        fill.offer_timeline(&request, TimelineRead::Newest, &page);
    }
    // A page the provider served from a copy it could not bring up to date, or with a notice,
    // proves nothing past its own newest message.
    let proven_until_ms = if page.as_of.is_some() || page.notice.is_some() {
        0
    } else {
        began_ms.saturating_sub(i64::try_from(settle.as_millis()).unwrap_or(i64::MAX))
    };
    Ok(state.live.publish_relisted(
        channel,
        &page.messages,
        request.thread_id.as_deref(),
        proven_until_ms,
    ))
}

/// Relist one channel: its newest page, and each thread a reply was pushed in since `since_ms`,
/// at most `threads` of them.
async fn relist_channel(
    state: &AppState,
    channel: &ChannelId,
    options: RelistOptions,
    since_ms: i64,
    pass: &mut RelistPass,
) {
    let RelistOptions {
        limit,
        threads,
        settle,
        ..
    } = options;
    let mut requests = vec![TimelineRequest {
        view: TimelineView::Main,
        limit,
        ..TimelineRequest::default()
    }];
    // A channel narrowed to one thread reads that thread as its own timeline already.
    let whole = crate::slack::ids::ChannelRef::parse(channel.as_str())
        .is_some_and(|target| target.thread_ts.is_none());
    if whole {
        requests.extend(
            state
                .live
                .active_threads(channel, since_ms)
                .into_iter()
                .take(threads)
                .map(|thread| TimelineRequest {
                    view: TimelineView::Thread,
                    thread_id: Some(thread),
                    limit,
                    ..TimelineRequest::default()
                }),
        );
    }
    for request in requests {
        pass.reads += 1;
        match relist_scope(state, channel, request, settle).await {
            Ok((updates, deletes)) => {
                pass.updates += updates;
                pass.deletes += deletes;
            }
            Err(error) => {
                pass.failed += 1;
                tracing::debug!(channel = %channel, %error, "a relist read failed; the next pass reads it");
            }
        }
    }
}

/// One relist pass over `channels`, or every relisted channel when `None`, at `now_ms`.
pub async fn relist_once(
    state: &AppState,
    channels: Option<&BTreeSet<ChannelId>>,
    options: RelistOptions,
    now_ms: i64,
) -> RelistPass {
    let active = options
        .interval
        .saturating_mul(ACTIVE_THREAD_INTERVALS)
        .max(ACTIVE_THREAD_FLOOR);
    let since_ms = now_ms.saturating_sub(i64::try_from(active.as_millis()).unwrap_or(i64::MAX));
    let mut pass = RelistPass::default();
    let mut first = true;
    for channel in relisted_channels(state) {
        if channels.is_some_and(|wanted| !wanted.contains(&channel.id)) {
            continue;
        }
        if !first {
            tokio::time::sleep(RELIST_SPACING).await;
        }
        first = false;
        ReadReason::Relist
            .scope(relist_channel(
                state,
                &channel.id,
                options,
                since_ms,
                &mut pass,
            ))
            .await;
    }
    if pass.updates + pass.deletes > 0 {
        tracing::info!(
            reads = pass.reads,
            updates = pass.updates,
            deletes = pass.deletes,
            "the relist found edits or deletions the event stream did not carry"
        );
    }
    pass
}

/// Relist every channel a provider's stream feeds every `interval`, and any channel asked for
/// early as soon as it is asked for. Never returns; spawned by `main` when `[ingest]
/// relist_seconds` is set.
pub async fn relist_forever(state: AppState, interval: Duration, limit: u16, threads: usize) {
    let options = RelistOptions {
        limit,
        threads,
        interval,
        settle: RELIST_SETTLE,
    };
    let mut next = tokio::time::Instant::now() + interval;
    loop {
        let wanted = state.live.take_relist_requests();
        let now_ms = jiff::Timestamp::now().as_millisecond();
        if tokio::time::Instant::now() >= next {
            relist_once(&state, None, options, now_ms).await;
            next = tokio::time::Instant::now() + interval;
        } else if !wanted.is_empty() {
            relist_once(&state, Some(&wanted), options, now_ms).await;
        }
        tokio::time::sleep(RELIST_WAKE.min(interval)).await;
    }
}
