//! What held the page's main thread, as bounded log lines: the client debug mode's records.
//!
//! `#2276 vibe-talk-client-debug-mode`. The page's freeze reports ([`crate::page_freeze`]) say
//! WHEN the page stalled and what it was showing, but not what code ran: a heartbeat freeze and
//! a `longtask` entry carry no attribution, and a browser without long-animation-frame entries
//! leaves the one free-text field empty. With the owner's debug switch on, the page times its
//! own hot paths as named spans, keeps long-animation-frame script detail where the browser has
//! it, and samples its main thread with the JS Self-Profiling API where the browser allows it.
//! It sends what it found beside the records it already sends: an [`EventDebug`] on a freeze
//! report's events, and a [`DebugSummary`] on a freeze report or a scroll trace
//! ([`crate::scroll_trace`]).
//!
//! **Timings and code names only.** Every string here is the page's OWN code: a span's name, a
//! function name, a script's file name and position, a long frame's invoker. None is a message's
//! words, an id or a token. The page strips what could carry one (a URL's query and fragment, an
//! element id that is not one of the page's own), and this module refuses any string outside
//! [`is_code_name`]'s narrow alphabet: no `?`, `=`, `&`, `@`, quotes or separators, so a query
//! string, a token or an address cannot ride in by accident.
//!
//! **Closed and bounded like the records it rides with.** Fixed fields with
//! `deny_unknown_fields`, every list capped ([`MAX_EVENT_SPANS`], [`MAX_FRAME_SCRIPTS`],
//! [`MAX_PROFILE_FRAMES`], [`MAX_SUMMARY_SPANS`], [`MAX_SUMMARY_FRAMES`]), every string capped
//! at [`MAX_NAME_CHARS`], and every number range-checked. The routes' own budgets and body caps
//! still apply.

use serde::{Deserialize, Serialize};

/// The longest code name accepted, in characters: a function, span, file or invoker name.
pub const MAX_NAME_CHARS: usize = 96;

/// The most spans one event names: the ones that ran longest inside its window.
pub const MAX_EVENT_SPANS: usize = 6;

/// The most scripts one long frame names: its longest.
pub const MAX_FRAME_SCRIPTS: usize = 4;

/// The most open spans one event names.
pub const MAX_OPEN_SPANS: usize = 4;

/// The most recent span ends one event names.
pub const MAX_RECENT_SPANS: usize = 8;

/// The largest element count accepted: far past any page a browser lays out.
pub const MAX_NODES: u64 = 100_000_000;

/// The most frames each profile list names.
pub const MAX_PROFILE_FRAMES: usize = 6;

/// The most span names one summary totals: the ones with the most time.
pub const MAX_SUMMARY_SPANS: usize = 16;

/// The most long frames one summary lists: the slowest.
pub const MAX_SUMMARY_FRAMES: usize = 20;

/// The most events of one freeze report that carry an [`EventDebug`]: the page keeps detail on
/// its longest stalls only, so a full ring stays inside the route's body cap.
pub const MAX_DETAILED_EVENTS: usize = 10;

/// The longest duration accepted, in milliseconds: a day, as for the freeze events.
pub const MAX_DUR_MS: u64 = 86_400_000;

/// The largest count accepted: far past anything one page counts between two reports.
pub const MAX_COUNT: u64 = 1_000_000_000;

/// The longest clock reading accepted, in milliseconds: a week of `performance.now`.
pub const MAX_TIME_MS: f64 = 604_800_000.0;

/// The cookie the page sets while the debug switch is on, `vibe-talk-client-debug=1`: the one
/// thing the server is told, so it can serve the shell with the profiling document policy. The
/// page clears it when the switch goes off.
pub const COOKIE_NAME: &str = "vibe-talk-client-debug";

/// The one-load URL flag, `?client-debug=1`, the same switch for a single load.
pub const QUERY_FLAG: &str = "client-debug=1";

/// The document policy that lets the page start the JS Self-Profiling API's sampler.
pub const DOCUMENT_POLICY: &str = "js-profiling";

/// The browser's family and major version from a request's `User-Agent` (`chrome-131`,
/// `webview-131`, `edge-131`, `firefox-133`, `safari-18`, `other`): what the client debug mode's
/// log lines carry so a reader knows which browser's records they are, never the string itself.
/// The page never reads the user agent (a page ground rule); the server reads its own header.
#[must_use]
pub fn ua_class(ua: &str) -> String {
    let major = |marker: &str| -> Option<String> {
        let at = ua.find(marker)? + marker.len();
        let digits: String = ua[at..]
            .chars()
            .take_while(char::is_ascii_digit)
            .take(4)
            .collect();
        (!digits.is_empty()).then_some(digits)
    };
    if let Some(v) = major("Edg/")
        .or_else(|| major("EdgA/"))
        .or_else(|| major("EdgiOS/"))
    {
        return format!("edge-{v}");
    }
    if let Some(v) = major("Firefox/").or_else(|| major("FxiOS/")) {
        return format!("firefox-{v}");
    }
    if let Some(v) = major("Chrome/").or_else(|| major("CriOS/")) {
        let webview = ua.contains("; wv)");
        return format!("{}-{v}", if webview { "webview" } else { "chrome" });
    }
    if ua.contains("Safari/") {
        if let Some(v) = major("Version/") {
            return format!("safari-{v}");
        }
    }
    "other".to_owned()
}

