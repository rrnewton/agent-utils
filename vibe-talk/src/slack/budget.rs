//! One Slack read budget, shared by priority. `#611 timeline-read-failures`.
//!
//! # Why this exists
//!
//! Slack limits each Web API method per workspace (`conversations.history` and
//! `conversations.replies` are tier 3, about fifty requests a minute), and every reader of a
//! channel spends the same allowance: the page, the live poller and relist, thread naming and
//! summaries, and the message cache's backfill. Before this, each request waited out its own rate
//! limit and the others kept sending into it; backfill held off only once Slack had already
//! refused someone (`#602 history-settings-and-limits`), by which time a page's read had failed.
//! With full backfill on, the owner's page failed one timeline read in three.
//!
//! # What it does
//!
//! Every request is counted against its method over the last minute, with its priority, from what
//! it is for ([`crate::read_reason::ReadReason`]):
//!
//! * **write** — a post, a read mark, and the read a post makes first (see "Writes first" below);
//! * **interactive** — the page's reads, speech, a voice agent's tools, and anything that says
//!   nothing of what it is for (an untagged request may be a person's);
//! * **live** — the live poller, relist, the reachability probe, the reply-mark check;
//! * **background** — thread naming and a summary's window;
//! * **crawl** — backfill and the shadow's check.
//!
//! Crawl work yields: once everyone else has sent [`CRAWL_YIELD_AT`] requests of a method in the
//! last minute, backfill holds off it ([`Sent::crawl_wait`], read through `crawl_hold` before each
//! of its reads), and a crawl request that asks anyway fails at once as a rate limit, without being
//! sent or counted as Slack having refused it. And once Slack has answered a method with a
//! `Retry-After`, nobody sends it again before that runs out: crawl work fails at once, and every
//! other request waits for it within its own budget, as each used to wait for its own. Nothing but
//! crawl work is refused by a count, so a person's request, live work and naming never are.
//!
//! The ceilings are this server's own, set below Slack's published tier 3 floor; the counts are
//! per process, which is per workspace in every deployment this serves.
//!
//! # Writes first (`#699 vt-send-priority`)
//!
//! A transport in front of Slack may limit the account as a whole rather than each method: one
//! token bucket for every call it makes, from every host. Then a refused read means the next post
//! will be refused too, and every background read sent meanwhile takes a token a post was waiting
//! for. Measured on 2026-10-29: one send took 35 s, 30 of them in its own read of the thread it was
//! posting into, refused three times behind a ten-second `Retry-After` while the live poller,
//! relist and older-page reads kept the bucket empty.
//!
//! So a **write** (a post, a read mark: [`is_write`]) and the reads a post makes first
//! ([`ReadReason::Write`]) come before everything else: they never wait out a `Retry-After`
//! another request was given, and when refused themselves they try again on their own short
//! schedule ([`write_retry_wait`]). And while the provider is **limiting** (it answered any method
//! with a rate limit in the last [`LIMITING_WINDOW`]), the reads that drain the bucket
//! ([`gives_way`]: `conversations.history` and `conversations.replies`) give way
//! ([`while_limiting`]): naming and crawl work pause; live work makes one try, never waiting out a
//! `Retry-After`, and pauses while a send is in flight; a read the message cache would answer in
//! the provider's place fails at once without being sent; and a person's read holds while a send
//! is in flight. A paused read says to come back in at most [`PAUSE_RETRY_AFTER`]. Author and
//! channel lookups are never paused: they are few, and a failed one is remembered for minutes.
//! The minute's report says how many requests were writes and how many reads were paused.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

use crate::read_reason::ReadReason;

