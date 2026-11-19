//! Agent transcript threads: classification and routing. `#1608 agent-transcripts-timeline`
//! (design: ai_docs/plans/2026-11-03-vibe-talk-transcript-threads-design.md, Owner 101).
//!
//! A transcript post is the bridge's trace of one agent's main TUI session
//! (`agentctl chat run --trace-thread`, #714/#832). It never enters the raw
//! message feed; it lives only in the Transcripts view. This module is the
//! one predicate — "is transcript" — and it *routes*, never hides: every
//! raw-feed surface excludes what this module classes, and the Transcripts
//! surface includes exactly that.
//!
//! Two signals, in order (design section 2.1):
//! 1. Slack message metadata: `event_type = "agentctl_trace"`, payload
//!    `v` 1 or 2, on the day root (`kind: "root"`) or any entry
//!    (`kind: "entry"`). Entries carry agent and day too, so a reply read
//!    before its root still classes its thread.
//! 2. Root-title fallback, for posts made before #832 and transports that
//!    drop metadata: a root whose whole text is
//!    `<agent> main session trace, YYYY-MM-DD ET`.

use serde::{Deserialize, Serialize};

/// How a thread came to be classed. Sticky: once classed, an edit or a
/// metadata-less re-read never unclasses; only `Owner` changes a class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClassSource {
    /// Slack `agentctl_trace` metadata (primary).
    Metadata,
    /// The fixed root title (fallback).
    Title,
    /// The owner's explicit override.
    Owner,
}

impl ClassSource {
    /// The stored spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Metadata => "metadata",
            Self::Title => "title",
            Self::Owner => "owner",
        }
    }

    /// A stored spelling read back, or `None` for one this build does not know.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "metadata" => Some(Self::Metadata),
            "title" => Some(Self::Title),
            "owner" => Some(Self::Owner),
            _ => None,
        }
    }
}

/// A thread classed as one agent's transcript for one ET day.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ThreadClass {
    /// The agent whose main session this traces.
    pub agent: String,
    /// The agent-teams team, when the bridge knew it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    /// The ET day, `YYYY-MM-DD`.
    pub day: String,
    /// How this class was decided.
    pub source: ClassSource,
}

/// The `event_type` the bridge files trace metadata under (#832).
pub const TRACE_EVENT_TYPE: &str = "agentctl_trace";

/// The design cap on one trace metadata payload, in bytes (section 2.1):
/// at most 8 reply references, whole payload under 2 KB, far inside the
/// transport shim's 8 KB. A payload past this is not read as metadata.
pub const MAX_TRACE_PAYLOAD_BYTES: usize = 2_048;

/// Classify from a Slack message object's `metadata`, or `None`.
///
/// Accepts payload `v` 1 and 2 (v2 readers accept v1: no `sent` field means
/// no timeline cards, never no class). Refuses another app's metadata, an
/// unknown version, a missing agent/day, and an oversize payload.
#[must_use]
pub fn class_of_metadata(value: &serde_json::Value) -> Option<ThreadClass> {
    let metadata = value.get("metadata")?;
    if metadata
        .get("event_type")
        .and_then(serde_json::Value::as_str)
        != Some(TRACE_EVENT_TYPE)
    {
        return None;
    }
    let payload = metadata.get("event_payload")?;
    if serde_json::to_string(payload).ok()?.len() > MAX_TRACE_PAYLOAD_BYTES {
        return None;
    }
    let version = payload.get("v").and_then(serde_json::Value::as_u64)?;
    if version != 1 && version != 2 {
        return None;
    }
    let kind = payload.get("kind").and_then(serde_json::Value::as_str)?;
    if kind != "root" && kind != "entry" {
        return None;
    }
    let agent = payload
        .get("agent")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())?
        .to_owned();
    let day = payload
        .get("day")
        .and_then(serde_json::Value::as_str)
        .filter(|text| is_day(text))?
        .to_owned();
    let team = payload
        .get("team")
        .and_then(serde_json::Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned);
    Some(ThreadClass {
        agent,
        team,
        day,
        source: ClassSource::Metadata,
    })
}

