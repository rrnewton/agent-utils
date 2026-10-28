//! Why a read of a chat provider was made. `remote-ops-audit`, read reason tags.
//!
//! # Why this exists
//!
//! The provider's own log counts how many reads it served, and this server's upstream profile
//! counts how long they took, but neither could say what each read was FOR: the page's poll, a
//! re-read after a live message, a thread being named in the background, a voice catch-up. A figure
//! like "forty timeline reads a minute" could not be attributed, so it could not be reproduced or
//! argued with. Every read now carries a reason, and the figures are kept per reason.
//!
//! # Where a reason comes from
//!
//! * **The page says so.** Its timeline and recent-window reads (`/timeline`, `/page`, `/todo`)
//!   carry `reason=` with the word for what asked for the read ([`ReadReason::from_query`]). The
//!   access middleware puts that word in the request's access line as `read_reason` and runs the
//!   handler inside it ([`ReadReason::scope`]).
//! * **The server says so for its own reads**: thread naming, the voice catch-up's windows, the
//!   reachability probe, a summary's context window, speech preparation, the recent-view shadow
//!   check, an MCP tool call, and the live poller (`#614 timeline-read-failures`). Each runs its
//!   read inside its own reason, which overrides any reason of the request it was made for.
//!
//! A read made with no reason around it counts as [`ReadReason::Untagged`] (a send, for one), so a
//! path nobody tagged shows up in the figures as exactly that rather than hiding under another's
//! name.
//!
//! The reason is carried in a task-local, not threaded through every signature: the chat trait
//! stays as it is, and [`crate::upstream_profile::ProfiledClient`] reads it where it times the
//! read. A task spawned from a request does not inherit it.
//!
//! # What a reason is not
//!
//! Attribution, never a decision, with two exceptions for the server's own `backfill` reason
//! alone, which no client can claim (see below): the upstream profile does not count a backfill
//! read as the provider being in use (`#453 cache-backfill`), and a backfill read gives up at the
//! first rate limit instead of waiting it out (`#602 history-settings-and-limits`). And for the
//! page's own words, decisions about the caller's own read only, never what it may see: Slack's
//! shared budget places a read by its reason (`#611 timeline-read-failures`,
//! [`crate::slack::budget`]); with serving on, a `side` or `counts` read gives up at the first rate
//! limit (the cache answers it) and fails once the stand-in wait has passed with nothing to answer
//! from, and a `side` read of a live-fed channel's thread list is answered from the cache in
//! between upstream reads of it (`#614`, `crate::ops`); and with serving on, the page's reads of a
//! view's newest end or of what changed since a cursor are answered from the cache first when it
//! holds anything of the view, the read upstream behind the answer (`#622 cache-first-reads`,
//! [`crate::cache_first::eligible`]). Otherwise
//! NOTHING reads it to decide what a caller may see or do, which read is made, whether one is
//! shared or kept, or how fresh an answer must be: it is written to the access log and counted in
//! the upstream profile, and that is all. So a client that lies about
//! it can mislabel only its own reads, and hurry or slow only its own answers. A word the page does not send — including every server-side
//! reason — is [`ReadReason::Other`] when it arrives from outside, so a request cannot pass itself
//! off as the server's own background work, and nothing a client sends is copied into the log.

use std::future::Future;

use serde::Serialize;

tokio::task_local! {
    static CURRENT: ReadReason;
    static HURRIED: bool;
}

/// Run `work` with every read it makes giving up at a provider's first rate limit when `hurried`,
/// rather than waiting it out: a read the message cache answers in the provider's place when it
/// fails. `#614 timeline-read-failures`; read by the Slack client ([`hurried`]).
pub async fn hurry<F: Future>(hurried: bool, work: F) -> F::Output {
    HURRIED.scope(hurried, work).await
}

/// Whether the read being made gives up at the first rate limit ([`hurry`]).
#[must_use]
pub fn hurried() -> bool {
    HURRIED.try_with(|hurried| *hurried).unwrap_or(false)
}

