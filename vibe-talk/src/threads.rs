//! Provider-neutral channel timelines, thread summaries, and pagination.
//!
//! Thread identifiers and cursors are opaque to callers. The backend owns discovery, ordering,
//! and verification that a thread belongs to the configured parent channel.

use serde::{Deserialize, Serialize};

use crate::model::{Message, MessageId};

/// Which HTTP contract provides thread access.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadApi {
    /// The native provider's channel and thread endpoints.
    #[default]
    Native,
    /// The normalized timeline and thread endpoints of a compatible bridge.
    Bridge,
    /// Child-thread discovery and thread-specific posting are disabled.
    ///
    /// A provider-managed source that identifies one upstream conversation may still be exposed
    /// as an ordinary app channel. This setting controls the projection within that channel, not
    /// which sources appear in the channel picker.
    Off,
}

/// A view of one configured channel.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TimelineView {
    /// Messages posted directly to the channel, including thread roots.
    #[default]
    Main,
    /// Thread summaries, ordered by their latest message activity.
    Threads,
    /// Channel messages and all accessible thread messages in one chronological history.
    Flat,
    /// The history of one explicitly selected thread.
    Thread,
}

impl TimelineView {
    /// The wire spelling used in query strings.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Threads => "threads",
            Self::Flat => "flat",
            Self::Thread => "thread",
        }
    }
}

/// One page request. Every returned page is ordered oldest first.
///
/// Three kinds of read share it: the NEWEST page (neither cursor), a step BACK (`before`), and a
/// step FORWARD from an earlier newest read (`after`), which answers only what changed since then.
/// `#203 incremental-refresh`.
#[derive(Clone, Debug)]
pub struct TimelineRequest {
    /// Which channel view to read.
    pub view: TimelineView,
    /// Required only for the thread view; opaque to callers.
    pub thread_id: Option<String>,
    /// The previous page's opaque backward continuation, or `None` for the newest page.
    pub before: Option<String>,
    /// The backend's OWN forward cursor, from an earlier [`TimelinePage::next_after`] for this
    /// channel, view and thread, or `None`.
    ///
    /// Never vibe-talk's envelope: [`crate::timeline_forward`] removes that before a backend sees
    /// the request, and only ever sets this for a backend that issued the cursor. Mutually
    /// exclusive with `before`; a backend refuses both with [`crate::chat::ChatError::Refused`].
    pub after: Option<String>,
    /// Maximum entries requested. The backend may clamp this to its page ceiling.
    pub limit: u16,
}

impl Default for TimelineRequest {
    fn default() -> Self {
        Self {
            view: TimelineView::Main,
            thread_id: None,
            before: None,
            after: None,
            limit: 50,
        }
    }
}

impl TimelineRequest {
    /// Refuse a request that names both cursors: a step back and a step forward at once means
    /// nothing, and guessing which was meant would answer a question nobody asked.
    ///
    /// # Errors
    ///
    /// [`crate::chat::ChatError::Refused`] when both `before` and `after` are present.
    pub fn one_direction(&self) -> Result<(), crate::chat::ChatError> {
        if self.before.is_some() && self.after.is_some() {
            return Err(crate::chat::ChatError::Refused(
                "before and after cannot both be given".to_owned(),
            ));
        }
        Ok(())
    }
}

