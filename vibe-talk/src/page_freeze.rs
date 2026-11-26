//! The page stopped answering touch, as bounded log lines.
//!
//! `#2066 vibe-talk-page-freeze`. The phone page locks up — swipes and presses do nothing for
//! a while, then it answers again, laggy at first — while this server stays healthy, so the
//! freeze is the page's main thread (or a press waiting on one). Nothing on the page could say
//! when it happened, for how long, or what the page was doing. The page now watches itself:
//! long tasks and long animation frames over a threshold, heartbeat gaps while visible, and
//! every hide/show with the backlog measured at the show. It keeps a bounded ring on the
//! device and posts it here (`POST /api/v1/freeze-report`) on recovery, on coming back into
//! sight, or on the next load — a frozen page cannot post, so the record is always made after
//! the thaw and says so (`t` is when the gap was detected, `dur_ms` how long it lasted).
//!
//! **Closed like the voice records** ([`crate::voice_health`]) and the scroll trace
//! ([`crate::scroll_trace`]): [`FreezeReport`] is a fixed set of fields — closed enums,
//! integers, and one capped free-text field, an event's `script`, which is the page code's
//! own invoker words from a long-animation-frame entry, never a message's. No ids of any
//! kind: freeze diagnosis needs what mode the page was in, not which channel it showed. An
//! unknown field, kind or value is refused rather than echoed.
//!
//! **Bounded.** [`MAX_BODY_BYTES`] caps what the route reads, [`MAX_EVENTS`] caps one
//! report, and the shared [`crate::voice_health::LogBudget`] caps the reports logged per
//! minute across every caller, one spend per report however many lines its events make.
//!
//! **Counted per day.** [`DayCounts`] totals each UTC day's reports and event kinds in
//! memory (a restart starts them at zero, as the summarizer's counters do), and every
//! report's header line carries the day's running totals — so the log itself answers "how
//! many freezes today" without storing anything.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The largest request body the route reads: [`MAX_EVENTS`] at every cap fits inside it, with
/// the client debug mode's detail on [`crate::client_debug::MAX_DETAILED_EVENTS`] of them and
/// its summary (`#2276 vibe-talk-client-debug-mode`).
pub const MAX_BODY_BYTES: usize = 160 * 1_024;

/// The most events one report carries: the page's ring holds no more.
pub const MAX_EVENTS: usize = 40;

/// The longest script an event carries, in characters: one long-animation-frame invoker.
pub const MAX_SCRIPT_CHARS: usize = 256;

/// The longest clock reading accepted, in milliseconds: a week of `performance.now`.
pub const MAX_TIME_MS: f64 = 604_800_000.0;

/// The longest duration or age accepted, in milliseconds: a day. Past that the number is
/// not a measurement of one freeze.
pub const MAX_DUR_MS: u64 = 86_400_000;

/// The largest queue count accepted: far past any backlog a page holds.
pub const MAX_QUEUE: u64 = 1_000_000;

/// How many days of totals [`DayCounts`] keeps: a week, pruned oldest first.
pub const MAX_DAYS: usize = 7;

/// What one event records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeKind {
    /// A `longtask` performance entry at or over the page's threshold.
    Longtask,
    /// A `long-animation-frame` entry at or over the threshold, with its invoker.
    Loaf,
    /// The heartbeat missed while visible: the main thread ran nothing for `dur_ms`.
    Freeze,
    /// The page went out of sight (`visibilitychange` to hidden).
    Hidden,
    /// The page came back into sight: `dur_ms` hidden, queue fields measured at the show.
    Visible,
}

impl FreezeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Longtask => "longtask",
            Self::Loaf => "loaf",
            Self::Freeze => "freeze",
            Self::Hidden => "hidden",
            Self::Visible => "visible",
        }
    }
}

/// Which top-level view the page showed (`Screens.view`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeView {
    /// The channel page.
    Discord,
    /// The voice call page.
    Voice,
    /// The documents reader.
    Docs,
    /// A view this record does not name.
    Other,
}

impl FreezeView {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::Voice => "voice",
            Self::Docs => "docs",
            Self::Other => "other",
        }
    }
}

/// Which screen stood over the view (`Screens.screen`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeScreen {
    /// Signing in.
    Signin,
    /// The main screen.
    Main,
    /// Settings.
    Settings,
    /// Help.
    Help,
    /// The welcome screen.
    Welcome,
    /// A screen this record does not name.
    Other,
}

