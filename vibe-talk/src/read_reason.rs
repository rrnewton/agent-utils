//! Why a read of a chat provider was made. `#278 timeline-single-flight`, remote-ops audit W0.3.
//!
//! # Why this exists
//!
//! The provider's own log counts how many reads it served, and this server's upstream profile
//! counts how long they took, but neither could say what each read was FOR: the page's poll, a
//! live message it re-read after, a thread being named in the background, a voice catch-up. A
//! figure like "forty timeline reads a minute" could not be attributed, so it could not be
//! reproduced or argued with. Every read now carries a reason, and the figures are kept per reason.
//!
//! # Where a reason comes from
//!
//! * **The page says so.** Its timeline requests carry `reason=` with the word it already uses for
//!   the refresh ([`ReadReason::from_query`]). The access middleware puts that word in the request's
//!   access line and runs the handler inside it ([`ReadReason::scope`]).
//! * **The server says so for its own reads**: thread naming, the voice catch-up's windows, the
//!   startup probe, a summary's context window, speech preparation, the recent-view shadow check,
//!   and an MCP tool call. Each runs its read inside its own reason, which overrides any reason of
//!   the request it was made for.
//!
//! A read made with no reason around it counts as [`ReadReason::Untagged`], so a path that was
//! missed shows up in the figures as exactly that rather than hiding under another's name.
//!
//! The reason is carried in a task-local, not threaded through every signature: the chat trait
//! stays as it is, and [`crate::upstream_profile::ProfiledClient`] reads it where it times the
//! read. A task spawned from a request does not inherit it; a spawn that does work on behalf of
//! its caller — the timeline single-flight's detached read — carries it across itself.
//!
//! # What a reason is not
//!
//! Advice for the figures, never a permission. Nothing reads it to decide what a caller may see or
//! do, so a client that lies about it can mislabel only its own reads. The one thing it changes is
//! how fresh a timeline answer must be: an explicit refresh ([`ReadReason::wants_fresh`]) always
//! makes a read of its own, never answered from the few seconds' memory of a newest page nor by a
//! read already running, so a client that claims one costs at most one more read of the provider,
//! and sees nothing it could not see otherwise. A word the page does not
//! send — including every server-side reason — is [`ReadReason::Other`] when it arrives from
//! outside, so a request cannot pass itself off as the server's own background work.

use std::future::Future;

use serde::Serialize;

tokio::task_local! {
    static CURRENT: ReadReason;
}

/// Why a read was made. The words are the page's own names for its refreshes, and the server's for
/// its own reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadReason {
    /// The page: opening a channel or a view, or coming back to it.
    Enter,
    /// The page: the background poll.
    Poll,
    /// The page: re-reading after a live message arrived.
    Live,
    /// The page: re-reading after a live edit or deletion.
    Mutation,
    /// The page: coming back to a page that was hidden.
    Visible,
    /// The page: the live stream reset or reconnected.
    Reset,
    /// The page: a stream replay it could not fold on its own.
    Replay,
    /// The page: pull to refresh.
    Pull,
    /// The page: a forward cursor the server could no longer continue.
    Expired,
    /// The page: a step back into older history, or older pages walked to reach a message.
    Older,
    /// The page: reading All behind Main to count unread replies.
    Counts,
    /// The page: settling which thread a reply to an unplaced message belongs in.
    ReplyPlace,
    /// The page: the thread list, for the selector or the thread directory.
    Threads,
    /// The server: reading a thread to name it in the background.
    Naming,
    /// The server: refreshing a channel's held window for a voice catch-up or warm-up.
    Recent,
    /// The server: the startup or on-demand reachability probe.
    Probe,
    /// The server: the channel window a summary takes its context from.
    SummaryWindow,
    /// The server: the window speech preparation finds its messages in.
    Speech,
    /// The server: the recent-view shadow's check of a window against Flat.
    Shadow,
    /// The server: a voice agent's MCP tool call.
    Tool,
    /// A reason was given, but it is not one the page sends.
    Other,
    /// No reason was given at all.
    Untagged,
}

/// Every reason, in declaration order. For tests and documentation.
pub const ALL: [ReadReason; 22] = [
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
    ReadReason::Counts,
    ReadReason::ReplyPlace,
    ReadReason::Threads,
    ReadReason::Naming,
    ReadReason::Recent,
    ReadReason::Probe,
    ReadReason::SummaryWindow,
    ReadReason::Speech,
    ReadReason::Shadow,
    ReadReason::Tool,
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
            Self::Counts => "counts",
            Self::ReplyPlace => "reply-place",
            Self::Threads => "threads",
            Self::Naming => "naming",
            Self::Recent => "recent",
            Self::Probe => "probe",
            Self::SummaryWindow => "summary-window",
            Self::Speech => "speech",
            Self::Shadow => "shadow",
            Self::Tool => "tool",
            Self::Other => "other",
            Self::Untagged => "untagged",
        }
    }

    /// Whether the reader asked for the channel as it stands NOW: pull to refresh, or opening a
    /// channel or a view. Such a timeline request is never answered by a read that began before it
    /// — neither from the memory of a newest answer that has just landed nor by a read already
    /// running — but always reads for itself. `#278 timeline-single-flight`; see
    /// [`crate::timeline_flight`].
    #[must_use]
    pub const fn wants_fresh(self) -> bool {
        matches!(self, Self::Pull | Self::Enter)
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
                | Self::Counts
                | Self::ReplyPlace
                | Self::Threads
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