/// Thread membership attached to a message without conflating it with a reply reference.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct MessageThread {
    /// Opaque thread identifier, scoped to the configured channel.
    pub id: String,
    /// The original channel message, if this provider exposes one.
    pub root_message_id: Option<MessageId>,
    /// Whether this message is the original thread root.
    pub is_root: bool,
    /// Number of replies, when the provider can supply it.
    pub reply_count: Option<u64>,
    /// Whether the reply count is exact rather than a provider estimate.
    pub reply_count_exact: bool,
    /// The thread's colour, in whole degrees of hue (`0..360`), as THIS server assigned it.
    /// `#427 server-thread-hues`; see [`crate::thread_hues`].
    ///
    /// FORMED BY THIS SERVER when a message is served to the page (and on the live stream's way
    /// out), from the durable assignment — never trusted from a provider, a bridge or an
    /// adapter, whose copies are overwritten, and absent where no assignment exists for the
    /// thread yet: an older bridge's answer, a read-scope caller meeting a thread the server has
    /// not coloured, a deployment with no store. Absent is the page's cue to fall back to its own
    /// algorithm for that thread, as every page did before `#427`; present, it is the colour on
    /// every device, and the page never recolours it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<u16>,
    /// On a thread's ROOT, the thread's name as [`ThreadSummary::display_name`] gives it: what a
    /// summariser filed, served by the same rule (`#484 summary-card-threshold`). `#528
    /// thread-card-markdown-and-cap`: a page that holds a root draws its thread's summary card from
    /// the read that brought the root, and from its saved copy on a reload, rather than waiting for
    /// a read of the thread list. Absent on replies, until named, and from an older server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// On a thread's root, the thread's sentence as [`ThreadSummary::summary`] gives it, under the
    /// same rule as `display_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// One thread in the thread list, independent of the underlying provider's channel model.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ThreadSummary {
    /// Opaque identifier used for reading or posting to this thread.
    pub id: String,
    /// The original message, when available and readable.
    pub root: Option<Message>,
    /// Provider-supplied title, or a neutral fallback.
    pub title: String,
    /// Number of replies when known.
    pub reply_count: Option<u64>,
    /// Whether the reported count is exact.
    pub reply_count_exact: bool,
    /// Latest message activity, as an RFC 3339 timestamp.
    pub updated_at: String,
    /// A short hyphenated name for the thread, chosen once by a summariser and then kept even if
    /// the thread drifts, so the reader always finds it under the same name. Absent until named.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// One sentence on what the thread is about now, re-summarised as it grows. Absent until
    /// summarised. See [`thread_summary_due`] for when it is refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The thread's colour, in whole degrees of hue (`0..360`). `#427 server-thread-hues`; the
    /// same value its messages carry in [`MessageThread::hue`], repeated here so a thread card
    /// is coloured even when its root could not be read with it. Absent under the same
    /// conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<u16>,
}

/// The fewest messages a thread holds before it is named or summarised at all: the root and
/// two replies. `#484 summary-card-threshold`.
///
/// One question and one response needs no summary card: its real words are shorter than any
/// sentence about them, and a model call spent naming it buys nothing. This constant is the
/// one place the threshold is stated: [`thread_summary_due`] never falls due below it, the
/// naming worker spends no model call on a thread its own read finds below it, and
/// [`thread_summary_shown`] keeps a name or sentence filed earlier off a page while its
/// thread is known to be below it.
pub const THREAD_SUMMARY_MIN_MESSAGES: u64 = 3;

/// The fewest messages a thread whose root is too long for its card holds before it is named:
/// the root and one reply. `#522 thread-card-max-lines`.
///
/// The owner, 2026-10-25: "if the message-root is bigger than a certain size ... let's
/// summarize it even with a single-response". A long root's words are no longer shorter than
/// a sentence about them, which is the whole reason [`THREAD_SUMMARY_MIN_MESSAGES`] waits for a
/// third message; so for such a root, and only for it, the wait ends at the first reply. A
/// root with no reply is no thread yet, long or not.
pub const LONG_ROOT_SUMMARY_MIN_MESSAGES: u64 = 2;

/// The fewest messages a thread holds before it is named or summarised, by whether its root is
/// too long for its card ([`root_overflows_card`]).
#[must_use]
pub fn thread_summary_min_messages(long_root: bool) -> u64 {
    if long_root {
        LONG_ROOT_SUMMARY_MIN_MESSAGES
    } else {
        THREAD_SUMMARY_MIN_MESSAGES
    }
}

