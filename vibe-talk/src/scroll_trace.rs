//! What moved the channel's lists, as bounded log lines.
//!
//! `#1144 scrollback-typing-jump`, part 2b. A scroll jump on the owner's phone leaves nothing
//! behind it: the list is somewhere else, and no code here or on the page can say what moved it.
//! With Settings' scroll trace on, the page keeps the last minute of scrolls of its two lists —
//! every write to them, with the words of code that asked for it — and "Send scroll trace" posts
//! that minute here (`POST /api/v1/scroll-trace`), where it becomes log lines an operator can
//! read back beside the rest of the page's records. Off unless the owner turns it on; the page
//! sends nothing until he presses Send.
//!
//! **Closed like the voice records** ([`crate::voice_health`]): [`TraceReport`] is a fixed set of
//! fields, each capped, and an unknown field, an unknown [`TraceKind`] or an oversize string is
//! refused rather than echoed. The one free-text field is an entry's `stack` — the page code's own
//! call words, never a message's — capped at [`MAX_STACK_CHARS`] with control characters refused,
//! and quoted in the line as the voice service's close reason is, so it cannot pass for a field.
//!
//! **Bounded.** [`MAX_BODY_BYTES`] caps what the route reads, [`MAX_ENTRIES`] caps one report's
//! entries, and the shared [`crate::voice_health::LogBudget`] caps the reports logged per minute
//! across every caller, one spend per report however many lines its entries make.

use serde::{Deserialize, Serialize};

/// The largest request body the route reads. A full minute at [`MAX_ENTRIES`] with every string
/// at its cap fits inside it; the route answers 413 past it.
pub const MAX_BODY_BYTES: usize = 256 * 1_024;

/// The most entries one report carries: the page's ring holds no more.
pub const MAX_ENTRIES: usize = 600;

/// The longest element or focus id an entry carries, in characters.
pub const MAX_ID_CHARS: usize = 128;

/// The longest anchor row id an entry carries, in characters: a message id, with room to spare.
pub const MAX_ANCHOR_ID_CHARS: usize = 256;

/// The longest stack an entry carries, in characters: four frames of the page's own code, cut by
/// the page to this order before it sends them.
pub const MAX_STACK_CHARS: usize = 512;

/// The longest clock reading accepted, in milliseconds: a week of `performance.now` since the
/// page loaded. Past that the number is not a measurement of this page.
pub const MAX_TIME_MS: f64 = 604_800_000.0;

/// The largest pixel measure accepted, either way from zero: far past any list a browser lays out.
pub const MAX_METRIC_PX: f64 = 100_000_000.0;

/// What kind of scroll one entry records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceKind {
    /// Code wrote a list's position: its `scrollTop`, or `scrollTo`/`scrollBy` on it.
    Write,
    /// A list scrolled (`scroll` heard on it), whoever moved it.
    Scroll,
    /// Code asked for an element to be shown (`scrollIntoView`), on any element.
    Into,
    /// The layout moved under the reader (`#1208`): a `layout-shift` entry and its sources.
    Shift,
    /// A row was replaced for the same message (`#1208`).
    Mutate,
    /// A live selection collapsed with no hand in it (`#1208`).
    Selection,
}

impl TraceKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Scroll => "scroll",
            Self::Into => "into",
            Self::Shift => "shift",
            Self::Mutate => "mutate",
            Self::Selection => "selection",
        }
    }
}

/// Where the channel's composer stood, in CSS pixels from the viewport's top.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComposerRect {
    /// Its top edge.
    pub top: f64,
    /// Its bottom edge.
    pub bottom: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

/// The visual viewport, as far as a keyboard moves it.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VisualViewport {
    /// Its height, in CSS pixels.
    pub height: f64,
    /// Its top edge's offset, in CSS pixels.
    #[serde(rename = "offsetTop")]
    pub offset_top: f64,
}

/// The first row the reader could see, and how far down their list it stood.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceAnchor {
    /// The row's first message id.
    pub id: String,
    /// The row's top below its list's top, in CSS pixels.
    pub offset: f64,
}