/// Classify from a thread root's whole text (the title fallback), or `None`.
///
/// The text must match `^(\S+) main session trace, (\d{4}-\d{2}-\d{2}) ET$`
/// exactly: a title quoted inside a longer reply is conversation, not a
/// trace root.
#[must_use]
pub fn class_of_title(text: &str) -> Option<ThreadClass> {
    let (agent, rest) = text.trim().split_once(' ')?;
    if agent.is_empty() || agent.chars().any(char::is_whitespace) {
        return None;
    }
    let day = rest
        .strip_prefix("main session trace, ")?
        .strip_suffix(" ET")?;
    if !is_day(day) {
        return None;
    }
    Some(ThreadClass {
        agent: agent.to_owned(),
        team: None,
        day: day.to_owned(),
        source: ClassSource::Title,
    })
}

/// Whether `text` is a `YYYY-MM-DD` day with plausible month and day fields.
fn is_day(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| {
        text.get(range)
            .filter(|part| part.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|part| part.parse::<u32>().ok())
    };
    let (Some(_year), Some(month), Some(day)) = (digits(0..4), digits(5..7), digits(8..10)) else {
        return false;
    };
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

/// One chat message a v2 trace entry says the agent sent before it
/// (`#1886 vibe-talk-transcripts-view` part C; the bridge spec is in
/// #1886's notes): the thread it went into, the message itself, its
/// instant, and how long it was. The Transcripts timeline stands these
/// in where they happened, as vestigial entries.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SentRef {
    /// The thread the sent message went into (its own `msg` when top-level).
    pub thread: String,
    /// The sent message's own id (`ts` on Slack).
    pub msg: String,
    /// The sent message's instant (`ts` text), for ordering when the
    /// message itself cannot be read back.
    pub ts: String,
    /// How many characters the sent text held.
    pub chars: u64,
}

/// The most `sent` references one entry may carry: the design cap of
/// section 2.1, which is also what keeps a busy day's metadata inside
/// the shim's whole-payload budget (the bridge drops its oldest pending
/// refs past the budget rather than truncating one).
pub const MAX_SENT_REFS: usize = 8;

/// The `sent` references a trace message's own metadata carries, or an
/// empty list: v1 metadata, another app's, a malformed list. Only a
/// payload [`class_of_metadata`] accepts is read at all, and only at
/// `v: 2` — v1 readers and bridges keep working unchanged, with no
/// timeline cards (the design's rule).
#[must_use]
pub fn sent_refs_of_metadata(value: &serde_json::Value) -> Vec<SentRef> {
    if class_of_metadata(value).is_none() {
        return Vec::new();
    }
    let Some(payload) = value.get("metadata").and_then(|m| m.get("event_payload")) else {
        return Vec::new();
    };
    if payload.get("v").and_then(serde_json::Value::as_u64) != Some(2) {
        return Vec::new();
    }
    let Some(list) = payload.get("sent").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|item| {
            let msg = item
                .get("msg")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())?
                .to_owned();
            let thread = item
                .get("thread")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| msg.clone());
            let ts = item
                .get("ts")
                .and_then(serde_json::Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| msg.clone());
            let chars = item
                .get("chars")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            Some(SentRef {
                thread,
                msg,
                ts,
                chars,
            })
        })
        .take(MAX_SENT_REFS)
        .collect()
}