/// The window the counts are kept over.
pub const WINDOW: Duration = Duration::from_secs(60);
/// Backfill and the shadow's check hold off a method while everyone else together has sent this
/// many of it in the last minute: half of Slack's tier 3 floor. Backfill's own pace
/// (`backfill_requests_per_minute`, twenty unless set) bounds its own share, so the two together
/// stay under the floor.
pub const CRAWL_YIELD_AT: usize = 25;
/// How long after a rate limit on any method the provider counts as limiting.
pub const LIMITING_WINDOW: Duration = Duration::from_secs(60);
/// The most a paused read says to wait before trying again: the limiting window moves on with
/// every rate limit, and a live poller that waited it out would go silent while it lasts.
pub const PAUSE_RETRY_AFTER: Duration = Duration::from_secs(10);
/// The reads that give way to writes while the provider is limiting.
const GIVES_WAY: [&str; 2] = ["conversations.history", "conversations.replies"];
/// The longest a person's read holds for a send in flight while the provider is limiting.
pub const WRITE_HOLD: Duration = Duration::from_secs(10);
/// How many times one write may be sent before a rate limit is declared unclearable: more than a
/// read, because each try is short.
pub const MAX_WRITE_ATTEMPTS: u32 = 12;
/// The waits between a refused write's tries, the last repeated; each no longer than the
/// `Retry-After` it was given.
const WRITE_RETRY_STEPS: [Duration; 5] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// The Web API methods that change something on the owner's behalf. `#699 vt-send-priority`.
const WRITE_METHODS: [&str; 6] = [
    "chat.postMessage",
    "chat.update",
    "chat.delete",
    "reactions.add",
    "reactions.remove",
    "conversations.mark",
];

/// Whether `method` is a write ([`Priority::Write`]).
#[must_use]
pub fn is_write(method: &str) -> bool {
    WRITE_METHODS.contains(&method)
}

/// Whether a read of `method` gives way to writes while the provider is limiting
/// ([`while_limiting`]).
#[must_use]
pub fn gives_way(method: &str) -> bool {
    GIVES_WAY.contains(&method)
}

/// Whether a read at `priority` that gives way makes only one try while the provider is limiting:
/// it never waits out a `Retry-After`, its own or another's. Live work, naming and crawl work.
#[must_use]
pub fn one_try_while_limiting(priority: Priority) -> bool {
    priority <= Priority::Live
}

/// How long a write waits before its next try, having been sent `attempts` times and refused
/// with `retry_after`: its own short schedule, never longer than what it was told.
#[must_use]
pub fn write_retry_wait(attempts: u32, retry_after: Duration) -> Duration {
    let step = usize::try_from(attempts.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(WRITE_RETRY_STEPS.len() - 1);
    WRITE_RETRY_STEPS[step].min(retry_after)
}

/// What a request does while the provider is limiting. `#699 vt-send-priority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WhileLimiting {
    /// Goes as it would otherwise.
    Go,
    /// Waits for the sends in flight to finish, at most [`WRITE_HOLD`], then goes.
    HoldForWrites,
    /// Fails at once as a rate limit, without being sent.
    Pause,
}

/// What a read that gives way ([`gives_way`]) at `priority` does while the provider is limiting: a
/// write goes; naming and crawl work pause; live work goes ([`one_try_while_limiting`]) except
/// while a send is in flight; a person's read the message cache would answer (`hurried`) pauses,
/// and any other holds for a send in flight.
#[must_use]
pub fn while_limiting(priority: Priority, hurried: bool, writes_in_flight: bool) -> WhileLimiting {
    match priority {
        Priority::Write => WhileLimiting::Go,
        Priority::Crawl | Priority::Background => WhileLimiting::Pause,
        Priority::Live if writes_in_flight => WhileLimiting::Pause,
        Priority::Live => WhileLimiting::Go,
        Priority::Interactive if hurried => WhileLimiting::Pause,
        Priority::Interactive if writes_in_flight => WhileLimiting::HoldForWrites,
        Priority::Interactive => WhileLimiting::Go,
    }
}

/// How much a request may take of a method's allowance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Backfill and the shadow: whatever nobody else is using.
    Crawl,
    /// Thread naming and a summary's window.
    Background,
    /// What keeps the page current without being asked: the live poller, relist, the probe.
    Live,
    /// What a person is waiting on.
    Interactive,
    /// A write, and the reads a post makes before it is sent. `#699 vt-send-priority`.
    Write,
}

impl Priority {
    /// The priority of a read made for `reason`.
    #[must_use]
    pub const fn of(reason: ReadReason) -> Self {
        match reason {
            ReadReason::Write => Self::Write,
            ReadReason::Backfill | ReadReason::Shadow => Self::Crawl,
            ReadReason::Naming | ReadReason::SummaryWindow => Self::Background,
            ReadReason::Relist
            | ReadReason::Probe
            | ReadReason::ReplyMark
            | ReadReason::LivePoll => Self::Live,
            _ => Self::Interactive,
        }
    }
}