/// How many characters one line of a thread card is counted at, in [`card_lines`].
/// `#522 thread-card-max-lines`.
///
/// A card's lines are a ROBUST PROXY for the lines it draws, not a measurement: the server
/// decides from it whether to name a thread early, and it has no screen to measure. 36 is what
/// a line of a card on a phone holds, measured in Chromium on 2026-10-25: at 393x852 the card's
/// text is 331px of 16px type, 40.2 characters of English prose on average by glyph width, but
/// wrapped between words its lines held 36 to 38 ("somebody check whether the migration" is
/// 36); 42.5 average at 412x915, 84.5 at a 1280x800 desk's Threads view. At 40 the proxy's five
/// lines drew as six on that phone. So K proxy lines are at most about K lines on a phone and
/// fewer on a desk, erring toward the compact card the owner asked for. The page counts with
/// the same rule (`wrapCardLine` in web/voice.js) and splits a card by it.
pub const CARD_LINE_CHARS: usize = 36;

/// The synced setting holding K, the most lines a thread card shows of its root.
/// `#522 thread-card-max-lines`; one of [`crate::client_settings::SYNCED_KEYS`].
pub const CARD_MAX_LINES_KEY: &str = "vibe-talk.voice.thread-card-max-lines";

/// K when the owner has chosen none: five lines, the owner's own figure.
pub const DEFAULT_CARD_MAX_LINES: usize = 5;

/// The fewest lines K may be.
pub const MIN_CARD_MAX_LINES: usize = 1;

/// The most lines K may be.
pub const MAX_CARD_MAX_LINES: usize = 12;

/// K from the synced setting's stored value, read as the page reads it (`clampCardMaxLines` in
/// web/voice.js): absent, blank or not a number is the default; anything else rounded and held
/// to [`MIN_CARD_MAX_LINES`]..=[`MAX_CARD_MAX_LINES`]. The server parses no other synced
/// setting; this one decides when it spends a model call.
#[must_use]
pub fn card_max_lines(value: Option<&str>) -> usize {
    let Some(number) = value
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .and_then(|text| text.parse::<f64>().ok())
        .filter(|number| number.is_finite())
    else {
        return DEFAULT_CARD_MAX_LINES;
    };
    // Rounded half up, as JavaScript's `Math.round` rounds, then held in range. The cast is
    // of a value already clamped to a small positive range.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lines = (number + 0.5)
        .floor()
        .clamp(MIN_CARD_MAX_LINES as f64, MAX_CARD_MAX_LINES as f64) as usize;
    lines
}

/// How many card lines `text` takes by the proxy: each line of it that is not blank, wrapped
/// word by word at [`CARD_LINE_CHARS`] characters as a card wraps it — a word that does not fit
/// on the line begins the next, and a word longer than a whole line is broken across lines. The
/// lines are the page's card lines (`cardLines` in web/voice.js), which drops blank lines and
/// closes up white space the same way, and the page wraps them by this same rule
/// (`wrapCardLine`) to find where K lines end. Word by word, not one line per so many
/// characters: a browser breaks only between words, and a count that ignored that would say
/// fewer lines than the card draws.
#[must_use]
pub fn card_lines(text: &str) -> usize {
    text.lines()
        .map(|line| {
            let mut lines = 0;
            // Characters on the line being filled; 0 while no line is open.
            let mut filled = 0;
            for word in line.split_whitespace() {
                let mut chars = word.chars().count();
                if filled > 0 && filled + 1 + chars <= CARD_LINE_CHARS {
                    filled += 1 + chars;
                    continue;
                }
                while chars > CARD_LINE_CHARS {
                    lines += 1;
                    chars -= CARD_LINE_CHARS;
                }
                lines += 1;
                filled = chars;
            }
            lines
        })
        .sum()
}

/// Whether a thread's root is too long for its card: more than `max_lines` card lines of its
/// text ([`card_lines`]). `#522 thread-card-max-lines`.
#[must_use]
pub fn root_overflows_card(root: Option<&Message>, max_lines: usize) -> bool {
    root.is_some_and(|root| card_lines(&root.content) > max_lines)
}

/// Whether a thread's filed name and sentence may be shown on a page, by its reported size:
/// `reply_count` replies, the root counted on top. Below [`thread_summary_min_messages`] the
/// card says the thread's real words instead. An unreported or inexact count proves nothing
/// about the thread's size, so what is filed still shows.
#[must_use]
pub fn thread_summary_shown(
    reply_count: Option<u64>,
    reply_count_exact: bool,
    long_root: bool,
) -> bool {
    !reply_count_exact
        || reply_count.is_none_or(|replies| {
            replies.saturating_add(1) >= thread_summary_min_messages(long_root)
        })
}