/// Why a read was made. The words are the page's names for what asked for a read, and the
/// server's for its own reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadReason {
    /// The page: opening a channel or a view, or coming back to it.
    Enter,
    /// The page: the background poll.
    Poll,
    /// The page: re-reading after a live message arrived.
    Live,
    /// The page: re-reading after something changed what is shown — a live edit or deletion, or
    /// the reader's own change (a Done undone, the noise rules, which of their messages count as
    /// read).
    Mutation,
    /// The page: coming back to a page that was hidden.
    Visible,
    /// The page: the live stream reset or reconnected.
    Reset,
    /// The page: a stream replay it could not fold on its own.
    Replay,
    /// The page: pull to refresh, at either end of the list.
    Pull,
    /// The page: a forward cursor the server could no longer continue.
    Expired,
    /// The page: a step back into older history, or older pages walked to reach a message.
    Older,
    /// The page: a step back the search took, looking for older matches.
    Search,
    /// The page: reading All behind Main to count unread replies.
    Counts,
    /// The page: settling which thread a reply to an unplaced message belongs in.
    ReplyPlace,
    /// The page: reading All to place messages the live stream delivered without their thread.
    /// `#308 live-reply-in-main`.
    Place,
    /// The page: the thread list's newest page, read for "Older threads…" when no cursor is known yet.
    Threads,
    /// The page: the right column's thread, or its list of threads, read beside Main.
    Side,
    /// The server: reading a thread to name it in the background.
    Naming,
    /// The server: refreshing a channel's held window for a voice catch-up or warm-up.
    Recent,
    /// The server: the startup or on-demand reachability probe.
    Probe,
    /// The server: the channel window a summary takes its context from.
    SummaryWindow,
    /// The server: the window, or the thread messages, speech preparation finds its messages in.
    Speech,
    /// The server: the recent-view shadow's check of a window against Flat.
    Shadow,
    /// The server: a voice agent's MCP tool call.
    Tool,
    /// The server: telling whether moving a channel's provider read marker after a reply would
    /// clear more than the channel holds. `#300 reply-marks-read`.
    ReplyMark,
    /// The server: reading a channel's newest page and its recently active threads again, for the
    /// edits and deletions a provider's event stream never carries. `#452 slack-live-ingest`.
    Relist,
    /// The server: walking a channel's older history to fill the message cache. `#453
    /// cache-backfill`.
    Backfill,
    /// The server: the live poller reading each polled channel's newest messages (and the threads
    /// that moved) to publish them. `#614 timeline-read-failures`; it used to be untagged.
    LivePoll,
    /// A reason was given, but it is not one the page sends.
    Other,
    /// No reason was given at all.
    Untagged,
}

/// Every reason, in declaration order. For tests and documentation.
pub const ALL: [ReadReason; 29] = [
    ReadReason::Enter,
    ReadReason::Poll,
    ReadReason::Live,
    ReadReason::Mutation,
    ReadReason::Visible,
    ReadReason::Reset,
    ReadReason::Replay,
    ReadReason::Pull,
    ReadReason::Expired,
    ReadReason::Older,
    ReadReason::Search,
    ReadReason::Counts,
    ReadReason::ReplyPlace,
    ReadReason::Place,
    ReadReason::Threads,
    ReadReason::Side,
    ReadReason::Naming,
    ReadReason::Recent,
    ReadReason::Probe,
    ReadReason::SummaryWindow,
    ReadReason::Speech,
    ReadReason::Shadow,
    ReadReason::Tool,
    ReadReason::ReplyMark,
    ReadReason::Relist,
    ReadReason::Backfill,
    ReadReason::LivePoll,
    ReadReason::Other,
    ReadReason::Untagged,
];