impl Priority {
    /// How many requests of a method with `ceiling` a minute may have been sent in the last minute,
    /// by everyone, for a request at this priority to go: the whole ceiling for a person's request
    /// and live work, three quarters for naming and summaries, half for crawl work. So what
    /// keeps the page current always has room the rest cannot take. A write is never held to a
    /// ceiling; this is its whole for completeness.
    #[must_use]
    pub fn share(self, ceiling: usize) -> usize {
        match self {
            Self::Write | Self::Interactive | Self::Live => ceiling,
            Self::Background => (ceiling * 3 / 4).max(1),
            Self::Crawl => (ceiling / 2).max(1),
        }
    }
}

/// The requests sent per method in the last [`WINDOW`], by priority.
#[derive(Debug, Default)]
pub struct Sent {
    by_method: HashMap<String, VecDeque<(Instant, Priority)>>,
}

impl Sent {
    fn window(&mut self, method: &str, now: Instant) -> &mut VecDeque<(Instant, Priority)> {
        let sent = self.by_method.entry(method.to_owned()).or_default();
        while sent
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) >= WINDOW)
        {
            sent.pop_front();
        }
        sent
    }

    /// The requests of `method` sent in the last minute at a priority above `priority`, and when
    /// the oldest of those that must leave the window for the count to drop below `at` leaves it.
    fn above(
        &mut self,
        method: &str,
        priority: Priority,
        at: usize,
        now: Instant,
    ) -> (usize, Option<Duration>) {
        let above: Vec<Instant> = self
            .window(method, now)
            .iter()
            .filter(|(_, sent)| *sent > priority)
            .map(|(when, _)| *when)
            .collect();
        let wait = (above.len() >= at)
            .then(|| above.get(above.len() - at).copied())
            .flatten()
            .map(|leaving| (leaving + WINDOW).saturating_duration_since(now));
        (above.len(), wait)
    }

    /// Whether a request of `method` at `priority` may go now: anything but crawl work always;
    /// crawl work while everyone else has sent fewer than [`CRAWL_YIELD_AT`] in the last minute.
    pub fn admits(&mut self, method: &str, priority: Priority, now: Instant) -> bool {
        priority != Priority::Crawl
            || self.above(method, priority, CRAWL_YIELD_AT, now).0 < CRAWL_YIELD_AT
    }

    /// How long until a request of `method` at `priority` fits under `ceiling` requests a minute
    /// ([`Priority::share`]): `None` when it fits now.
    pub fn ceiling_wait(
        &mut self,
        method: &str,
        priority: Priority,
        ceiling: usize,
        now: Instant,
    ) -> Option<Duration> {
        let share = priority.share(ceiling);
        let sent = self.window(method, now);
        if sent.len() < share {
            return None;
        }
        // Once this one has left the window, `share - 1` remain.
        let (leaving, _) = sent[sent.len() - share];
        Some((leaving + WINDOW).saturating_duration_since(now)).filter(|wait| !wait.is_zero())
    }

    /// Count one request of `method` at `priority`, sent now.
    pub fn record(&mut self, method: &str, priority: Priority, now: Instant) {
        self.window(method, now).push_back((now, priority));
    }

    /// How long backfill should hold off `method` because everyone else has been using it: `None`
    /// when it need not.
    pub fn crawl_wait(&mut self, method: &str, now: Instant) -> Option<Duration> {
        self.above(method, Priority::Crawl, CRAWL_YIELD_AT, now)
            .1
            .filter(|wait| !wait.is_zero())
    }
}

/// Slack calls since the last report, by method and by what each was for: the one INFO line a
/// minute that says where the allowance went (`consumer="slack_calls"`). `#614
/// timeline-read-failures`: the figures had per-reason timeline reads but not what each spent of
/// each Slack method, so what kept `conversations.history` spent could not be read from the log.
///
/// Reported by the first call a minute or more after the last report, so a quiet minute writes no
/// line, and the line says how long it covers.
#[derive(Debug, Default)]
pub struct Tally {
    since: Option<Instant>,
    /// Requests sent, by method and read reason.
    sent: BTreeMap<(&'static str, ReadReason), u32>,
    /// Answers that were a rate limit, by method.
    limited: BTreeMap<&'static str, u32>,
    /// Requests failed as a rate limit without being sent, by method: a `Retry-After` in force, or
    /// crawl work yielding.
    refused: BTreeMap<&'static str, u32>,
    /// Of those, the ones paused because the provider was limiting ([`while_limiting`]).
    paused: BTreeMap<&'static str, u32>,
    /// Requests sent at [`Priority::Write`].
    writes: u32,
    /// Rate-limited answers to them.
    limited_writes: u32,
}

/// One method's calls in a report: how many, and how many for each read reason.
type MethodCalls = (u32, Vec<(ReadReason, u32)>);

/// What one [`Tally`] report says.
#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    /// Seconds covered.
    pub window_s: u64,
    /// Requests sent.
    pub sent: u32,
    /// Answers that were a rate limit.
    pub limited: u32,
    /// Requests refused without being sent.
    pub refused: u32,
    /// Requests sent at [`Priority::Write`]: posts, read marks, and the reads a post makes first.
    pub writes: u32,
    /// Requests sent for anything else.
    pub reads: u32,
    /// Rate-limited answers to writes.
    pub limited_writes: u32,
    /// Refusals that were reads paused while the provider was limiting.
    pub paused: u32,
    /// `method=n` for each method sent, most first.
    pub by_method: String,
    /// `method:reason=n,...` for each method, most first within it.
    pub by_caller: String,
    /// `method=n` for rate-limited answers.
    pub limited_by_method: String,
    /// `method=n` for refusals.
    pub refused_by_method: String,
    /// `method=n` for paused reads.
    pub paused_by_method: String,
}