/// Whether a shell request asks for the debug mode: the URL flag in its `query`, or the switch's
/// cookie among its `cookies` (each a `Cookie` header's value, `a=1; b=2`).
#[must_use]
pub fn asked_for<'a>(query: Option<&str>, cookies: impl IntoIterator<Item = &'a str>) -> bool {
    let flagged = query.is_some_and(|query| query.split('&').any(|pair| pair == QUERY_FLAG));
    flagged
        || cookies.into_iter().any(|header| {
            header.split(';').map(str::trim).any(|pair| {
                pair.split_once('=')
                    .is_some_and(|(name, value)| name == COOKIE_NAME && value == "1")
            })
        })
}

/// Whether `text` is a code name: in the code-name alphabet (ASCII letters and digits and
/// `_ $ . # : / < > ( ) [ ] -` and the space: nothing that separates fields in a log line,
/// nothing a query string, a token or an address needs), and with no word that looks like an
/// id ([`looks_like_id`]). A trailing `:line` or `:line:column` position is exempt from the id
/// rule, since a script position is a number.
#[must_use]
pub fn is_code_name(text: &str) -> bool {
    let alphabet = text.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(
                c,
                '_' | '$' | '.' | '#' | ':' | '/' | '<' | '>' | '(' | ')' | '[' | ']' | '-' | ' '
            )
    });
    alphabet
        && !without_position(text)
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(looks_like_id)
}

/// `text` without a trailing `:123` or `:123:45` (a script position).
fn without_position(text: &str) -> &str {
    let mut rest = text;
    for _ in 0..2 {
        match rest.rsplit_once(':') {
            Some((head, tail)) if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) => {
                rest = head;
            }
            _ => break,
        }
    }
    rest
}

/// Whether one word (letters and digits only) looks like an id rather than a code name: six or
/// more digits in a row (a message id, a timestamp, a snowflake), an upper-case word of eight or
/// more characters with a digit in it (a Slack-style channel or user id), or sixteen or more hex
/// digits in a row (a hash, a token). The page redacts the same words before it sends
/// (`web/src/view/shell/clientdebug.ts`), so a report is never refused whole for one name.
#[must_use]
pub fn looks_like_id(word: &str) -> bool {
    let mut digits = 0_usize;
    let mut hex = 0_usize;
    for b in word.bytes() {
        digits = if b.is_ascii_digit() { digits + 1 } else { 0 };
        hex = if b.is_ascii_hexdigit() { hex + 1 } else { 0 };
        if digits >= 6 || hex >= 16 {
            return true;
        }
    }
    word.len() >= 8
        && word
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && word.bytes().any(|b| b.is_ascii_digit())
}

fn check_name(name: &str, value: &str) -> Result<(), String> {
    if value.chars().count() > MAX_NAME_CHARS {
        return Err(format!("{name} is longer than {MAX_NAME_CHARS} characters"));
    }
    if !is_code_name(value) {
        return Err(format!("{name} is not a code name"));
    }
    Ok(())
}

fn check_ms(name: &str, value: u64) -> Result<(), String> {
    if value > MAX_DUR_MS {
        return Err(format!("{name} is larger than {MAX_DUR_MS}"));
    }
    Ok(())
}

fn check_count(name: &str, value: u64) -> Result<(), String> {
    if value > MAX_COUNT {
        return Err(format!("{name} is larger than {MAX_COUNT}"));
    }
    Ok(())
}

fn check_time(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=MAX_TIME_MS).contains(&value) {
        return Err(format!(
            "{name} is not a clock reading this page could make"
        ));
    }
    Ok(())
}

fn check_len<T>(name: &str, list: &[T], max: usize) -> Result<(), String> {
    if list.len() > max {
        return Err(format!("{name} has more than {max} items"));
    }
    Ok(())
}

/// What the page's browser offers the debug mode, so a reader of the log knows which records
/// to expect from this device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Caps {
    /// `long-animation-frame` entries are delivered.
    pub loaf: bool,
    /// `longtask` entries are delivered.
    pub longtask: bool,
    /// The JS Self-Profiling API exists in this browser.
    pub profiler: bool,
    /// A profiler could be started: the page was served with the profiling document policy.
    pub policy: bool,
    /// `event` (Event Timing) entries are delivered: input delay and handler time per event.
    #[serde(default)]
    pub event: bool,
    /// The profiler's sampling interval in this upload's period, in milliseconds; 0 where none ran.
    #[serde(default)]
    pub sample_ms: u64,
}

/// A long frame script's kind, as the browser names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InvokerType {
    /// A callback the page handed the browser (a timer, an animation frame).
    UserCallback,
    /// An event listener.
    EventListener,
    /// A promise's resolve handler.
    ResolvePromise,
    /// A promise's reject handler.
    RejectPromise,
    /// A classic script's top level.
    ClassicScript,
    /// A module script's top level.
    ModuleScript,
    /// A kind this record does not name.
    Other,
}