impl ReadReason {
    /// The word, as the access line, the report and the hourly log write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Enter => "enter",
            Self::Poll => "poll",
            Self::Live => "live",
            Self::Mutation => "mutation",
            Self::Visible => "visible",
            Self::Reset => "reset",
            Self::Replay => "replay",
            Self::Pull => "pull",
            Self::Expired => "expired",
            Self::Older => "older",
            Self::Search => "search",
            Self::Counts => "counts",
            Self::ReplyPlace => "reply-place",
            Self::Place => "place",
            Self::Threads => "threads",
            Self::Side => "side",
            Self::Naming => "naming",
            Self::Recent => "recent",
            Self::Probe => "probe",
            Self::SummaryWindow => "summary-window",
            Self::Speech => "speech",
            Self::Shadow => "shadow",
            Self::Tool => "tool",
            Self::ReplyMark => "reply-mark",
            Self::Relist => "relist",
            Self::Backfill => "backfill",
            Self::LivePoll => "live-poll",
            Self::Other => "other",
            Self::Untagged => "untagged",
        }
    }

    /// Whether the page may send this word. The server's own reasons, and the two fallbacks, are
    /// never accepted from a request.
    #[must_use]
    pub const fn from_the_page(self) -> bool {
        matches!(
            self,
            Self::Enter
                | Self::Poll
                | Self::Live
                | Self::Mutation
                | Self::Visible
                | Self::Reset
                | Self::Replay
                | Self::Pull
                | Self::Expired
                | Self::Older
                | Self::Search
                | Self::Counts
                | Self::ReplyPlace
                | Self::Place
                | Self::Threads
                | Self::Side
        )
    }

    /// The reason a request's `reason=` names: one of the page's words, or [`ReadReason::Other`]
    /// for anything else, a server-side word included.
    #[must_use]
    pub fn from_query(word: &str) -> Self {
        ALL.into_iter()
            .find(|reason| reason.from_the_page() && reason.as_str() == word)
            .unwrap_or(Self::Other)
    }

    /// The `reason=` of a raw query string, when it has one. Only the first is read. The value is
    /// not percent-decoded: every word the page sends is plain, so an encoded one is not one of
    /// them, and is [`ReadReason::Other`] as any other unknown word is.
    #[must_use]
    pub fn of_query_string(query: Option<&str>) -> Option<Self> {
        query?
            .split('&')
            .find_map(|pair| pair.strip_prefix("reason="))
            .map(Self::from_query)
    }

    /// The reason the running task's reads are made for: the innermost [`ReadReason::scope`] around
    /// it, or [`ReadReason::Untagged`].
    #[must_use]
    pub fn current() -> Self {
        CURRENT.try_with(|reason| *reason).unwrap_or(Self::Untagged)
    }

    /// Run `work` with every read it makes counted under this reason, overriding any outer one.
    pub async fn scope<F: Future>(self, work: F) -> F::Output {
        CURRENT.scope(self, work).await
    }
}

impl std::fmt::Display for ReadReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_reads_back_as_its_reason_and_the_list_is_complete() {
        let mut words: Vec<&str> = ALL.iter().map(|reason| reason.as_str()).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), ALL.len(), "two reasons share a word");
        for reason in ALL {
            let expected = if reason.from_the_page() {
                reason
            } else {
                ReadReason::Other
            };
            assert_eq!(
                ReadReason::from_query(reason.as_str()),
                expected,
                "{reason}"
            );
            assert_eq!(
                serde_json::to_value(reason).expect("serializes"),
                reason.as_str(),
                "the report and the log disagree about {reason:?}"
            );
        }
    }

    #[test]
    fn the_page_sends_exactly_the_words_its_reads_use() {
        // `web/voice.js` names these, and only these, in `reason=`; a word added there and not here
        // would be logged as `other`.
        let page: Vec<&str> = ALL
            .into_iter()
            .filter(|reason| reason.from_the_page())
            .map(ReadReason::as_str)
            .collect();
        assert_eq!(
            page,
            [
                "enter",
                "poll",
                "live",
                "mutation",
                "visible",
                "reset",
                "replay",
                "pull",
                "expired",
                "older",
                "search",
                "counts",
                "reply-place",
                "place",
                "threads",
                "side"
            ]
        );
    }

    #[test]
    fn a_query_string_names_its_reason_and_an_unknown_one_is_other() {
        assert_eq!(
            ReadReason::of_query_string(Some("view=main&reason=poll&limit=50")),
            Some(ReadReason::Poll)
        );
        assert_eq!(
            ReadReason::of_query_string(Some("reason=reply-place")),
            Some(ReadReason::ReplyPlace)
        );
        assert_eq!(
            ReadReason::of_query_string(Some("reason=sneaky")),
            Some(ReadReason::Other)
        );
        assert_eq!(
            ReadReason::of_query_string(Some("reason=naming")),
            Some(ReadReason::Other),
            "a request passed itself off as the server's own work"
        );
        assert_eq!(ReadReason::of_query_string(Some("view=main")), None);
        assert_eq!(ReadReason::of_query_string(None), None);
        assert_eq!(
            ReadReason::of_query_string(Some("treason=poll")),
            None,
            "a parameter that merely ends in reason"
        );
    }

    #[tokio::test]
    async fn the_innermost_scope_wins_and_none_is_untagged() {
        assert_eq!(ReadReason::current(), ReadReason::Untagged);
        let seen = ReadReason::Poll
            .scope(async {
                let outer = ReadReason::current();
                let inner = ReadReason::Speech
                    .scope(async { ReadReason::current() })
                    .await;
                (outer, inner, ReadReason::current())
            })
            .await;
        assert_eq!(
            seen,
            (ReadReason::Poll, ReadReason::Speech, ReadReason::Poll)
        );
    }
}