/// Whether a thread with `messages` messages, last summarised at `last` messages, is due again.
///
/// Nothing below [`thread_summary_min_messages`] (three messages, or two under a root too long
/// for its card); from there, backed off exponentially, then linearly: summarise at 3, 4 and 8
/// messages, then every 10 after that (18, 28, ...). Early on a thread's subject is still
/// settling and each message changes it a lot; later one more message rarely does, and each
/// summary costs a model call.
/// `last` is `None` for a thread never summarised. The display name is chosen at the first
/// summary and never changes; only the sentence follows this schedule.
#[must_use]
pub fn thread_summary_due(messages: u64, last: Option<u64>, long_root: bool) -> bool {
    const LINEAR_AFTER: u64 = 8;
    const LINEAR_STEP: u64 = 10;
    if messages < thread_summary_min_messages(long_root) {
        return false;
    }
    let Some(last) = last else {
        return true;
    };
    if messages <= last {
        return false;
    }
    let next = if last < LINEAR_AFTER {
        // 1 -> 2 -> 4 -> 8: the next power of two above the last summarised count.
        (last + 1).next_power_of_two().min(LINEAR_AFTER)
    } else {
        last + LINEAR_STEP
    };
    messages >= next
}

/// One complete page from a channel view, never a silently truncated discovery result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct TimelinePage {
    /// Messages for main, flat, or thread views, ordered oldest first.
    pub messages: Vec<Message>,
    /// Summaries for the threads view, ordered by last activity oldest first.
    pub threads: Vec<ThreadSummary>,
    /// The selected thread, for the thread view. Outside it, the one thread a channel registered as
    /// a single conversation is scoped to: that channel has no child threads (`has_threads` is
    /// false), and its main view carries the conversation's replies as well as its root.
    pub thread: Option<ThreadSummary>,
    /// Whether accessible threads exist in this channel.
    pub has_threads: bool,
    /// Whether another backward page exists.
    pub has_more: bool,
    /// Opaque continuation, present exactly when another page exists.
    pub next_before: Option<String>,
    /// Relevant provider limitation or scope information, if any.
    pub notice: Option<String>,
    /// Forward cursor for this view: what to pass as `after` to learn what changed since this
    /// page. `#203 incremental-refresh`.
    ///
    /// From a backend, present only when that backend answers `after` itself. Above the backend
    /// vibe-talk always fills it on a newest page or a delta — with its own generic cursor when
    /// the backend left it empty — and removes it from a `before` page, which says nothing about
    /// the newest end. Absent from an older bridge's answer, which therefore still parses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_after: Option<String>,
    /// Present exactly on an answer to `after`: the page then carries only what is new or changed
    /// since that cursor, and rows absent from it are UNCHANGED, never deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<TimelineDelta>,
    /// The instant, RFC 3339, up to which the backend knows this page is complete, when it is
    /// serving data it could not bring up to date just now. Absent means now.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    /// When the read this page answers began, on this server's clock, in Unix milliseconds — or
    /// the instant it is complete to, when that is earlier (`as_of`). `#473
    /// replication-c7-server-versions`. Every copy it carries is at least this new; a copy a page
    /// holds that was observed before it is older than this page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_ms: Option<i64>,
    /// On a newest page only: messages created before this instant, provider time in Unix
    /// milliseconds, were listed by the read if they were there, so one a page holds from before
    /// [`TimelinePage::observed_ms`] that this page lacks is gone. Anything created at or after
    /// it is in the unsettled band: a provider may list a message late, so its absence proves
    /// nothing. The replication model's rule 4. `#473 replication-c7-server-versions`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_below_ms: Option<i64>,
    /// Messages of this page taken from the server's own copy rather than read upstream: the
    /// replies of threads the message cache held whole ([`crate::held_replies`], `#603
    /// server-side-search`). Never sent: it keeps the read from being offered back to the cache
    /// as having seen them.
    #[serde(skip)]
    pub held: Vec<crate::model::MessageId>,
}