impl FreezeScreen {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Signin => "signin",
            Self::Main => "main",
            Self::Settings => "settings",
            Self::Help => "help",
            Self::Welcome => "welcome",
            Self::Other => "other",
        }
    }
}

/// Which channel view was selected (the page's selection view).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeChannelView {
    /// Top-level messages.
    Main,
    /// Everything in time order.
    Flat,
    /// One thread.
    Thread,
    /// The thread list.
    Threads,
    /// No channel view stood (another top-level view).
    None,
    /// A channel view this record does not name.
    Other,
}

impl FreezeChannelView {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Flat => "flat",
            Self::Thread => "thread",
            Self::Threads => "threads",
            Self::None => "none",
            Self::Other => "other",
        }
    }
}

/// One column or two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeLayout {
    /// One column.
    Single,
    /// Two columns side by side.
    TwoColumn,
}

impl FreezeLayout {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::TwoColumn => "two_column",
        }
    }
}

/// What read-aloud was doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeReading {
    /// Nothing being read.
    Idle,
    /// Reading aloud.
    Playing,
    /// A read held paused.
    Paused,
    /// A read was asked for and its speech was still being prepared: the await-idle shape,
    /// told apart from a busy main thread.
    Pending,
    /// A read-aloud state this record does not name.
    Other,
}

impl FreezeReading {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Playing => "playing",
            Self::Paused => "paused",
            Self::Pending => "pending",
            Self::Other => "other",
        }
    }
}

/// Whether the page was in sight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeVisibility {
    /// In sight.
    Visible,
    /// Out of sight.
    Hidden,
}

impl FreezeVisibility {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
        }
    }
}

/// The reader's last act before the event, by input kind only — never what it pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FreezeAction {
    /// No act yet on this page.
    None,
    /// A pointer press.
    Pointer,
    /// A key press.
    Key,
    /// A wheel turn.
    Wheel,
    /// A touch.
    Touch,
    /// A scroll.
    Scroll,
}

impl FreezeAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Pointer => "pointer",
            Self::Key => "key",
            Self::Wheel => "wheel",
            Self::Touch => "touch",
            Self::Scroll => "scroll",
        }
    }
}

/// One stall, hide or show, as the page saw it. The field set IS the allowlist.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreezeEvent {
    /// When it was detected, `performance.now` in milliseconds.
    pub t: f64,
    /// What kind of event it was.
    pub kind: FreezeKind,
    /// How long it lasted: a task's duration, a heartbeat gap, or a hidden stretch.
    pub dur_ms: u64,
    /// The top-level view.
    pub view: FreezeView,
    /// The screen over it.
    pub screen: FreezeScreen,
    /// The channel view selected.
    pub channel_view: FreezeChannelView,
    /// One column or two.
    pub layout: FreezeLayout,
    /// What read-aloud was doing.
    pub reading: FreezeReading,
    /// Whether the lists were windowed (only a slice of rows mounted).
    pub windowed: bool,
    /// Whether the page was in sight.
    pub visibility: FreezeVisibility,
    /// The reader's last act, by kind.
    pub action: FreezeAction,
    /// Milliseconds from the reader's last act to this event, where there was one.
    #[serde(default)]
    pub action_age_ms: Option<u64>,
    /// Milliseconds from the last coming-back-into-sight to this event, where the page had
    /// been hidden: small values are the first stalls after a resume.
    #[serde(default)]
    pub resume_age_ms: Option<u64>,
    /// A long animation frame's invoker words (the page code's own); empty otherwise.
    #[serde(default)]
    pub script: String,
    /// Live replays still waiting to be drawn at a show (`visible` only).
    #[serde(default)]
    pub replays_waiting: Option<u64>,
    /// Inputs heard while hidden, counted at the show (`visible` only).
    #[serde(default)]
    pub inputs_hidden: Option<u64>,
    /// How late a zero timer set at the show ran: the timer backlog (`visible` only).
    #[serde(default)]
    pub timer_delay_ms: Option<u64>,
    /// How late the first animation frame after the show came (`visible` only).
    #[serde(default)]
    pub raf_delay_ms: Option<u64>,
    /// What code ran in the stall, where the owner's client debug mode is on (`#2276`):
    /// named spans, long-frame scripts and the sampler's top frames; timings and code names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<crate::client_debug::EventDebug>,
}