impl Tally {
    /// Count a request of `method` for `reason` at `priority`, sent now.
    pub fn sent(
        &mut self,
        method: &'static str,
        reason: ReadReason,
        priority: Priority,
        now: Instant,
    ) {
        self.since.get_or_insert(now);
        *self.sent.entry((method, reason)).or_default() += 1;
        if priority == Priority::Write {
            self.writes += 1;
        }
    }

    /// Count an answer of `method` at `priority` that was a rate limit.
    pub fn limited(&mut self, method: &'static str, priority: Priority, now: Instant) {
        self.since.get_or_insert(now);
        *self.limited.entry(method).or_default() += 1;
        if priority == Priority::Write {
            self.limited_writes += 1;
        }
    }

    /// Count a read of `method` paused, without being sent, because the provider was limiting.
    /// Counted as refused too.
    pub fn paused(&mut self, method: &'static str, now: Instant) {
        self.since.get_or_insert(now);
        *self.paused.entry(method).or_default() += 1;
    }

    /// Count a request of `method` failed as a rate limit without being sent.
    pub fn refused(&mut self, method: &'static str, now: Instant) {
        self.since.get_or_insert(now);
        *self.refused.entry(method).or_default() += 1;
    }

    /// The report due at `now`, once [`WINDOW`] has passed since the first count after the last
    /// one, which it clears: `None` before then.
    pub fn due(&mut self, now: Instant) -> Option<Report> {
        let since = self.since?;
        let covered = now.saturating_duration_since(since);
        if covered < WINDOW {
            return None;
        }
        let taken = std::mem::take(self);
        let mut methods: BTreeMap<&'static str, MethodCalls> = BTreeMap::new();
        for ((method, reason), n) in &taken.sent {
            let entry = methods.entry(method).or_default();
            entry.0 += n;
            entry.1.push((*reason, *n));
        }
        let mut methods: Vec<(&'static str, MethodCalls)> = methods.into_iter().collect();
        methods.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
        let by_method = methods
            .iter()
            .map(|(method, (n, _))| format!("{method}={n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let by_caller = methods
            .iter_mut()
            .map(|(method, (_, reasons))| {
                reasons.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                let reasons = reasons
                    .iter()
                    .map(|(reason, n)| format!("{}={n}", reason.as_str()))
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{method}:{reasons}")
            })
            .collect::<Vec<_>>()
            .join("; ");
        let listed = |counts: &BTreeMap<&'static str, u32>| {
            counts
                .iter()
                .map(|(method, n)| format!("{method}={n}"))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let sent: u32 = taken.sent.values().sum();
        Some(Report {
            window_s: covered.as_secs(),
            sent,
            limited: taken.limited.values().sum(),
            refused: taken.refused.values().sum(),
            writes: taken.writes,
            reads: sent.saturating_sub(taken.writes),
            limited_writes: taken.limited_writes,
            paused: taken.paused.values().sum(),
            by_method,
            by_caller,
            limited_by_method: listed(&taken.limited),
            refused_by_method: listed(&taken.refused),
            paused_by_method: listed(&taken.paused),
        })
    }
}

impl Report {
    /// Write the report as its one INFO line.
    pub fn log(&self) {
        tracing::info!(
            consumer = "slack_calls",
            window_s = self.window_s,
            sent = self.sent,
            limited = self.limited,
            refused = self.refused,
            writes = self.writes,
            reads = self.reads,
            limited_writes = self.limited_writes,
            paused = self.paused,
            by_method = %self.by_method,
            by_caller = %self.by_caller,
            limited_by_method = %self.limited_by_method,
            refused_by_method = %self.refused_by_method,
            paused_by_method = %self.paused_by_method,
            "Slack calls since the last report, by method and by what each was for"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_person_s_read_comes_first_then_live_work_then_naming_then_backfill() {
        assert_eq!(Priority::of(ReadReason::Side), Priority::Interactive);
        assert_eq!(Priority::of(ReadReason::Counts), Priority::Interactive);
        // An untagged request may be a person's send.
        assert_eq!(Priority::of(ReadReason::Untagged), Priority::Interactive);
        assert_eq!(Priority::of(ReadReason::Relist), Priority::Live);
        assert_eq!(Priority::of(ReadReason::LivePoll), Priority::Live);
        assert_eq!(Priority::of(ReadReason::Naming), Priority::Background);
        assert_eq!(Priority::of(ReadReason::Backfill), Priority::Crawl);
        assert_eq!(Priority::of(ReadReason::Shadow), Priority::Crawl);
        assert!(Priority::Interactive > Priority::Live);
        assert!(Priority::Live > Priority::Background);
        assert!(Priority::Background > Priority::Crawl);
        // `#699 vt-send-priority`: a post and the reads it makes first come before a person's read.
        assert_eq!(Priority::of(ReadReason::Write), Priority::Write);
        assert!(Priority::Write > Priority::Interactive);
        assert!(is_write("chat.postMessage") && is_write("conversations.mark"));
        assert!(!is_write("conversations.replies") && !is_write("conversations.history"));
    }

    #[test]
    fn while_limiting_a_write_goes_and_everything_else_gives_way() {
        use WhileLimiting::{Go, HoldForWrites, Pause};
        for writes in [false, true] {
            assert_eq!(while_limiting(Priority::Write, false, writes), Go);
            for priority in [Priority::Background, Priority::Crawl] {
                assert_eq!(while_limiting(priority, false, writes), Pause);
            }
            // A read the message cache would answer is not sent at all.
            assert_eq!(while_limiting(Priority::Interactive, true, writes), Pause);
        }
        // A person's read and live work go, but not beside a send.
        assert_eq!(while_limiting(Priority::Interactive, false, false), Go);
        assert_eq!(
            while_limiting(Priority::Interactive, false, true),
            HoldForWrites
        );
        assert_eq!(while_limiting(Priority::Live, false, false), Go);
        assert_eq!(while_limiting(Priority::Live, false, true), Pause);
        // Live work, naming and crawl work make one try; a person's read waits as before.
        assert!(one_try_while_limiting(Priority::Live));
        assert!(one_try_while_limiting(Priority::Crawl));
        assert!(!one_try_while_limiting(Priority::Interactive));
        // Only the reads that drain the bucket give way; lookups never do.
        assert!(gives_way("conversations.replies") && gives_way("conversations.history"));
        assert!(!gives_way("users.info") && !gives_way("conversations.info"));
    }

    #[test]
    fn a_refused_write_tries_again_soon_and_never_later_than_it_was_told() {
        let told = Duration::from_secs(10);
        let waits: Vec<Duration> = (1..=7).map(|n| write_retry_wait(n, told)).collect();
        assert_eq!(
            waits,
            [250, 500, 1000, 1000, 2000, 2000, 2000].map(Duration::from_millis)
        );
        assert_eq!(
            write_retry_wait(5, Duration::from_millis(300)),
            Duration::from_millis(300)
        );
        // Twelve tries fit well inside the thirty seconds a read may wait.
        let total: Duration = (1..MAX_WRITE_ATTEMPTS)
            .map(|n| write_retry_wait(n, told))
            .sum();
        assert!(total < Duration::from_secs(30), "{total:?}");
    }

    #[test]
    fn a_ceiling_keeps_room_for_a_person_and_live_work_over_naming_and_crawl() {
        const METHOD: &str = "conversations.history";
        let now = Instant::now();
        let mut sent = Sent::default();
        for _ in 0..10 {
            sent.record(METHOD, Priority::Interactive, now);
        }
        // Half of twenty sent: crawl work waits, everyone else goes.
        assert!(sent
            .ceiling_wait(METHOD, Priority::Crawl, 20, now)
            .is_some_and(|wait| wait <= WINDOW));
        for priority in [Priority::Background, Priority::Live, Priority::Interactive] {
            assert_eq!(
                sent.ceiling_wait(METHOD, priority, 20, now),
                None,
                "{priority:?}"
            );
        }
        for _ in 0..5 {
            sent.record(METHOD, Priority::Live, now);
        }
        // Fifteen: naming waits too.
        assert!(sent
            .ceiling_wait(METHOD, Priority::Background, 20, now)
            .is_some());
        assert_eq!(sent.ceiling_wait(METHOD, Priority::Live, 20, now), None);
        let later = now + Duration::from_secs(10);
        for _ in 0..5 {
            sent.record(METHOD, Priority::Interactive, later);
        }
        // Twenty: a person waits for the oldest to leave the minute, fifty seconds on.
        assert_eq!(
            sent.ceiling_wait(METHOD, Priority::Interactive, 20, later),
            Some(Duration::from_secs(50))
        );
        assert_eq!(
            sent.ceiling_wait(METHOD, Priority::Interactive, 20, now + WINDOW),
            None
        );
    }

    #[test]
    fn the_minute_s_report_counts_calls_by_method_and_reason_and_clears() {
        let now = Instant::now();
        let mut tally = Tally::default();
        assert_eq!(tally.due(now), None, "nothing counted, nothing to say");
        for _ in 0..3 {
            tally.sent(
                "conversations.history",
                ReadReason::Side,
                Priority::Interactive,
                now,
            );
        }
        tally.sent(
            "conversations.history",
            ReadReason::LivePoll,
            Priority::Live,
            now,
        );
        tally.sent(
            "conversations.replies",
            ReadReason::Naming,
            Priority::Background,
            now,
        );
        tally.sent(
            "chat.postMessage",
            ReadReason::Untagged,
            Priority::Write,
            now,
        );
        tally.limited("conversations.history", Priority::Live, now);
        tally.limited("chat.postMessage", Priority::Write, now);
        tally.refused("conversations.history", now);
        tally.refused("conversations.replies", now);
        tally.paused("conversations.replies", now);
        assert_eq!(tally.due(now + Duration::from_secs(59)), None);
        let report = tally.due(now + WINDOW).expect("a minute on, a report");
        assert_eq!(
            report,
            Report {
                window_s: 60,
                sent: 6,
                limited: 2,
                refused: 2,
                writes: 1,
                reads: 5,
                limited_writes: 1,
                paused: 1,
                by_method: "conversations.history=4 chat.postMessage=1 conversations.replies=1"
                    .to_owned(),
                by_caller: "conversations.history:side=3,live-poll=1; \
                            chat.postMessage:untagged=1; conversations.replies:naming=1"
                    .to_owned(),
                limited_by_method: "chat.postMessage=1 conversations.history=1".to_owned(),
                refused_by_method: "conversations.history=1 conversations.replies=1".to_owned(),
                paused_by_method: "conversations.replies=1".to_owned(),
            }
        );
        assert_eq!(tally.due(now + 2 * WINDOW), None, "cleared by the report");
    }

    #[test]
    fn backfill_yields_to_everyone_else_but_never_to_itself_and_nothing_else_is_refused() {
        const METHOD: &str = "conversations.replies";
        let now = Instant::now();
        let mut sent = Sent::default();
        // Backfill's own requests never hold it: its own pace bounds them.
        for _ in 0..100 {
            sent.record(METHOD, Priority::Crawl, now);
        }
        assert_eq!(sent.crawl_wait(METHOD, now), None);
        assert!(sent.admits(METHOD, Priority::Crawl, now));
        // Everyone else's do.
        for _ in 0..CRAWL_YIELD_AT {
            sent.record(METHOD, Priority::Background, now);
        }
        let wait = sent.crawl_wait(METHOD, now).expect("backfill holds off");
        assert!(wait <= WINDOW && wait > Duration::ZERO, "{wait:?}");
        assert!(!sent.admits(METHOD, Priority::Crawl, now));
        for priority in [Priority::Background, Priority::Live, Priority::Interactive] {
            assert!(sent.admits(METHOD, priority, now), "{priority:?}");
        }
        // Another method's allowance is its own.
        assert_eq!(sent.crawl_wait("conversations.history", now), None);
        // A minute later every count has gone.
        let later = now + WINDOW;
        assert_eq!(sent.crawl_wait(METHOD, later), None);
        assert!(sent.admits(METHOD, Priority::Crawl, later));
    }
}
