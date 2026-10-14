//! [`crate::chat::ChatClient::fetch_range`] for Slack: `conversations.history` and
//! `conversations.replies` bounded by `oldest` and `latest`. remote-ops-audit W10
//! `#287 bulk-reads`.
//!
//! # Two walks, chosen by the feed's own order
//!
//! Slack serves history newest first and a thread's replies oldest first. A range read that
//! starts from the end the feed starts from (history newest first, replies oldest first) pins the
//! window with `oldest`, `latest` and `inclusive`, and follows Slack's own cursor: one call per
//! page of rows returned, never a call for a message outside the window, and `exhausted` only
//! when Slack answered `has_more: false`. That is what a count or a backfill asks for.
//!
//! A read from the other end (history oldest first, as a forward walk through a span asks, or a
//! thread newest first) has to see the whole window before it knows its far end. It reads the
//! window in feed order within the same page budget as [`super::history`]'s other walks, and when
//! the window is larger than that, narrows by time as they do. Either way it never reads past the
//! window, so a span with an end costs what the span holds, not what came after it.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

use super::client::{refused, shape, HttpSlackClient, Raw};
use super::history::{
    advance, far_future_micros, nearest, Feed, Narrowed, Toward, MAX_WINDOW_PAGES, SCAN_PAGE_LIMIT,
};
use super::ids;
use crate::chat::{ChatError, RangeOrder, RangeRead, ReadScope, TimeRange};
use crate::model::ChannelId;

/// The rows one call is counted as returning: a compatible bridge may cap a page at 20, well
/// below the 200 asked for, so a budget counted at 20 a call is never too small.
pub const PAGE_ROWS: u16 = 20;

/// Where a range read continues. Base64 of JSON, as the timeline's own cursors are.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RangeCursor {
    /// The channel, scope, window and order it continues; any other read refuses it.
    ch: String,
    sc: String,
    s: i64,
    u: Option<i64>,
    o: char,
    /// The window's newest instant, inclusive microseconds, pinned by the first step so every
    /// later one reads the same window even when it runs to "now" (an hour past this host's
    /// present then, so Slack's clock decides what "now" holds).
    l: i64,
    /// Slack's own cursor, for a walk in feed order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    c: Option<String>,
    /// How far a walk against feed order got: the instant, microseconds, past which it continues.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    e: Option<i64>,
}

/// What a range cursor is checked against: the read it may continue.
struct Read<'a> {
    channel: &'a ChannelId,
    scope: String,
    range: TimeRange,
    order: char,
}

impl Read<'_> {
    fn cursor(
        &self,
        latest: i64,
        slack: Option<String>,
        edge: Option<i64>,
    ) -> Result<String, ChatError> {
        let cursor = RangeCursor {
            ch: self.channel.as_str().to_owned(),
            sc: self.scope.clone(),
            s: self.range.since_ms,
            u: self.range.until_ms,
            o: self.order,
            l: latest,
            c: slack,
            e: edge,
        };
        serde_json::to_vec(&cursor)
            .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
            .map_err(|_| shape("could not encode the range continuation"))
    }

    fn decode(&self, raw: &str) -> Result<RangeCursor, ChatError> {
        let cursor: RangeCursor = URL_SAFE_NO_PAD
            .decode(raw)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| refused("this Slack backend did not issue that range cursor"))?;
        if cursor.ch != self.channel.as_str()
            || cursor.sc != self.scope
            || cursor.s != self.range.since_ms
            || cursor.u != self.range.until_ms
            || cursor.o != self.order
        {
            return Err(refused(
                "that range cursor continues another read: another channel, scope, window or order",
            ));
        }
        Ok(cursor)
    }
}