impl InvokerType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::UserCallback => "user-callback",
            Self::EventListener => "event-listener",
            Self::ResolvePromise => "resolve-promise",
            Self::RejectPromise => "reject-promise",
            Self::ClassicScript => "classic-script",
            Self::ModuleScript => "module-script",
            Self::Other => "other",
        }
    }
}

/// One named span's time inside an event's window: how many ran there, and for how long in all.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpanTime {
    /// The span's name, the page's own (`key`, `search-fast`, `rows`).
    pub name: String,
    /// How many of it overlapped the window.
    pub n: u64,
    /// Their time inside the window, in milliseconds (nested spans each count their own).
    pub ms: u64,
    /// Layout reads (`getBoundingClientRect`, `getClientRects`) they made, in all.
    #[serde(default)]
    pub reads: u64,
    /// The page's element count at the end of the longest of them, where it was 16 ms or more.
    #[serde(default)]
    pub nodes: Option<u64>,
    /// The channel's messages held (`S`, `canon.messages.length`) at the end of the longest of
    /// them, where it was 16 ms or more: what size the work ran at.
    #[serde(default)]
    pub s: Option<u64>,
    /// The list's rows mounted (`D`, `list.children.length`) at the same moment.
    #[serde(default)]
    pub d: Option<u64>,
    /// Where `s` was counted: the channel's store, or the message rows drawn.
    #[serde(default)]
    pub s_src: Option<SizeSource>,
    /// Characters of saved message cache they read or wrote, in all.
    #[serde(default)]
    pub chars: Option<u64>,
    /// Rows they dressed, in all.
    #[serde(default)]
    pub rows: Option<u64>,
}

/// Where a span's `S` was counted (`#2276` follow-up): a threaded provider's reads land in the
/// channel's store (`canon.messages.length`); an unthreaded channel's pages are drawn without it,
/// so there `S` is the message rows drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeSource {
    /// The channel's store (`canon.messages.length`).
    Canon,
    /// The message rows drawn in the list.
    Drawn,
}

impl SizeSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Canon => "canon",
            Self::Drawn => "drawn",
        }
    }
}

/// A span still running when a stall began: the likeliest code to have held the thread.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSpan {
    /// The span's name.
    pub name: String,
    /// How long before the stall's start it began, in milliseconds.
    pub age_ms: u64,
}

/// One of the last spans to end before a stall began.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpanEnd {
    /// The span's name.
    pub name: String,
    /// How long it ran, in milliseconds.
    pub dur_ms: u64,
    /// How long before the stall's start it ended, in milliseconds.
    pub ago_ms: u64,
}

/// One script of a long animation frame.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptTime {
    /// The function the browser names (`sourceFunctionName`); empty when it names none.
    #[serde(rename = "fn")]
    pub function: String,
    /// The script's file name and character position (`voice.<hash>.js:1234`), never a query.
    pub src: String,
    /// What kind of entry point ran it.
    pub invoker_type: InvokerType,
    /// The entry point, as the browser words it (`DIV#discord-log.onclick`), page ids only.
    pub invoker: String,
    /// How long it ran, in milliseconds.
    pub dur_ms: u64,
    /// How much of that was forced style and layout, in milliseconds.
    pub forced_layout_ms: u64,
}

/// A long animation frame's breakdown, where the browser has those entries.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameDetail {
    /// How long the frame blocked input, in milliseconds (`blockingDuration`).
    pub blocking_ms: u64,
    /// From its rendering's start to its end, in milliseconds, where it rendered.
    #[serde(default)]
    pub render_ms: Option<u64>,
    /// From its style and layout's start to its end, in milliseconds, where it had them.
    #[serde(default)]
    pub style_layout_ms: Option<u64>,
    /// Its longest scripts, longest first.
    #[serde(default)]
    pub scripts: Vec<ScriptTime>,
}

/// One function in a profile summary, with how many samples found it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFrame {
    /// The function's name; empty for an anonymous one.
    #[serde(rename = "fn")]
    pub function: String,
    /// Its file name and line (`voice.<hash>.js:1234`).
    pub src: String,
    /// The samples that found it.
    pub n: u64,
}

/// What the main-thread sampler saw inside an event's window.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileTop {
    /// Samples inside the window.
    pub samples: u64,
    /// Of those, the ones running page script (the rest: style, layout, paint, collection, idle).
    pub js_samples: u64,
    /// The sampling interval the browser used, in milliseconds.
    pub interval_ms: u64,
    /// The functions found running themselves, most samples first.
    #[serde(default)]
    pub self_top: Vec<ProfileFrame>,
    /// The functions found anywhere on the stack, most samples first.
    #[serde(default)]
    pub total_top: Vec<ProfileFrame>,
}