/// One flush of the page's ring: `POST /api/v1/freeze-report`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FreezeReport {
    /// The report's version. Only `1` is defined.
    pub v: u32,
    /// When this page loaded, `performance.now` in milliseconds — the events' own clock.
    #[serde(rename = "startedAt")]
    pub started_at: f64,
    /// The events, oldest first. At most [`MAX_EVENTS`].
    pub events: Vec<FreezeEvent>,
    /// The client debug mode's totals since its last report, where it is on (`#2276`). A
    /// report may carry this alone, with no events: the mode's once-a-minute summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<crate::client_debug::DebugSummary>,
}

fn check_time(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=MAX_TIME_MS).contains(&value) {
        return Err(format!(
            "{name} is not a clock reading this page could make"
        ));
    }
    Ok(())
}

fn check_ms(name: &str, value: Option<u64>, max: u64) -> Result<(), String> {
    if let Some(ms) = value {
        if ms > max {
            return Err(format!("{name} is larger than {max}"));
        }
    }
    Ok(())
}

impl FreezeReport {
    /// Refuse anything that is not a plausible report.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self) -> Result<(), String> {
        if self.v != 1 {
            return Err(format!("v is {}, not the one version defined", self.v));
        }
        check_time("startedAt", self.started_at)?;
        if self.events.len() > MAX_EVENTS {
            return Err(format!("events is more than {MAX_EVENTS}"));
        }
        for (at, event) in self.events.iter().enumerate() {
            event.validate(at)?;
        }
        let detailed = self
            .events
            .iter()
            .filter(|event| event.debug.is_some())
            .count();
        if detailed > crate::client_debug::MAX_DETAILED_EVENTS {
            return Err(format!(
                "more than {} events carry debug detail",
                crate::client_debug::MAX_DETAILED_EVENTS
            ));
        }
        if let Some(summary) = &self.debug {
            summary.validate()?;
        }
        Ok(())
    }

    /// The line that opens a report in the log: its size and the day's running totals.
    #[must_use]
    pub fn header_fields(&self, day: &str, totals: &Counts) -> String {
        format!(
            "v={} started_at={} events={} day={} day_reports={} day_freezes={} \
             day_longtasks={} day_loafs={} day_resumes={}",
            self.v,
            self.started_at,
            self.events.len(),
            day,
            totals.reports,
            totals.freezes,
            totals.longtasks,
            totals.loafs,
            totals.resumes,
        )
    }
}

impl FreezeEvent {
    fn validate(&self, at: usize) -> Result<(), String> {
        let name = |field: &str| format!("events[{at}].{field}");
        check_time(&name("t"), self.t)?;
        check_ms(&name("dur_ms"), Some(self.dur_ms), MAX_DUR_MS)?;
        check_ms(&name("action_age_ms"), self.action_age_ms, MAX_DUR_MS)?;
        check_ms(&name("resume_age_ms"), self.resume_age_ms, MAX_DUR_MS)?;
        check_ms(&name("timer_delay_ms"), self.timer_delay_ms, MAX_DUR_MS)?;
        check_ms(&name("raf_delay_ms"), self.raf_delay_ms, MAX_DUR_MS)?;
        check_ms(&name("replays_waiting"), self.replays_waiting, MAX_QUEUE)?;
        check_ms(&name("inputs_hidden"), self.inputs_hidden, MAX_QUEUE)?;
        if self.script.chars().count() > MAX_SCRIPT_CHARS {
            return Err(format!(
                "{} is longer than {MAX_SCRIPT_CHARS} characters",
                name("script")
            ));
        }
        if self.script.chars().any(char::is_control) {
            return Err(format!("{} carries a control character", name("script")));
        }
        if let Some(debug) = &self.debug {
            debug.validate(&format!("events[{at}]"))?;
        }
        Ok(())
    }

    /// The event's one log line: enums and numbers plain, the script quoted so it cannot
    /// pass for a field; `-` for what was not sent.
    #[must_use]
    pub fn log_fields(&self, at: usize) -> String {
        let number = |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |n| n.to_string());
        format!(
            "i={at} t={} kind={} dur_ms={} view={} screen={} channel_view={} layout={} \
             reading={} windowed={} visibility={} action={} action_age_ms={} \
             resume_age_ms={} replays_waiting={} inputs_hidden={} timer_delay_ms={} \
             raf_delay_ms={} script={:?}",
            self.t,
            self.kind.as_str(),
            self.dur_ms,
            self.view.as_str(),
            self.screen.as_str(),
            self.channel_view.as_str(),
            self.layout.as_str(),
            self.reading.as_str(),
            self.windowed,
            self.visibility.as_str(),
            self.action.as_str(),
            number(self.action_age_ms),
            number(self.resume_age_ms),
            number(self.replays_waiting),
            number(self.inputs_hidden),
            number(self.timer_delay_ms),
            number(self.raf_delay_ms),
            self.script,
        )
    }
}