/// One scroll, as the page saw it. The field set IS the allowlist.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceEntry {
    /// When it happened, `performance.now` in milliseconds.
    pub t: f64,
    /// What kind of scroll it was.
    pub kind: TraceKind,
    /// The id of the element scrolled, or asked to be shown.
    pub id: String,
    /// The list's position before, where the kind has one.
    pub from: Option<f64>,
    /// The position asked for or arrived at, where the kind has one.
    pub to: Option<f64>,
    /// The first frames of the code that asked, one line; empty for a scroll nobody asked this
    /// page for. At most [`MAX_STACK_CHARS`], with no control characters.
    pub stack: String,
    /// The id of the element holding the focus, where it had one.
    pub focus: Option<String>,
    /// Where the channel's composer stood, where the page found it.
    pub composer: Option<ComposerRect>,
    /// The visual viewport, where the browser has one.
    pub vv: Option<VisualViewport>,
    /// The scrolled element's whole height, in CSS pixels.
    #[serde(rename = "scrollHeight")]
    pub scroll_height: f64,
    /// The scrolled element's shown height, in CSS pixels.
    #[serde(rename = "clientHeight")]
    pub client_height: f64,
    /// The first row the reader could see, where the page named one.
    pub anchor: Option<TraceAnchor>,
    /// A layout shift's size, as a fraction of the viewport (`shift` only, `#1208`).
    #[serde(default)]
    pub value: Option<f64>,
    /// Whether a hand had just been in a layout shift (`shift` only, `#1208`).
    #[serde(default)]
    pub input: Option<bool>,
}

/// One minute of a page's scrolls: `POST /api/v1/scroll-trace`. The field set IS the allowlist,
/// as for [`crate::voice_health::EndReport`].
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TraceReport {
    /// The report's version. Only `1` is defined.
    pub v: u32,
    /// When this trace started, `performance.now` in milliseconds — the entries' own clock.
    #[serde(rename = "startedAt")]
    pub started_at: f64,
    /// The scrolls, oldest first. At most [`MAX_ENTRIES`].
    pub entries: Vec<TraceEntry>,
}

