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
//! * **interactive** — the page's reads, a send, speech, a voice agent's tools, and anything that
//!   says nothing of what it is for (an untagged request may be a person's);
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
}

impl Priority {
    /// The priority of a read made for `reason`.
    #[must_use]
    pub const fn of(reason: ReadReason) -> Self {
        match reason {
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
    /// `method=n` for each method sent, most first.
    pub by_method: String,
    /// `method:reason=n,...` for each method, most first within it.
    pub by_caller: String,
    /// `method=n` for rate-limited answers.
    pub limited_by_method: String,
    /// `method=n` for refusals.
    pub refused_by_method: String,
}

impl Tally {
    /// Count a request of `method` for `reason`, sent now.
    pub fn sent(&mut self, method: &'static str, reason: ReadReason, now: Instant) {
        self.since.get_or_insert(now);
        *self.sent.entry((method, reason)).or_default() += 1;
    }

    /// Count an answer of `method` that was a rate limit.
    pub fn limited(&mut self, method: &'static str, now: Instant) {
        self.since.get_or_insert(now);
        *self.limited.entry(method).or_default() += 1;
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
        Some(Report {
            window_s: covered.as_secs(),
            sent: taken.sent.values().sum(),
            limited: taken.limited.values().sum(),
            refused: taken.refused.values().sum(),
            by_method,
            by_caller,
            limited_by_method: listed(&taken.limited),
            refused_by_method: listed(&taken.refused),
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
            by_method = %self.by_method,
            by_caller = %self.by_caller,
            limited_by_method = %self.limited_by_method,
            refused_by_method = %self.refused_by_method,
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
    }

    #[test]
    fn the_minute_s_report_counts_calls_by_method_and_reason_and_clears() {
        let now = Instant::now();
        let mut tally = Tally::default();
        assert_eq!(tally.due(now), None, "nothing counted, nothing to say");
        for _ in 0..3 {
            tally.sent("conversations.history", ReadReason::Side, now);
        }
        tally.sent("conversations.history", ReadReason::LivePoll, now);
        tally.sent("conversations.replies", ReadReason::Naming, now);
        tally.limited("conversations.history", now);
        tally.refused("conversations.history", now);
        assert_eq!(tally.due(now + Duration::from_secs(59)), None);
        let report = tally.due(now + WINDOW).expect("a minute on, a report");
        assert_eq!(
            report,
            Report {
                window_s: 60,
                sent: 5,
                limited: 1,
                refused: 1,
                by_method: "conversations.history=4 conversations.replies=1".to_owned(),
                by_caller:
                    "conversations.history:side=3,live-poll=1; conversations.replies:naming=1"
                        .to_owned(),
                limited_by_method: "conversations.history=1".to_owned(),
                refused_by_method: "conversations.history=1".to_owned(),
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