/// One day's totals.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Reports received that day.
    pub reports: u64,
    /// Heartbeat freezes.
    pub freezes: u64,
    /// Long tasks.
    pub longtasks: u64,
    /// Long animation frames.
    pub loafs: u64,
    /// Comings-back-into-sight.
    pub resumes: u64,
}

/// Per-day freeze totals, in memory: a restart starts them at zero. Days are ISO dates
/// (`YYYY-MM-DD`), so the map's order is the calendar's; only the newest [`MAX_DAYS`] stay.
#[derive(Debug, Default)]
pub struct DayCounts {
    days: Mutex<BTreeMap<String, Counts>>,
}

impl DayCounts {
    /// No day counted yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Count one received report under `day` and answer the day's totals including it.
    pub fn note(&self, day: &str, report: &FreezeReport) -> Counts {
        let mut days = self
            .days
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let totals = days.entry(day.to_owned()).or_default();
        totals.reports = totals.reports.saturating_add(1);
        for event in &report.events {
            match event.kind {
                FreezeKind::Freeze => totals.freezes = totals.freezes.saturating_add(1),
                FreezeKind::Longtask => {
                    totals.longtasks = totals.longtasks.saturating_add(1);
                }
                FreezeKind::Loaf => totals.loafs = totals.loafs.saturating_add(1),
                FreezeKind::Visible => totals.resumes = totals.resumes.saturating_add(1),
                FreezeKind::Hidden => {}
            }
        }
        let totals = *totals;
        while days.len() > MAX_DAYS {
            let oldest = days.keys().next().cloned();
            match oldest {
                Some(oldest) => {
                    days.remove(&oldest);
                }
                None => break,
            }
        }
        totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: FreezeKind) -> FreezeEvent {
        FreezeEvent {
            t: 12_345.5,
            kind,
            dur_ms: 4_200,
            view: FreezeView::Discord,
            screen: FreezeScreen::Main,
            channel_view: FreezeChannelView::Thread,
            layout: FreezeLayout::TwoColumn,
            reading: FreezeReading::Pending,
            windowed: true,
            visibility: FreezeVisibility::Visible,
            action: FreezeAction::Pointer,
            action_age_ms: Some(300),
            resume_age_ms: Some(1_500),
            script: "handler at renderRows".to_owned(),
            replays_waiting: Some(7),
            inputs_hidden: Some(0),
            timer_delay_ms: Some(900),
            raf_delay_ms: Some(1_100),
            debug: None,
        }
    }

    fn report(events: Vec<FreezeEvent>) -> FreezeReport {
        FreezeReport {
            v: 1,
            started_at: 10_000.0,
            events,
            debug: None,
        }
    }