impl TimelinePage {
    /// A newest Flat page as the window a read of a channel's most recent messages answers:
    /// oldest first by id, each message once, at most `limit` of them, the newest kept, with the
    /// page's `as_of`. `#208 persistent-cache`, milestone `bridge-recent-flat`.
    ///
    /// A bridge already pages that way; this holds the shape whatever it sends, so a caller cannot
    /// tell from the shape which endpoint answered. Thread membership is kept: it is true of the
    /// message, and a reader that files messages by thread uses it. Never `exhausted`: a page that
    /// says it has no more is not a proof of a channel's start that any caller relies on yet. The
    /// page's notice is kept: a read the provider says it bounded proves nothing complete.
    #[must_use]
    pub fn into_recent(self, limit: u16) -> crate::chat::RecentRead {
        let mut messages = self.messages;
        crate::model::sort_oldest_first(&mut messages);
        messages.dedup_by(|later, earlier| later.id == earlier.id);
        let excess = messages.len().saturating_sub(usize::from(limit));
        messages.drain(..excess);
        crate::chat::RecentRead {
            messages,
            as_of: self.as_of,
            exhausted: false,
            notice: self.notice,
        }
    }
}

/// What a forward read says about the changes it carries. `#203 incremental-refresh`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct TimelineDelta {
    /// More changes remain past this page: ask again with `next_after` straight away.
    pub more: bool,
    /// True when this delta carries EVERY creation, edit, deletion and summary change in the view
    /// since the cursor. False when it carries additions only: a read filtered by creation time
    /// cannot see an edit or a deletion, which then reach the caller through live events or its
    /// next full read.
    pub complete: bool,
    /// Messages removed since the cursor, channel-wide in every view. Always empty when
    /// `complete` is false.
    #[serde(default)]
    pub deleted: Vec<MessageId>,
    /// Threads that left the thread list since the cursor. Threads view only; empty elsewhere.
    #[serde(default)]
    pub removed_threads: Vec<String>,
}

#[cfg(test)]
mod summary_schedule_tests {
    use super::{
        card_lines, card_max_lines, root_overflows_card, thread_summary_shown, CARD_LINE_CHARS,
        DEFAULT_CARD_MAX_LINES, MAX_CARD_MAX_LINES, MIN_CARD_MAX_LINES,
    };

    fn thread_summary_due(messages: u64, last: Option<u64>) -> bool {
        super::thread_summary_due(messages, last, false)
    }

    /// The counts at which a thread growing one message at a time is summarised.
    fn schedule(up_to: u64) -> Vec<u64> {
        schedule_for(up_to, false)
    }

    fn schedule_for(up_to: u64, long_root: bool) -> Vec<u64> {
        let mut last = None;
        let mut at = Vec::new();
        for messages in 0..=up_to {
            if super::thread_summary_due(messages, last, long_root) {
                at.push(messages);
                last = Some(messages);
            }
        }
        at
    }

    #[test]
    fn a_growing_thread_is_summarised_from_its_third_message_then_on_the_backoff() {
        // `#484 summary-card-threshold`: nothing at 1 or 2 messages; the first summary is at 3,
        // and the old powers-of-two backoff runs from there (4, 8, then every 10).
        assert_eq!(schedule(40), [3, 4, 8, 18, 28, 38]);
        assert!(
            !thread_summary_due(1, None),
            "a lone root is not summarised"
        );
        assert!(
            !thread_summary_due(2, None),
            "one question and one response is not summarised"
        );
        assert!(thread_summary_due(3, None), "the third message is");
    }

    #[test]
    fn a_thread_that_jumped_ahead_is_summarised_once_not_once_per_missed_step() {
        assert!(thread_summary_due(30, Some(2)));
        assert!(!thread_summary_due(30, Some(30)));
        assert!(!thread_summary_due(3, Some(2)), "the step after 2 is 4");
        assert!(
            !thread_summary_due(0, None),
            "an empty thread has nothing to summarise"
        );
    }