fn check_text(name: &str, value: &str, max: usize) -> Result<(), String> {
    if value.chars().count() > max {
        return Err(format!("{name} is longer than {max} characters"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{name} carries a control character"));
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

fn check_metric(name: &str, value: f64) -> Result<(), String> {
    if !value.is_finite() || value.abs() > MAX_METRIC_PX {
        return Err(format!("{name} is larger than {MAX_METRIC_PX} px"));
    }
    Ok(())
}

impl TraceReport {
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
        if self.entries.len() > MAX_ENTRIES {
            return Err(format!("entries is more than {MAX_ENTRIES}"));
        }
        for (at, entry) in self.entries.iter().enumerate() {
            entry.validate(at)?;
        }
        Ok(())
    }

    /// The line that opens a report in the log: its version, its start, and how many lines follow.
    #[must_use]
    pub fn header_fields(&self) -> String {
        format!(
            "v={} started_at={} entries={}",
            self.v,
            self.started_at,
            self.entries.len()
        )
    }
}

impl TraceEntry {
    fn validate(&self, at: usize) -> Result<(), String> {
        let name = |field: &str| format!("entries[{at}].{field}");
        check_time(&name("t"), self.t)?;
        check_text(&name("id"), &self.id, MAX_ID_CHARS)?;
        for (field, value) in [("from", self.from), ("to", self.to)] {
            if let Some(px) = value {
                check_metric(&name(field), px)?;
            }
        }
        check_text(&name("stack"), &self.stack, MAX_STACK_CHARS)?;
        if let Some(focus) = &self.focus {
            check_text(&name("focus"), focus, MAX_ID_CHARS)?;
        }
        if let Some(rect) = &self.composer {
            for (field, value) in [
                ("composer.top", rect.top),
                ("composer.bottom", rect.bottom),
                ("composer.width", rect.width),
                ("composer.height", rect.height),
            ] {
                check_metric(&name(field), value)?;
            }
        }
        if let Some(vv) = &self.vv {
            check_metric(&name("vv.height"), vv.height)?;
            check_metric(&name("vv.offsetTop"), vv.offset_top)?;
        }
        check_metric(&name("scrollHeight"), self.scroll_height)?;
        check_metric(&name("clientHeight"), self.client_height)?;
        if let Some(value) = self.value {
            check_metric(&name("value"), value)?;
        }
        if let Some(anchor) = &self.anchor {
            check_text(&name("anchor.id"), &anchor.id, MAX_ANCHOR_ID_CHARS)?;
            check_metric(&name("anchor.offset"), anchor.offset)?;
        }
        Ok(())
    }

    /// The entry's one log line: numbers and the kind plain, every string quoted as the stack is,
    /// so none of them can pass for a field; `-` for what was not sent.
    #[must_use]
    pub fn log_fields(&self, at: usize) -> String {
        let number = |value: Option<f64>| value.map_or_else(|| "-".to_owned(), |n| n.to_string());
        let quoted = |value: Option<&String>| {
            value.map_or_else(|| "-".to_owned(), |text| format!("{text:?}"))
        };
        let composer = self.composer.map_or_else(
            || "-".to_owned(),
            |rect| {
                format!(
                    "{},{},{},{}",
                    rect.top, rect.bottom, rect.width, rect.height
                )
            },
        );
        let vv = self.vv.map_or_else(
            || "-".to_owned(),
            |vv| format!("{},{}", vv.height, vv.offset_top),
        );
        let anchor = self.anchor.as_ref().map_or_else(
            || "-".to_owned(),
            |anchor| format!("{:?}@{}", anchor.id, anchor.offset),
        );
        format!(
            "i={at} t={} kind={} id={:?} from={} to={} focus={} composer={} vv={} \
             scroll_height={} client_height={} anchor={} value={} input={} stack={:?}",
            self.t,
            self.kind.as_str(),
            self.id,
            number(self.from),
            number(self.to),
            quoted(self.focus.as_ref()),
            composer,
            vv,
            self.scroll_height,
            self.client_height,
            anchor,
            number(self.value),
            self.input
                .map_or_else(|| "-".to_owned(), |yes| yes.to_string()),
            self.stack,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> TraceEntry {
        TraceEntry {
            t: 12_345.5,
            kind: TraceKind::Write,
            id: "scroll-area".to_owned(),
            from: Some(400.0),
            to: Some(900.0),
            stack: "at followFoot (voice.js:1:2) | at render (voice.js:3:4)".to_owned(),
            focus: Some("channel-compose-text".to_owned()),
            composer: Some(ComposerRect {
                top: 600.0,
                bottom: 660.0,
                width: 390.0,
                height: 60.0,
            }),
            vv: Some(VisualViewport {
                height: 500.0,
                offset_top: 120.0,
            }),
            scroll_height: 9_000.0,
            client_height: 700.0,
            anchor: Some(TraceAnchor {
                id: "m-1".to_owned(),
                offset: -12.0,
            }),
            value: None,
            input: None,
        }
    }

    fn report(entries: Vec<TraceEntry>) -> TraceReport {
        TraceReport {
            v: 1,
            started_at: 10_000.0,
            entries,
        }
    }

    #[test]
    fn a_report_reads_as_the_page_sends_it_and_logs_quoted_strings_only() {
        let wire = serde_json::json!({
            "v": 1, "startedAt": 10000.0,
            "entries": [{
                "t": 12345.5, "kind": "write", "id": "scroll-area", "from": 400, "to": 900,
                "stack": "at followFoot (voice.js:1:2) | at render (voice.js:3:4)",
                "focus": "channel-compose-text",
                "composer": {"top": 600, "bottom": 660, "width": 390, "height": 60},
                "vv": {"height": 500, "offsetTop": 120},
                "scrollHeight": 9000, "clientHeight": 700,
                "anchor": {"id": "m-1", "offset": -12},
            }],
        });
        let parsed: TraceReport = serde_json::from_value(wire).expect("the page's shape reads");
        assert_eq!(parsed, report(vec![entry()]));
        parsed.validate().expect("valid");
        assert_eq!(parsed.header_fields(), "v=1 started_at=10000 entries=1");
        let line = parsed.entries[0].log_fields(0);
        assert_eq!(
            line,
            "i=0 t=12345.5 kind=write id=\"scroll-area\" from=400 to=900 \
             focus=\"channel-compose-text\" composer=600,660,390,60 vv=500,120 \
             scroll_height=9000 client_height=700 anchor=\"m-1\"@-12 value=- input=- \
             stack=\"at followFoot (voice.js:1:2) | at render (voice.js:3:4)\""
        );
        // A stack that tries to look like more fields stays inside its quotes.
        let forged = TraceEntry {
            stack: "x\" kind=scroll".to_owned(),
            ..entry()
        };
        assert!(
            forged.log_fields(0).ends_with(r#"stack="x\" kind=scroll""#),
            "{}",
            forged.log_fields(0)
        );
        // An empty minute is a report too: the button says so rather than failing.
        report(Vec::new()).validate().expect("valid");
        for (kind, word) in [
            (TraceKind::Write, "write"),
            (TraceKind::Scroll, "scroll"),
            (TraceKind::Into, "into"),
        ] {
            assert_eq!(serde_json::to_value(kind).expect("serializes"), word);
        }
    }

    #[test]
    fn a_report_refuses_unknown_fields_kinds_and_another_version() {
        let mut smuggled = serde_json::to_value(report(vec![entry()])).expect("serializes");
        smuggled["entries"][0]["message"] = "the words on screen".into();
        assert!(
            serde_json::from_value::<TraceReport>(smuggled).is_err(),
            "a free field rode in"
        );
        let mut smuggled = serde_json::to_value(report(vec![entry()])).expect("serializes");
        smuggled["device"] = "the owner's phone".into();
        assert!(
            serde_json::from_value::<TraceReport>(smuggled).is_err(),
            "a free field rode in"
        );
        let mut unknown = serde_json::to_value(report(vec![entry()])).expect("serializes");
        unknown["entries"][0]["kind"] = "teleport".into();
        assert!(serde_json::from_value::<TraceReport>(unknown).is_err());
        assert!(TraceReport {
            v: 2,
            ..report(Vec::new())
        }
        .validate()
        .is_err());
    }

    #[test]
    fn a_report_refuses_oversize_strings_counts_and_impossible_numbers() {
        report(vec![entry(); MAX_ENTRIES])
            .validate()
            .expect("a full minute is valid");
        assert!(report(vec![entry(); MAX_ENTRIES + 1]).validate().is_err());
        for absurd in [
            TraceEntry {
                stack: "x".repeat(MAX_STACK_CHARS + 1),
                ..entry()
            },
            TraceEntry {
                stack: "frame one\nframe two".to_owned(),
                ..entry()
            },
            TraceEntry {
                id: "x".repeat(MAX_ID_CHARS + 1),
                ..entry()
            },
            TraceEntry {
                t: f64::INFINITY,
                ..entry()
            },
            TraceEntry { t: -1.0, ..entry() },
            TraceEntry {
                to: Some(f64::NAN),
                ..entry()
            },
            TraceEntry {
                scroll_height: MAX_METRIC_PX + 1.0,
                ..entry()
            },
            TraceEntry {
                anchor: Some(TraceAnchor {
                    id: "x".repeat(MAX_ANCHOR_ID_CHARS + 1),
                    offset: 0.0,
                }),
                ..entry()
            },
        ] {
            assert!(report(vec![absurd]).validate().is_err());
        }
        // Counted in characters, not bytes, as the close reason is.
        report(vec![TraceEntry {
            stack: "é".repeat(MAX_STACK_CHARS),
            ..entry()
        }])
        .validate()
        .expect("512 characters of two bytes each is still 512 characters");
    }
}