    #[test]
    fn a_report_reads_as_the_page_sends_it_and_logs_enums_and_numbers_only() {
        let wire = serde_json::json!({
            "v": 1, "startedAt": 10000.0,
            "events": [{
                "t": 12345.5, "kind": "freeze", "dur_ms": 4200,
                "view": "discord", "screen": "main", "channel_view": "thread",
                "layout": "two_column", "reading": "pending", "windowed": true,
                "visibility": "visible", "action": "pointer",
                "action_age_ms": 300, "resume_age_ms": 1500,
                "script": "handler at renderRows",
                "replays_waiting": 7, "inputs_hidden": 0,
                "timer_delay_ms": 900, "raf_delay_ms": 1100
            }],
        });
        let parsed: FreezeReport = serde_json::from_value(wire).expect("the page's shape reads");
        assert_eq!(parsed, report(vec![event(FreezeKind::Freeze)]));
        parsed.validate().expect("valid");
        let totals = Counts {
            reports: 2,
            freezes: 1,
            longtasks: 3,
            loafs: 0,
            resumes: 1,
        };
        assert_eq!(
            parsed.header_fields("2026-11-21", &totals),
            "v=1 started_at=10000 events=1 day=2026-11-21 day_reports=2 day_freezes=1 \
             day_longtasks=3 day_loafs=0 day_resumes=1"
        );
        assert_eq!(
            parsed.events[0].log_fields(0),
            "i=0 t=12345.5 kind=freeze dur_ms=4200 view=discord screen=main \
             channel_view=thread layout=two_column reading=pending windowed=true \
             visibility=visible action=pointer action_age_ms=300 resume_age_ms=1500 \
             replays_waiting=7 inputs_hidden=0 timer_delay_ms=900 raf_delay_ms=1100 \
             script=\"handler at renderRows\""
        );
        // Absent optionals read as absent and log as `-`.
        let bare = serde_json::json!({
            "v": 1, "startedAt": 0.0,
            "events": [{
                "t": 1.0, "kind": "hidden", "dur_ms": 0,
                "view": "voice", "screen": "main", "channel_view": "none",
                "layout": "single", "reading": "idle", "windowed": false,
                "visibility": "hidden", "action": "none"
            }],
        });
        let bare: FreezeReport = serde_json::from_value(bare).expect("defaults read");
        bare.validate().expect("valid");
        assert!(bare.events[0].log_fields(0).contains("resume_age_ms=-"));
        assert!(bare.events[0].log_fields(0).ends_with("script=\"\""));
        report(Vec::new())
            .validate()
            .expect("an empty flush is valid");
    }

    #[test]
    fn a_report_refuses_unknown_fields_kinds_ids_and_another_version() {
        for smuggle in ["channel_id", "text", "message", "device"] {
            let mut wire =
                serde_json::to_value(report(vec![event(FreezeKind::Freeze)])).expect("serializes");
            wire["events"][0][smuggle] = "the words on screen".into();
            assert!(
                serde_json::from_value::<FreezeReport>(wire).is_err(),
                "a free field rode in: {smuggle}"
            );
        }
        let mut unknown =
            serde_json::to_value(report(vec![event(FreezeKind::Freeze)])).expect("serializes");
        unknown["events"][0]["kind"] = "teleport".into();
        assert!(serde_json::from_value::<FreezeReport>(unknown).is_err());
        let mut unknown =
            serde_json::to_value(report(vec![event(FreezeKind::Freeze)])).expect("serializes");
        unknown["events"][0]["reading"] = "shouting".into();
        assert!(serde_json::from_value::<FreezeReport>(unknown).is_err());
        assert!(FreezeReport {
            v: 2,
            ..report(Vec::new())
        }
        .validate()
        .is_err());
    }