/// One freeze event's attribution: `FreezeEvent.debug`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EventDebug {
    /// The named spans that ran inside the event's window, most time first.
    #[serde(default)]
    pub spans: Vec<SpanTime>,
    /// The spans still running when the stall began, outermost first.
    #[serde(default)]
    pub open: Vec<OpenSpan>,
    /// The last spans to end before the stall began, newest first.
    #[serde(default)]
    pub recent: Vec<SpanEnd>,
    /// The stall's milliseconds no span covered: a large share says where a span is missing.
    #[serde(default)]
    pub unattributed_ms: Option<u64>,
    /// The long frame's breakdown, where the event is one and the browser gave it.
    #[serde(default)]
    pub frame: Option<FrameDetail>,
    /// The sampler's summary of the window, where it ran.
    #[serde(default)]
    pub profile: Option<ProfileTop>,
}

/// One span name's totals over a summary's period.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SpanTotal {
    /// The span's name.
    pub name: String,
    /// How many ran.
    pub n: u64,
    /// Their whole time, in milliseconds.
    pub total_ms: u64,
    /// The longest one, in milliseconds.
    pub max_ms: u64,
}

/// One long animation frame in a summary.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameBrief {
    /// When it started, `performance.now` in milliseconds.
    pub t: u64,
    /// How long it lasted, in milliseconds.
    pub dur_ms: u64,
    /// How long it blocked input, in milliseconds.
    pub blocking_ms: u64,
    /// Its longest script's function; empty when it named none.
    #[serde(rename = "fn")]
    pub function: String,
    /// Its longest script's file and position.
    pub src: String,
}

/// The debug mode's totals since `since`: `FreezeReport.debug` and `TraceReport.debug`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugSummary {
    /// What this browser offers the debug mode.
    pub caps: Caps,
    /// When the period began, `performance.now` in milliseconds (whole).
    pub since: u64,
    /// The span names with the most time, most first.
    #[serde(default)]
    pub spans: Vec<SpanTotal>,
    /// Long animation frames of 50 ms or more in the period.
    pub frames_over_50: u64,
    /// Of those, the ones of 100 ms or more.
    pub frames_over_100: u64,
    /// The longest frame in the period, in milliseconds.
    pub max_frame_ms: u64,
    /// The slowest long frames, slowest first.
    #[serde(default)]
    pub frames: Vec<FrameBrief>,
}