/// Whether a message text opens with the legacy label line old trace posts
/// carried (`— after: <trigger>`, #833's surviving fragment). Raw and
/// timeline rendering drop that one leading line in a transcript.
#[must_use]
pub fn strip_legacy_label(text: &str) -> &str {
    match text.lines().next() {
        Some(first) if first.starts_with("— after: ") => text
            .get(first.len()..)
            .map_or("", |rest| rest.strip_prefix('\n').unwrap_or(rest)),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata(agent: &str, day: &str, kind: &str) -> serde_json::Value {
        json!({"ts": "1794746400.000100", "text": "thinking",
               "metadata": {"event_type": "agentctl_trace",
                            "event_payload": {"v": 1, "kind": kind, "agent": agent,
                                              "team": null, "session": "s1",
                                              "day": day, "seq": 3,
                                              "trigger": "owner message"}}})
    }

    #[test]
    fn metadata_root_and_entry_both_class() {
        for kind in ["root", "entry"] {
            let class = class_of_metadata(&metadata("coordx", "2026-11-15", kind))
                .expect("trace metadata classes");
            assert_eq!(class.agent, "coordx");
            assert_eq!(class.day, "2026-11-15");
            assert_eq!(class.source, ClassSource::Metadata);
        }
    }

    #[test]
    fn v2_sent_refs_parse_and_v1_has_none() {
        // `#1886` part C: a v2 entry's `sent` list, parsed whole; a v1
        // entry - and an entry from the test above with refs missing
        // parts filled from `msg` - per the bridge spec.
        let sent = sent_refs_of_metadata(&metadata("liaison", "2026-11-19", "entry"));
        assert!(sent.is_empty(), "v1 metadata carries no sent refs");
        let mut v2 = metadata("liaison", "2026-11-19", "entry");
        v2["metadata"]["event_payload"]["v"] = json!(2);
        v2["metadata"]["event_payload"]["sent"] = json!([
            {"thread": "1763510400.000100", "msg": "1763510400.000100", "ts": "1763510400.000100", "chars": 212},
            {"msg": "1763510460.000300"}
        ]);
        let sent = sent_refs_of_metadata(&v2);
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].chars, 212);
        assert_eq!(sent[1].thread, "1763510460.000300", "thread falls back to the message");
        assert_eq!(sent[1].ts, "1763510460.000300");
        // Past the cap, only the first MAX_SENT_REFS stand.
        v2["metadata"]["event_payload"]["sent"] = json!((0..20).map(|n| json!({"msg": format!("{n}.1")})).collect::<Vec<_>>());
        assert_eq!(sent_refs_of_metadata(&v2).len(), MAX_SENT_REFS);
        // Another app's metadata never yields refs, however v2-shaped.
        v2["metadata"]["event_type"] = json!("other");
        assert!(sent_refs_of_metadata(&v2).is_empty());
    }

    #[test]
    fn v2_metadata_classes_and_v1_shape_is_not_required() {
        let mut value = metadata("lane-a", "2026-11-15", "entry");
        value["metadata"]["event_payload"]["v"] = json!(2);
        value["metadata"]["event_payload"]["sent"] = json!([{"thread": ":thread:060", "msg": ":email:1846",
                    "ts": "1794746400.000100", "chars": 812}]);
        assert!(class_of_metadata(&value).is_some());
    }

    #[test]
    fn other_apps_and_bad_shapes_do_not_class() {
        let mut other = metadata("coordx", "2026-11-15", "entry");
        other["metadata"]["event_type"] = json!("vibe_talk_post");
        assert_eq!(class_of_metadata(&other), None);
        let mut bad_day = metadata("coordx", "15 Nov 2026", "entry");
        bad_day["metadata"]["event_payload"]["day"] = json!("2026-13-40");
        assert_eq!(class_of_metadata(&bad_day), None);
        let mut bad_version = metadata("coordx", "2026-11-15", "entry");
        bad_version["metadata"]["event_payload"]["v"] = json!(3);
        assert_eq!(class_of_metadata(&bad_version), None);
    }

    #[test]
    fn an_oversize_payload_is_not_read_as_metadata() {
        let mut value = metadata("coordx", "2026-11-15", "entry");
        value["metadata"]["event_payload"]["pad"] = json!("x".repeat(MAX_TRACE_PAYLOAD_BYTES));
        assert_eq!(class_of_metadata(&value), None);
    }

    #[test]
    fn the_fixed_title_classes_and_near_misses_do_not() {
        let class = class_of_title("coordx main session trace, 2026-11-15 ET").expect("title");
        assert_eq!(class.source, ClassSource::Title);
        for miss in [
            "coordx main session trace, ET",
            "coordx main session trace, 2026-11-15",
            "as coordx said: coordx main session trace, 2026-11-15 ET",
            "coordx main session trace, 2026-11-15 ET and more",
            "main session trace, 2026-11-15 ET",
        ] {
            assert_eq!(class_of_title(miss), None, "{miss:?}");
        }
    }

    #[test]
    fn the_legacy_label_line_is_dropped_only_at_the_head() {
        assert_eq!(strip_legacy_label("— after: owner message\nhi"), "hi");
        assert_eq!(strip_legacy_label("hi\n— after: x"), "hi\n— after: x");
        assert_eq!(strip_legacy_label("— after: owner message"), "");
    }
}