    #[test]
    fn a_report_refuses_oversize_counts_scripts_and_impossible_numbers() {
        report(vec![event(FreezeKind::Freeze); MAX_EVENTS])
            .validate()
            .expect("a full ring is valid");
        assert!(report(vec![event(FreezeKind::Freeze); MAX_EVENTS + 1])
            .validate()
            .is_err());
        for absurd in [
            FreezeEvent {
                script: "x".repeat(MAX_SCRIPT_CHARS + 1),
                ..event(FreezeKind::Loaf)
            },
            FreezeEvent {
                script: "one\ntwo".to_owned(),
                ..event(FreezeKind::Loaf)
            },
            FreezeEvent {
                t: f64::INFINITY,
                ..event(FreezeKind::Freeze)
            },
            FreezeEvent {
                t: -1.0,
                ..event(FreezeKind::Freeze)
            },
            FreezeEvent {
                dur_ms: MAX_DUR_MS + 1,
                ..event(FreezeKind::Freeze)
            },
            FreezeEvent {
                resume_age_ms: Some(MAX_DUR_MS + 1),
                ..event(FreezeKind::Longtask)
            },
            FreezeEvent {
                replays_waiting: Some(MAX_QUEUE + 1),
                ..event(FreezeKind::Visible)
            },
        ] {
            assert!(report(vec![absurd]).validate().is_err());
        }
        // A forged script cannot look like more fields inside its quotes.
        let forged = FreezeEvent {
            script: "x\" kind=freeze".to_owned(),
            ..event(FreezeKind::Loaf)
        };
        assert!(forged
            .log_fields(0)
            .ends_with(r#"script="x\" kind=freeze""#));
    }

    #[test]
    fn debug_detail_reads_is_capped_per_report_and_a_full_report_fits_the_body_cap() {
        use crate::client_debug::fixtures::{full_event_debug, full_summary};
        use crate::client_debug::MAX_DETAILED_EVENTS;
        // Every string at its cap in the widest characters, every number at its cap.
        let widest = FreezeEvent {
            t: MAX_TIME_MS,
            dur_ms: MAX_DUR_MS,
            action_age_ms: Some(MAX_DUR_MS),
            resume_age_ms: Some(MAX_DUR_MS),
            timer_delay_ms: Some(MAX_DUR_MS),
            raf_delay_ms: Some(MAX_DUR_MS),
            replays_waiting: Some(MAX_QUEUE),
            inputs_hidden: Some(MAX_QUEUE),
            script: "\u{10348}".repeat(MAX_SCRIPT_CHARS),
            channel_view: FreezeChannelView::Threads,
            ..event(FreezeKind::Longtask)
        };
        let mut events = vec![widest; MAX_EVENTS];
        for one in events.iter_mut().take(MAX_DETAILED_EVENTS) {
            one.debug = Some(full_event_debug());
        }
        let full = FreezeReport {
            started_at: MAX_TIME_MS,
            debug: Some(full_summary()),
            ..report(events)
        };
        full.validate().expect("a report at every cap is valid");
        let bytes = serde_json::to_vec(&full).expect("serializes").len();
        assert!(
            bytes <= MAX_BODY_BYTES,
            "{bytes} bytes overflow the route's cap"
        );
        // One detailed event too many is refused.
        let mut over = full.clone();
        over.events[MAX_DETAILED_EVENTS].debug = Some(full_event_debug());
        assert!(over.validate().is_err());
        // A summary alone, with no events, is the debug mode's minute report.
        let summary_only = FreezeReport {
            debug: Some(full_summary()),
            ..report(Vec::new())
        };
        summary_only.validate().expect("valid");
        // A bad attribution names its event.
        let mut bad = report(vec![event(FreezeKind::Loaf)]);
        let mut debug = full_event_debug();
        debug.spans[0].name = "token=secret".to_owned();
        bad.events[0].debug = Some(debug);
        let refused = bad.validate().expect_err("refused");
        assert!(
            refused.starts_with("events[0].debug.spans[0].name"),
            "{refused}"
        );
        // The page's exact wire shape reads, and a report without debug serializes as before.
        let wire = serde_json::json!({
            "v": 1, "startedAt": 0.0,
            "events": [{
                "t": 1.0, "kind": "longtask", "dur_ms": 250,
                "view": "discord", "screen": "main", "channel_view": "flat",
                "layout": "single", "reading": "idle", "windowed": false,
                "visibility": "visible", "action": "key", "action_age_ms": 4,
                "debug": {"spans": [{"name": "key", "n": 1, "ms": 240}]}
            }],
            "debug": {
                "caps": {"loaf": false, "longtask": true, "profiler": false, "policy": false},
                "since": 0, "frames_over_50": 0, "frames_over_100": 0, "max_frame_ms": 0
            }
        });
        let parsed: FreezeReport = serde_json::from_value(wire).expect("reads");
        parsed.validate().expect("valid");
        let plain =
            serde_json::to_value(report(vec![event(FreezeKind::Freeze)])).expect("serializes");
        assert!(plain.get("debug").is_none());
        assert!(plain["events"][0].get("debug").is_none());
    }

    #[test]
    fn day_counts_total_by_kind_and_keep_a_week() {
        let counts = DayCounts::new();
        let one = counts.note(
            "2026-11-21",
            &report(vec![
                event(FreezeKind::Freeze),
                event(FreezeKind::Longtask),
                event(FreezeKind::Hidden),
                event(FreezeKind::Visible),
            ]),
        );
        assert_eq!(
            one,
            Counts {
                reports: 1,
                freezes: 1,
                longtasks: 1,
                loafs: 0,
                resumes: 1,
            }
        );
        let two = counts.note("2026-11-21", &report(vec![event(FreezeKind::Loaf)]));
        assert_eq!(two.reports, 2);
        assert_eq!(two.loafs, 1);
        // Another day counts apart, and the eighth day prunes the first.
        for day in 22..=28 {
            counts.note(&format!("2026-11-{day}"), &report(Vec::new()));
        }
        let again = counts.note("2026-11-21", &report(Vec::new()));
        assert_eq!(again.reports, 1, "the pruned day starts over");
    }
}