impl EventDebug {
    /// Refuse anything that is not a plausible attribution; `at` names the event.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self, at: &str) -> Result<(), String> {
        let name = |field: &str| format!("{at}.debug.{field}");
        check_len(&name("spans"), &self.spans, MAX_EVENT_SPANS)?;
        for (i, span) in self.spans.iter().enumerate() {
            check_name(&name(&format!("spans[{i}].name")), &span.name)?;
            check_count(&name(&format!("spans[{i}].n")), span.n)?;
            check_ms(&name(&format!("spans[{i}].ms")), span.ms)?;
            check_count(&name(&format!("spans[{i}].reads")), span.reads)?;
            for (field, value) in [
                ("s", span.s),
                ("d", span.d),
                ("chars", span.chars),
                ("rows", span.rows),
            ] {
                if let Some(count) = value {
                    check_count(&name(&format!("spans[{i}].{field}")), count)?;
                }
            }
            if span.nodes.is_some_and(|nodes| nodes > MAX_NODES) {
                return Err(format!(
                    "{} is larger than {MAX_NODES}",
                    name(&format!("spans[{i}].nodes"))
                ));
            }
        }
        check_len(&name("open"), &self.open, MAX_OPEN_SPANS)?;
        for (i, open) in self.open.iter().enumerate() {
            check_name(&name(&format!("open[{i}].name")), &open.name)?;
            check_ms(&name(&format!("open[{i}].age_ms")), open.age_ms)?;
        }
        check_len(&name("recent"), &self.recent, MAX_RECENT_SPANS)?;
        for (i, recent) in self.recent.iter().enumerate() {
            check_name(&name(&format!("recent[{i}].name")), &recent.name)?;
            check_ms(&name(&format!("recent[{i}].dur_ms")), recent.dur_ms)?;
            check_ms(&name(&format!("recent[{i}].ago_ms")), recent.ago_ms)?;
        }
        if let Some(ms) = self.unattributed_ms {
            check_ms(&name("unattributed_ms"), ms)?;
        }
        if let Some(frame) = &self.frame {
            check_ms(&name("frame.blocking_ms"), frame.blocking_ms)?;
            for (field, value) in [
                ("frame.render_ms", frame.render_ms),
                ("frame.style_layout_ms", frame.style_layout_ms),
            ] {
                if let Some(ms) = value {
                    check_ms(&name(field), ms)?;
                }
            }
            check_len(&name("frame.scripts"), &frame.scripts, MAX_FRAME_SCRIPTS)?;
            for (i, script) in frame.scripts.iter().enumerate() {
                let field = |part: &str| name(&format!("frame.scripts[{i}].{part}"));
                check_name(&field("fn"), &script.function)?;
                check_name(&field("src"), &script.src)?;
                check_name(&field("invoker"), &script.invoker)?;
                check_ms(&field("dur_ms"), script.dur_ms)?;
                check_ms(&field("forced_layout_ms"), script.forced_layout_ms)?;
            }
        }
        if let Some(profile) = &self.profile {
            check_count(&name("profile.samples"), profile.samples)?;
            check_count(&name("profile.js_samples"), profile.js_samples)?;
            if profile.js_samples > profile.samples {
                return Err(format!(
                    "{} is more than its samples",
                    name("profile.js_samples")
                ));
            }
            check_ms(&name("profile.interval_ms"), profile.interval_ms)?;
            for (list, frames) in [
                ("profile.self_top", &profile.self_top),
                ("profile.total_top", &profile.total_top),
            ] {
                check_len(&name(list), frames, MAX_PROFILE_FRAMES)?;
                for (i, frame) in frames.iter().enumerate() {
                    check_name(&name(&format!("{list}[{i}].fn")), &frame.function)?;
                    check_name(&name(&format!("{list}[{i}].src")), &frame.src)?;
                    check_count(&name(&format!("{list}[{i}].n")), frame.n)?;
                }
            }
        }
        Ok(())
    }

    /// The attribution's log fields: open spans as `name:age`, spans as
    /// `name:n:ms:reads:nodes:S:S-source:D:chars:rows`, recent ends as `name:dur:ago`, scripts as
    /// `type|invoker|fn|src|dur|forced`, profile frames as `fn@src:n`, each list quoted; `-`
    /// for what was not sent. The alphabet of [`is_code_name`] has none of `,` `|` `@` `"` `=`,
    /// so no name can pass for a separator or a field.
    #[must_use]
    pub fn log_fields(&self) -> String {
        let spans = self
            .spans
            .iter()
            .map(|span| {
                let or_dash =
                    |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |n| n.to_string());
                format!(
                    "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
                    span.name,
                    span.n,
                    span.ms,
                    span.reads,
                    or_dash(span.nodes),
                    or_dash(span.s),
                    span.s_src.map_or("-", SizeSource::as_str),
                    or_dash(span.d),
                    or_dash(span.chars),
                    or_dash(span.rows)
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let open = self
            .open
            .iter()
            .map(|open| format!("{}:{}", open.name, open.age_ms))
            .collect::<Vec<_>>()
            .join(",");
        let recent = self
            .recent
            .iter()
            .map(|end| format!("{}:{}:{}", end.name, end.dur_ms, end.ago_ms))
            .collect::<Vec<_>>()
            .join(",");
        let unattributed = self
            .unattributed_ms
            .map_or_else(|| "-".to_owned(), |ms| ms.to_string());
        let (frame, scripts) = self.frame.as_ref().map_or_else(
            || ("-".to_owned(), "-".to_owned()),
            |frame| {
                let number =
                    |value: Option<u64>| value.map_or_else(|| "-".to_owned(), |n| n.to_string());
                let scripts = frame
                    .scripts
                    .iter()
                    .map(|script| {
                        format!(
                            "{}|{}|{}|{}|{}|{}",
                            script.invoker_type.as_str(),
                            script.invoker,
                            script.function,
                            script.src,
                            script.dur_ms,
                            script.forced_layout_ms
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                (
                    format!(
                        "blocking:{},render:{},style_layout:{}",
                        frame.blocking_ms,
                        number(frame.render_ms),
                        number(frame.style_layout_ms)
                    ),
                    format!("{scripts:?}"),
                )
            },
        );
        let (profile, self_top, total_top) = self.profile.as_ref().map_or_else(
            || ("-".to_owned(), "-".to_owned(), "-".to_owned()),
            |profile| {
                (
                    format!(
                        "samples:{},js:{},interval:{}",
                        profile.samples, profile.js_samples, profile.interval_ms
                    ),
                    format!("{:?}", profile_list(&profile.self_top)),
                    format!("{:?}", profile_list(&profile.total_top)),
                )
            },
        );
        format!(
            "unattributed_ms={unattributed} open={open:?} spans={spans:?} recent={recent:?} \
             frame={frame} scripts={scripts} profile={profile} self_top={self_top} \
             total_top={total_top}"
        )
    }
}

fn profile_list(frames: &[ProfileFrame]) -> String {
    frames
        .iter()
        .map(|frame| format!("{}@{}:{}", frame.function, frame.src, frame.n))
        .collect::<Vec<_>>()
        .join(",")
}

impl DebugSummary {
    /// Refuse anything that is not a plausible summary.
    ///
    /// # Errors
    ///
    /// Returns a sentence naming the offending field.
    pub fn validate(&self) -> Result<(), String> {
        #[allow(clippy::cast_precision_loss)]
        check_time("debug.since", self.since as f64)?;
        check_ms("debug.caps.sample_ms", self.caps.sample_ms)?;
        check_len("debug.spans", &self.spans, MAX_SUMMARY_SPANS)?;
        for (i, span) in self.spans.iter().enumerate() {
            let field = |part: &str| format!("debug.spans[{i}].{part}");
            check_name(&field("name"), &span.name)?;
            check_count(&field("n"), span.n)?;
            check_ms(&field("max_ms"), span.max_ms)?;
            if span.total_ms > MAX_DUR_MS.saturating_mul(7) {
                return Err(format!("{} is larger than a week", field("total_ms")));
            }
        }
        check_count("debug.frames_over_50", self.frames_over_50)?;
        check_count("debug.frames_over_100", self.frames_over_100)?;
        if self.frames_over_100 > self.frames_over_50 {
            return Err("debug.frames_over_100 is more than debug.frames_over_50".to_owned());
        }
        check_ms("debug.max_frame_ms", self.max_frame_ms)?;
        check_len("debug.frames", &self.frames, MAX_SUMMARY_FRAMES)?;
        for (i, frame) in self.frames.iter().enumerate() {
            let field = |part: &str| format!("debug.frames[{i}].{part}");
            #[allow(clippy::cast_precision_loss)]
            check_time(&field("t"), frame.t as f64)?;
            check_ms(&field("dur_ms"), frame.dur_ms)?;
            check_ms(&field("blocking_ms"), frame.blocking_ms)?;
            check_name(&field("fn"), &frame.function)?;
            check_name(&field("src"), &frame.src)?;
        }
        Ok(())
    }

    /// The summary's one log line: the browser class `ua` (from [`ua_class`]), capabilities and
    /// counts plain, spans as `name:n:total:max` and frames as `t:dur:blocking:fn@src`, each
    /// list quoted.
    #[must_use]
    pub fn log_fields(&self, ua: &str) -> String {
        let spans = self
            .spans
            .iter()
            .map(|span| format!("{}:{}:{}:{}", span.name, span.n, span.total_ms, span.max_ms))
            .collect::<Vec<_>>()
            .join(",");
        let frames = self
            .frames
            .iter()
            .map(|frame| {
                format!(
                    "{}:{}:{}:{}@{}",
                    frame.t, frame.dur_ms, frame.blocking_ms, frame.function, frame.src
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "since={} ua={} loaf={} longtask={} event={} profiler={} policy={} sample_ms={} \
             frames_over_50={} frames_over_100={} max_frame_ms={} spans={spans:?} frames={frames:?}",
            self.since,
            ua,
            self.caps.loaf,
            self.caps.longtask,
            self.caps.event,
            self.caps.profiler,
            self.caps.policy,
            self.caps.sample_ms,
            self.frames_over_50,
            self.frames_over_100,
            self.max_frame_ms,
        )
    }
}

/// Test fixtures shared with the routes' tests: every list at its cap, every string at its cap.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    fn name(stem: &str) -> String {
        let mut text = stem.to_owned();
        while text.chars().count() < MAX_NAME_CHARS {
            text.push('x');
        }
        text
    }

    /// The largest [`EventDebug`] the caps allow.
    pub(crate) fn full_event_debug() -> EventDebug {
        EventDebug {
            spans: (0..MAX_EVENT_SPANS)
                .map(|_| SpanTime {
                    name: name("span"),
                    n: MAX_COUNT,
                    ms: MAX_DUR_MS,
                    reads: MAX_COUNT,
                    nodes: Some(MAX_NODES),
                    s: Some(MAX_COUNT),
                    s_src: Some(SizeSource::Drawn),
                    d: Some(MAX_COUNT),
                    chars: Some(MAX_COUNT),
                    rows: Some(MAX_COUNT),
                })
                .collect(),
            open: (0..MAX_OPEN_SPANS)
                .map(|_| OpenSpan {
                    name: name("open"),
                    age_ms: MAX_DUR_MS,
                })
                .collect(),
            recent: (0..MAX_RECENT_SPANS)
                .map(|_| SpanEnd {
                    name: name("recent"),
                    dur_ms: MAX_DUR_MS,
                    ago_ms: MAX_DUR_MS,
                })
                .collect(),
            unattributed_ms: Some(MAX_DUR_MS),
            frame: Some(FrameDetail {
                blocking_ms: MAX_DUR_MS,
                render_ms: Some(MAX_DUR_MS),
                style_layout_ms: Some(MAX_DUR_MS),
                scripts: (0..MAX_FRAME_SCRIPTS)
                    .map(|_| ScriptTime {
                        function: name("fn"),
                        src: name("src"),
                        invoker_type: InvokerType::EventListener,
                        invoker: name("invoker"),
                        dur_ms: MAX_DUR_MS,
                        forced_layout_ms: MAX_DUR_MS,
                    })
                    .collect(),
            }),
            profile: Some(ProfileTop {
                samples: MAX_COUNT,
                js_samples: MAX_COUNT,
                interval_ms: MAX_DUR_MS,
                self_top: (0..MAX_PROFILE_FRAMES)
                    .map(|_| ProfileFrame {
                        function: name("fn"),
                        src: name("src"),
                        n: MAX_COUNT,
                    })
                    .collect(),
                total_top: (0..MAX_PROFILE_FRAMES)
                    .map(|_| ProfileFrame {
                        function: name("fn"),
                        src: name("src"),
                        n: MAX_COUNT,
                    })
                    .collect(),
            }),
        }
    }

    /// The largest [`DebugSummary`] the caps allow.
    pub(crate) fn full_summary() -> DebugSummary {
        DebugSummary {
            caps: Caps {
                loaf: true,
                longtask: true,
                profiler: true,
                policy: true,
                event: true,
                sample_ms: MAX_DUR_MS,
            },
            since: 604_800_000,
            spans: (0..MAX_SUMMARY_SPANS)
                .map(|_| SpanTotal {
                    name: name("span"),
                    n: MAX_COUNT,
                    total_ms: MAX_DUR_MS,
                    max_ms: MAX_DUR_MS,
                })
                .collect(),
            frames_over_50: MAX_COUNT,
            frames_over_100: MAX_COUNT,
            max_frame_ms: MAX_DUR_MS,
            frames: (0..MAX_SUMMARY_FRAMES)
                .map(|_| FrameBrief {
                    t: 604_800_000,
                    dur_ms: MAX_DUR_MS,
                    blocking_ms: MAX_DUR_MS,
                    function: name("fn"),
                    src: name("src"),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{full_event_debug, full_summary};
    use super::*;

    #[test]
    fn an_attribution_reads_as_the_page_sends_it_and_logs_names_and_numbers_only() {
        let wire = serde_json::json!({
            "spans": [
                {"name": "key", "n": 1, "ms": 180, "reads": 2},
                {"name": "search-fast", "n": 2, "ms": 150, "reads": 0, "nodes": 5400, "s": 812, "s_src": "canon", "d": 140},
                {"name": "save-scope", "n": 1, "ms": 40, "reads": 0, "chars": 900000}
            ],
            "open": [{"name": "key", "age_ms": 3}],
            "recent": [{"name": "rows", "dur_ms": 12, "ago_ms": 30}],
            "unattributed_ms": 40,
            "frame": {
                "blocking_ms": 250, "render_ms": 30, "style_layout_ms": 12,
                "scripts": [{
                    "fn": "applySearchFast", "src": "voice.js:45678",
                    "invoker_type": "event-listener", "invoker": "DIV#discord-log.onkeydown",
                    "dur_ms": 180, "forced_layout_ms": 20
                }]
            },
            "profile": {
                "samples": 30, "js_samples": 25, "interval_ms": 10,
                "self_top": [{"fn": "drawSeams", "src": "voice.js:9", "n": 12}],
                "total_top": [{"fn": "applySearchFast", "src": "voice.js:7", "n": 25}]
            }
        });
        let debug: EventDebug = serde_json::from_value(wire).expect("the page's shape reads");
        debug.validate("events[0]").expect("valid");
        assert_eq!(
            debug.log_fields(),
            "unattributed_ms=40 open=\"key:3\" spans=\"key:1:180:2:-:-:-:-:-:-,search-fast:2:150:0:5400:812:canon:140:-:-,\
             save-scope:1:40:0:-:-:-:-:900000:-\" \
             recent=\"rows:12:30\" frame=blocking:250,render:30,style_layout:12 \
             scripts=\"event-listener|DIV#discord-log.onkeydown|applySearchFast|voice.js:45678|180|20\" \
             profile=samples:30,js:25,interval:10 self_top=\"drawSeams@voice.js:9:12\" \
             total_top=\"applySearchFast@voice.js:7:25\""
        );
        let bare: EventDebug = serde_json::from_value(serde_json::json!({})).expect("all optional");
        bare.validate("events[0]").expect("valid");
        assert_eq!(
            bare.log_fields(),
            "unattributed_ms=- open=\"\" spans=\"\" recent=\"\" frame=- scripts=- profile=- \
             self_top=- total_top=-"
        );
    }

    #[test]
    fn the_shell_is_asked_for_by_the_flag_or_the_cookie_only() {
        assert!(asked_for(Some("client-debug=1"), []));
        assert!(asked_for(Some("a=b&client-debug=1"), []));
        assert!(asked_for(None, ["theme=dark; vibe-talk-client-debug=1"]));
        assert!(asked_for(None, ["x=1", "vibe-talk-client-debug=1"]));
        assert!(!asked_for(None, []));
        assert!(!asked_for(
            Some("client-debug=0"),
            ["vibe-talk-client-debug=0"]
        ));
        assert!(!asked_for(
            Some("xclient-debug=1"),
            ["other-vibe-talk-client-debug=1"]
        ));
    }

    #[test]
    fn the_browser_class_is_a_family_and_a_major_version_only() {
        for (ua, class) in [
            ("Mozilla/5.0 (Linux; Android 14; Pixel 8 Build/AP2A; wv) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/131.0.6778.39 Mobile Safari/537.36", "webview-131"),
            ("Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36", "chrome-131"),
            ("Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/131.0 Safari/537.36 Edg/131.0", "edge-131"),
            ("Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0", "firefox-133"),
            ("Mozilla/5.0 (iPhone; CPU iPhone OS 18_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1", "safari-18"),
            ("curl/8.5", "other"),
            ("", "other"),
        ] {
            assert_eq!(ua_class(ua), class, "{ua}");
        }
    }

    #[test]
    fn a_summary_reads_and_logs() {
        let wire = serde_json::json!({
            "caps": {"loaf": false, "longtask": true, "profiler": true, "policy": false,
                     "event": true, "sample_ms": 16},
            "since": 1000,
            "spans": [{"name": "key", "n": 40, "total_ms": 900, "max_ms": 120}],
            "frames_over_50": 3, "frames_over_100": 1, "max_frame_ms": 140,
            "frames": [{"t": 5000, "dur_ms": 140, "blocking_ms": 90, "fn": "renderChannelRowsNow", "src": "voice.js:7"}]
        });
        let summary: DebugSummary = serde_json::from_value(wire).expect("reads");
        summary.validate().expect("valid");
        assert_eq!(
            summary.log_fields("webview-131"),
            "since=1000 ua=webview-131 loaf=false longtask=true event=true profiler=true \
             policy=false sample_ms=16 frames_over_50=3 frames_over_100=1 max_frame_ms=140 \
             spans=\"key:40:900:120\" frames=\"5000:140:90:renderChannelRowsNow@voice.js:7\""
        );
    }

    #[test]
    fn code_names_refuse_queries_tokens_addresses_separators_and_words() {
        for good in [
            "",
            "applySearchFast",
            "voice.js:45678",
            "voice.js:1:2345678",
            "voice.0123abcd.js:45678",
            "UTF8",
            "renderRows2",
            "DIV#discord-log.onkeydown",
            "Window.requestAnimationFrame",
            "<anonymous>",
            "get [Symbol.iterator]",
            "$el",
        ] {
            assert!(is_code_name(good), "refused {good:?}");
        }
        for bad in [
            "voice.js?token=abc",
            "a=b",
            "owner@example.com",
            "x&y",
            "say \"hi\"",
            "one,two",
            "a|b",
            "line\nbreak",
            "caf\u{e9}",
            "%41",
            "it's",
            // Ids: a Slack-style channel id, a message timestamp, a snowflake, a hash.
            "C0123ABCDEF",
            "row-C0123ABCDEF",
            "1732600000.123456",
            "m123456",
            "voice.0123456789abcdef0123.js",
        ] {
            assert!(!is_code_name(bad), "took {bad:?}");
        }
        let mut debug = full_event_debug();
        debug
            .validate("events[0]")
            .expect("the caps themselves are valid");
        debug.spans[0].name = "x".repeat(MAX_NAME_CHARS + 1);
        assert!(debug.validate("events[0]").is_err());
        let mut debug = full_event_debug();
        debug.spans.push(debug.spans[0].clone());
        assert!(debug.validate("events[0]").is_err(), "one span too many");
        let mut debug = full_event_debug();
        if let Some(frame) = debug.frame.as_mut() {
            frame.scripts[0].src = "https://host/voice.js?token=secret".to_owned();
        }
        assert!(debug.validate("events[0]").is_err(), "a URL with a query");
        let mut debug = full_event_debug();
        if let Some(profile) = debug.profile.as_mut() {
            profile.js_samples = 1;
            profile.samples = 0;
        }
        assert!(
            debug.validate("events[0]").is_err(),
            "more js samples than samples"
        );
    }

    #[test]
    fn a_span_name_with_an_email_a_url_query_or_a_slack_id_is_refused_server_side() {
        // The liaison's privacy check (`#2276` review): each, as a span name, refuses the report.
        for leak in [
            "send-owner@example.com",
            "https://chat.example/archives?token=xoxb-123",
            "thread-C0123ABCDEF",
        ] {
            let mut debug = full_event_debug();
            debug.spans[0].name = leak.to_owned();
            assert!(
                debug.validate("events[0]").is_err(),
                "span name took {leak:?}"
            );
            let mut summary = full_summary();
            summary.spans[0].name = leak.to_owned();
            assert!(summary.validate().is_err(), "summary span took {leak:?}");
            let mut debug = full_event_debug();
            debug.open[0].name = leak.to_owned();
            assert!(
                debug.validate("events[0]").is_err(),
                "open span took {leak:?}"
            );
        }
        let mut wire = serde_json::to_value(full_summary()).expect("serializes");
        wire["caps"]["ua"] = "Mozilla/5.0 (Linux; Android 14)".into();
        assert!(
            serde_json::from_value::<DebugSummary>(wire).is_err(),
            "the page never sends a user agent"
        );
    }

    #[test]
    fn a_summary_refuses_oversize_lists_and_impossible_numbers() {
        full_summary()
            .validate()
            .expect("the caps themselves are valid");
        let mut summary = full_summary();
        summary.frames.push(summary.frames[0].clone());
        assert!(summary.validate().is_err());
        let mut summary = full_summary();
        summary.spans.push(summary.spans[0].clone());
        assert!(summary.validate().is_err());
        let mut summary = full_summary();
        summary.frames_over_50 = 0;
        assert!(summary.validate().is_err(), "more over 100 than over 50");
        let mut summary = full_summary();
        summary.since = 604_800_001;
        assert!(summary.validate().is_err());
        let mut summary = full_summary();
        summary.spans[0].name = "message words, here".to_owned();
        assert!(summary.validate().is_err());
        let mut wire = serde_json::to_value(full_summary()).expect("serializes");
        wire["text"] = "the words on screen".into();
        assert!(serde_json::from_value::<DebugSummary>(wire).is_err());
        let mut wire = serde_json::to_value(full_event_debug()).expect("serializes");
        wire["frame"]["scripts"][0]["invoker_type"] = "teleport".into();
        assert!(serde_json::from_value::<EventDebug>(wire).is_err());
    }
}