    #[test]
    fn a_root_too_long_for_its_card_is_summarised_from_its_first_reply() {
        // `#522 thread-card-max-lines`: the one-reply exemption is for short roots only.
        assert_eq!(schedule_for(20, true), [2, 4, 8, 18]);
        assert!(
            !super::thread_summary_due(1, None, true),
            "a long root alone is no thread yet"
        );
        assert!(super::thread_summary_due(2, None, true));
        assert!(
            !super::thread_summary_due(2, None, false),
            "a short root still waits (#484)"
        );
        // And what is filed is served from the same count.
        assert!(thread_summary_shown(Some(1), true, true));
        assert!(!thread_summary_shown(Some(1), true, false));
        assert!(!thread_summary_shown(Some(0), true, true));
        assert!(
            thread_summary_shown(Some(1), false, false),
            "an inexact count proves nothing"
        );
    }

    #[test]
    fn card_lines_count_each_line_by_the_card_width_proxy() {
        assert_eq!(card_lines(""), 0);
        assert_eq!(card_lines("one short line"), 1);
        assert_eq!(
            card_lines("a\n\n\n  \nb"),
            2,
            "blank lines are dropped, as the card drops them"
        );
        assert_eq!(card_lines(&"x".repeat(CARD_LINE_CHARS)), 1);
        assert_eq!(
            card_lines(&"x".repeat(CARD_LINE_CHARS + 1)),
            2,
            "a long word is broken"
        );
        assert_eq!(card_lines(&"x".repeat(2 * CARD_LINE_CHARS + 1)), 3);
        let half = (CARD_LINE_CHARS - 1) / 2;
        let other = CARD_LINE_CHARS - 1 - half;
        assert_eq!(
            card_lines(&format!("{}   {}", "x".repeat(half), "y".repeat(other))),
            1,
            "white space is closed up before wrapping: two words and a space fill one line"
        );
        assert_eq!(
            card_lines(&format!("{} {}", "x".repeat(half + 1), "y".repeat(other))),
            2,
            "a word that does not fit begins the next line: one character over"
        );
        // Words of 13 characters go two to a line (13 + 1 + 13 = 27; a third makes 41): ten of
        // them, 139 characters, are five lines, where one line per 36 characters would say four.
        assert_eq!(card_lines(&["abcdefghijklm"; 10].join(" ")), 5);
        let root = |content: &str| crate::model::Message {
            edited_timestamp: None,
            thread: None,
            id: crate::model::MessageId("1".to_owned()),
            channel_id: crate::model::ChannelId("c".to_owned()),
            author: "a".to_owned(),
            author_id: crate::model::UserId("7".to_owned()),
            author_is_bot: false,
            timestamp: "t".to_owned(),
            spoken_time: String::new(),
            reply_to: None,
            content: content.to_owned(),
            spoken_content: String::new(),
            content_html: String::new(),
            noise: false,
            reactions: None,
            heading: None,
            mentions: Vec::new(),
            upstream_url: None,
            upstream_app_url: None,
            op_key: None,
            observed_ms: None,
        };
        assert!(!root_overflows_card(Some(&root("1\n2\n3\n4\n5")), 5));
        assert!(root_overflows_card(Some(&root("1\n2\n3\n4\n5\n6")), 5));
        assert!(
            !root_overflows_card(None, 5),
            "an unknown root is not known to be long"
        );
    }

    #[test]
    fn card_max_lines_reads_the_setting_as_the_page_does() {
        assert_eq!(card_max_lines(None), DEFAULT_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("  ")), DEFAULT_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("lots")), DEFAULT_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("NaN")), DEFAULT_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("7")), 7);
        assert_eq!(
            card_max_lines(Some("2.5")),
            3,
            "rounded half up, as Math.round rounds"
        );
        assert_eq!(card_max_lines(Some("0")), MIN_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("99")), MAX_CARD_MAX_LINES);
        assert_eq!(card_max_lines(Some("-inf")), DEFAULT_CARD_MAX_LINES);
    }
}