impl HttpSlackClient {
    /// [`crate::chat::ChatClient::fetch_range`], without the provider label.
    pub(super) async fn read_range(
        &self,
        channel: &ChannelId,
        scope: &ReadScope,
        range: TimeRange,
        order: RangeOrder,
        limit: u16,
        cursor: Option<&str>,
    ) -> Result<RangeRead, ChatError> {
        let target = Self::channel_ref(channel)?;
        let conversation = target.conversation.as_str();
        let thread = match (scope, &target.thread_ts) {
            (ReadScope::Flat, _) => {
                return Err(refused(
                    "the Slack backend cannot read a conversation's whole history, replies \
                     included, by time",
                ))
            }
            (ReadScope::Main, own) => own.clone(),
            (ReadScope::Thread(id), own) => {
                let ts = ids::canonical_ts(id)
                    .ok_or_else(|| refused(&format!("{id} is not a Slack thread id")))?;
                if own.as_ref().is_some_and(|own| *own != ts) {
                    return Err(refused(
                        "a channel narrowed to one thread holds no other thread",
                    ));
                }
                Some(ts)
            }
        };
        let read = Read {
            channel,
            scope: thread
                .clone()
                .map_or_else(|| "main".to_owned(), |ts| format!("t:{ts}")),
            range,
            order: match order {
                RangeOrder::OldestFirst => 'o',
                RangeOrder::NewestFirst => 'n',
            },
        };
        let cursor = cursor.map(|raw| read.decode(raw)).transpose()?;
        let oldest = range
            .since_ms
            .checked_mul(1000)
            .ok_or_else(|| refused("the window starts outside the range of time Slack can name"))?
            .max(0);
        // Inclusive: Slack's `inclusive` applies to both bounds, so the window's exclusive end
        // is the microsecond before it. A window with no end runs to Slack's now, not this
        // host's: a clock behind Slack's would otherwise cut off the newest messages and still
        // call the window exhausted. So it is named as no upper bound is elsewhere, an hour past
        // the present, and pinned in the cursor so every later step reads the same window.
        let latest = match (range.until_ms, &cursor) {
            (Some(until), _) => {
                until.checked_mul(1000).ok_or_else(|| {
                    refused("the window ends outside the range of time Slack can name")
                })? - 1
            }
            (None, Some(cursor)) => cursor.l,
            (None, None) => far_future_micros(),
        };
        if latest < oldest {
            // Nothing can have been created inside an empty window.
            return Ok(RangeRead {
                exhausted: true,
                ..RangeRead::default()
            });
        }
        let feed = match &thread {
            Some(ts) => Feed::Replies(conversation, ts),
            None => Feed::History(conversation),
        };
        let want = usize::from(limit.max(1));
        let in_feed_order = matches!(
            (&thread, order),
            (None, RangeOrder::NewestFirst) | (Some(_), RangeOrder::OldestFirst)
        );
        let (raws, next, exhausted) = if in_feed_order {
            let slack = cursor.and_then(|cursor| cursor.c);
            let (raws, slack, exhausted) = self
                .range_in_feed_order(feed, oldest, latest, want, slack)
                .await?;
            let next = slack
                .map(|slack| read.cursor(latest, Some(slack), None))
                .transpose()?;
            (raws, next, exhausted)
        } else {
            let toward = match order {
                RangeOrder::OldestFirst => Toward::Newer,
                RangeOrder::NewestFirst => Toward::Older,
            };
            let edge = cursor.and_then(|cursor| cursor.e);
            let (raws, edge, exhausted) = self
                .range_against_feed_order(feed, toward, oldest, latest, want, edge)
                .await?;
            let next = edge
                .map(|edge| read.cursor(latest, None, Some(edge)))
                .transpose()?;
            (raws, next, exhausted)
        };
        if raws.iter().any(Raw::is_thread_root) {
            self.mark_threaded(conversation);
        }
        Ok(RangeRead {
            messages: self.messages(channel, &raws).await,
            next,
            exhausted,
            as_of: None,
            emulated: false,
        })
    }

    /// Follow Slack's own cursor through a pinned window, within one call per [`PAGE_ROWS`] rows
    /// wanted and one more. Returns the real messages oldest first, Slack's cursor when the walk
    /// stopped short of the window's end, and whether Slack said there was nothing more.
    async fn range_in_feed_order(
        &self,
        feed: Feed<'_>,
        oldest: i64,
        latest: i64,
        want: usize,
        mut cursor: Option<String>,
    ) -> Result<(Vec<Raw>, Option<String>, bool), ChatError> {
        let budget = want.div_ceil(usize::from(PAGE_ROWS)) + 1;
        let mut real: Vec<Raw> = Vec::new();
        let mut exhausted = false;
        for _ in 0..budget {
            // Never more than is still wanted: Slack's cursor continues after the last row it
            // returned, so a row asked for and then dropped would be skipped by the next step.
            let ask = u16::try_from(
                want.saturating_sub(real.len())
                    .clamp(1, usize::from(SCAN_PAGE_LIMIT)),
            )
            .unwrap_or(SCAN_PAGE_LIMIT);
            let page = self
                .page(
                    feed,
                    Some(oldest),
                    Some(latest),
                    true,
                    ask,
                    cursor.as_deref(),
                )
                .await?;
            real.extend(page.raws.into_iter().filter(Raw::is_real));
            if !page.has_more {
                exhausted = true;
                cursor = None;
                break;
            }
            cursor = advance(cursor, page.next_cursor)?;
            if real.len() >= want {
                break;
            }
        }
        real.sort_by_key(|raw| raw.micros);
        real.dedup_by_key(|raw| raw.micros);
        Ok((real, cursor, exhausted))
    }

    /// Read a window from the end its feed does not start from: the whole window in feed order
    /// within [`MAX_WINDOW_PAGES`], or, when it is larger, the run adjacent to the near end found
    /// by narrowing. Returns the `want` real messages nearest that end, oldest first, the instant
    /// a next step continues past, and whether the window was read to its far end.
    async fn range_against_feed_order(
        &self,
        feed: Feed<'_>,
        toward: Toward,
        oldest: i64,
        latest: i64,
        want: usize,
        edge: Option<i64>,
    ) -> Result<(Vec<Raw>, Option<i64>, bool), ChatError> {
        // Exclusive bounds of what is still to be read.
        let (lo, hi) = match toward {
            Toward::Newer => (edge.unwrap_or(oldest - 1), latest + 1),
            Toward::Older => (oldest - 1, edge.unwrap_or(latest + 1)),
        };
        if hi - lo <= 1 {
            return Ok((Vec::new(), None, true));
        }
        let (primary, complete) = self
            .window(feed, Some(lo), Some(hi), MAX_WINDOW_PAGES)
            .await?;
        let far = |found: &[Raw]| match toward {
            Toward::Newer => found.last().map(|raw| raw.micros),
            Toward::Older => found.first().map(|raw| raw.micros),
        };
        if complete {
            let every = nearest(&primary, usize::MAX, toward).len();
            let found = nearest(&primary, want, toward);
            if every <= want {
                return Ok((found, None, true));
            }
            let edge = far(&found);
            return Ok((found, edge, false));
        }
        let near = match toward {
            Toward::Newer => lo,
            Toward::Older => hi,
        };
        match self.narrow(feed, toward, near, &primary, want).await? {
            Narrowed::Found(found) => {
                let edge = far(&found);
                Ok((found, edge, false))
            }
            Narrowed::Proven(to) => Ok((Vec::new(), Some(to), false)),
        }
    }
}
