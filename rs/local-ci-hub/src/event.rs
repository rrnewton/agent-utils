//! Typed ledger events, envelope validation, and the refusing union.
//!
//! Events are slim, append-only facts. Committed bytes are never edited; a
//! wrong fact is superseded by `run.correct`. The union collapses identical
//! copies of one event and refuses the whole read when the same `event_id`
//! carries a different body, compared on written pairs. There is no
//! first-wins and no last-wins for conflicting bodies.

use crate::pyjson::{dumps_pairs, parse, py_repr, py_str, PyValue};
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;

/// Input event schema tag this reader accepts.
pub const SCHEMA: &str = "validate-ledger/v1";

/// Closed event-kind vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A run started.
    RunStart,
    /// A run finished with an outcome.
    RunResult,
    /// Fill fields that are absent on the run; never overwrites.
    RunEnrich,
    /// Supersede a wrong fact; requires an existing run and a reason.
    RunCorrect,
    /// Metadata only; never changes derived state.
    RunAnnotate,
}

impl EventKind {
    /// Parse the wire name (`run.result` and friends).
    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
            "run.start" => Some(Self::RunStart),
            "run.result" => Some(Self::RunResult),
            "run.enrich" => Some(Self::RunEnrich),
            "run.correct" => Some(Self::RunCorrect),
            "run.annotate" => Some(Self::RunAnnotate),
            _ => None,
        }
    }

    /// The wire name.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::RunStart => "run.start",
            Self::RunResult => "run.result",
            Self::RunEnrich => "run.enrich",
            Self::RunCorrect => "run.correct",
            Self::RunAnnotate => "run.annotate",
        }
    }
}

/// Closed run-outcome vocabulary (envelope `outcome` only; row results are
/// open strings and are never validated against this set).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Tests ran and passed.
    Pass,
    /// Tests ran and failed.
    Fail,
    /// The run hit its time budget.
    Timeout,
    /// The run was killed.
    Killed,
    /// The run stopped before a verdict.
    Incomplete,
    /// No result was produced; never evidence of green.
    NoResult,
}

impl Outcome {
    /// Parse the wire name.
    pub fn from_wire(name: &str) -> Option<Self> {
        match name {
            "pass" => Some(Self::Pass),
            "fail" => Some(Self::Fail),
            "timeout" => Some(Self::Timeout),
            "killed" => Some(Self::Killed),
            "incomplete" => Some(Self::Incomplete),
            "no_result" => Some(Self::NoResult),
            _ => None,
        }
    }
}

/// One admitted event: its stored line and its parsed value.
#[derive(Clone, Debug)]
pub struct RawEvent {
    /// The stored line text (no trailing newline). For expanded stored
    /// lines this is the canonical expanded text.
    pub line: String,
    /// The parsed event object with written pairs preserved.
    pub value: PyValue,
}

impl RawEvent {
    /// Dict-view lookup (last value wins).
    pub fn get(&self, key: &str) -> Option<&PyValue> {
        self.value.get(key)
    }

    /// Dict-view string lookup.
    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(PyValue::as_str)
    }

    /// The event kind, from `event_type` (falling back to `kind`).
    pub fn kind(&self) -> Option<EventKind> {
        self.get_str("event_type")
            .or_else(|| self.get_str("kind"))
            .and_then(EventKind::from_wire)
    }

    /// Written-pair identity used for conflict comparison.
    pub fn identity(&self) -> String {
        dumps_pairs(&self.value)
    }

    /// Hash of the written-pair identity, for conflict comparison without
    /// retaining the full encoding. Equality matches [`RawEvent::identity`]
    /// up to SHA-256 collision resistance.
    pub fn identity_hash(&self) -> String {
        crate::digest::sha256_of(dumps_pairs(&self.value).as_bytes())
    }

    /// Grouping key: Python `str(event_id)` over the dict view.
    pub fn grouping_id(&self) -> String {
        py_str(self.get("event_id"))
    }

    /// Grouping key borrowing the stored text when the id is already a
    /// string (the common case), allocating only for coerced ids.
    pub fn grouping_key(&self) -> Cow<'_, str> {
        match self.get("event_id") {
            Some(PyValue::Str(text)) => Cow::Borrowed(text.as_str()),
            other => Cow::Owned(py_str(other)),
        }
    }

    /// Total-order key: `(str(emitted_at), str(event_id))`.
    pub fn order_key(&self) -> (String, String) {
        (py_str(self.get("emitted_at")), py_str(self.get("event_id")))
    }
}

/// Borrowed `str(value)` text when it needs no coercion: a missing value
/// reads as the empty string and a string reads as itself. Any other shape
/// returns `None`, meaning the caller must fall back to [`py_str`].
fn borrowed_py_str(value: Option<&PyValue>) -> Option<&str> {
    match value {
        None => Some(""),
        Some(PyValue::Str(text)) => Some(text.as_str()),
        _ => None,
    }
}

/// Compare two envelope fields as Python compares their `str()` forms,
/// without allocating when both sides borrow (the common string case).
fn cmp_py_str(a: Option<&PyValue>, b: Option<&PyValue>) -> Ordering {
    match (borrowed_py_str(a), borrowed_py_str(b)) {
        (Some(left), Some(right)) => left.cmp(right),
        _ => py_str(a).cmp(&py_str(b)),
    }
}

/// Total order over events: `(str(emitted_at), str(event_id))`, compared
/// without materialising owned keys for string fields.
pub fn order_cmp(a: &RawEvent, b: &RawEvent) -> Ordering {
    cmp_py_str(a.get("emitted_at"), b.get("emitted_at"))
        .then_with(|| cmp_py_str(a.get("event_id"), b.get("event_id")))
}

/// A read refusal. Refusals are disjoint from validation results: none of
/// them is ever a test red, and none is ever green.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The same `event_id` carries two different bodies.
    ConflictingEventId {
        /// The conflicting (stringified) event id.
        event_id: String,
    },
    /// A `run.annotate` event tried to change derived state.
    AnnotateChangesValue {
        /// The offending event id, when known.
        event_id: Option<String>,
        /// Why the annotate was refused.
        reason: String,
    },
    /// A stored line is not a readable event object.
    Malformed {
        /// Why the line was refused.
        reason: String,
    },
    /// Evidence failed its hash, length, or host check.
    EvidenceRefused {
        /// Why the evidence was refused.
        reason: String,
    },
    /// An event failed envelope validation.
    InvalidEvent {
        /// The event id, when known.
        event_id: Option<String>,
        /// Validation codes or messages, in check order.
        codes: Vec<String>,
    },
}

impl Refusal {
    /// Stable refusal class string, as used in reports.
    pub fn class(&self) -> &'static str {
        match self {
            Refusal::ConflictingEventId { .. } => "conflicting-event-id",
            Refusal::AnnotateChangesValue { .. } => "annotate-changes-value",
            Refusal::Malformed { .. } => "malformed",
            Refusal::EvidenceRefused { .. } => "evidence-refused",
            Refusal::InvalidEvent { .. } => "invalid-event",
        }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::ConflictingEventId { event_id } => {
                write!(
                    f,
                    "event_id {} appears twice with different bodies",
                    py_repr(&PyValue::Str(event_id.clone()))
                )
            }
            Refusal::AnnotateChangesValue { event_id, reason } => {
                write!(
                    f,
                    "{}: {} ({reason})",
                    self.class(),
                    event_id.as_deref().unwrap_or("?")
                )
            }
            Refusal::Malformed { reason } | Refusal::EvidenceRefused { reason } => {
                write!(f, "{}: {reason}", self.class())
            }
            Refusal::InvalidEvent { event_id, codes } => {
                write!(
                    f,
                    "validation event {} violates [{}]",
                    event_id.as_deref().unwrap_or("?"),
                    codes.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for Refusal {}

/// Fields a `run.annotate` event must never carry: anything that could move
/// outcome, commit, tree, selection, timing, or verdict, and any
/// value-changing reference.
const ANNOTATE_FORBIDDEN: [&str; 10] = [
    "outcome",
    "result",
    "commit",
    "tree",
    "selection_mode",
    "profile",
    "finished_at",
    "enriches",
    "supersedes",
    "corrects",
];

/// Tool named by a `producer` value: its `tool` field when it is an
/// object, the string itself when it is one, otherwise empty.
pub fn producer_tool(value: Option<&PyValue>) -> &str {
    match value {
        Some(object @ PyValue::Object(_)) => {
            object.get("tool").and_then(PyValue::as_str).unwrap_or("")
        }
        Some(PyValue::Str(text)) => text,
        _ => "",
    }
}

/// Whether an event was produced by a fixture tool, by its own producer
/// or by its folded row's producer. Fixture producer names are deployment
/// policy data, never hardcoded here; an empty set retains everything.
/// This mirrors how an admission history excludes acceptance fixtures
/// after the union and before envelope checks and the fold.
pub fn is_fixture_event(
    event: &RawEvent,
    fixture_producers: &std::collections::BTreeSet<String>,
) -> bool {
    if fixture_producers.is_empty() {
        return false;
    }
    fixture_producers.contains(producer_tool(event.get("producer")))
        || fixture_producers.contains(producer_tool(
            event.get("legacy_row").and_then(|row| row.get("producer")),
        ))
}

/// Validate one annotate event as metadata-only.
fn check_annotate(event: &RawEvent) -> Result<(), Refusal> {
    for field in ANNOTATE_FORBIDDEN {
        if event.value.contains_key(field) {
            return Err(Refusal::AnnotateChangesValue {
                event_id: event.get_str("event_id").map(ToOwned::to_owned),
                reason: format!("annotate carries {field}"),
            });
        }
    }
    Ok(())
}

/// Validate one annotate event as metadata-only (the check [`union`]
/// applies to every copy, exposed for streaming folds).
pub fn check_annotate_event(event: &RawEvent) -> Result<(), Refusal> {
    check_annotate(event)
}

/// Parse one ordinary stored line into a [`RawEvent`].
///
/// Stored codec lines (carrying `legacy_row_codec`) must go through
/// expansion first; this entry parses the already-expanded text.
pub fn parse_line(line: &str) -> Result<RawEvent, Refusal> {
    let trimmed = line.trim();
    let value = parse(trimmed).map_err(|e| Refusal::Malformed {
        reason: format!("line is not valid JSON: {e}"),
    })?;
    if !value.is_object() {
        return Err(Refusal::Malformed {
            reason: "line is not a JSON object".to_owned(),
        });
    }
    Ok(RawEvent {
        line: trimmed.to_owned(),
        value,
    })
}

/// Collapse identical copies and refuse conflicting bodies.
///
/// Input order is scan order (shard path, then line); the first conflict in
/// that order is reported, naming the stringified grouping id. Each later
/// copy is compared against the first copy kept for its id. The result is
/// in total order `(str(emitted_at), str(event_id))`.
pub fn union(events: Vec<RawEvent>) -> Result<Vec<RawEvent>, Refusal> {
    // Only identity hashes are retained (one per distinct id); the full
    // written-pair encoding of each event is freed with the event.
    let mut first_identity: HashMap<String, String> = HashMap::new();
    let mut kept: Vec<RawEvent> = Vec::new();
    for event in events {
        if event.kind() == Some(EventKind::RunAnnotate) {
            check_annotate(&event)?;
        }
        let grouping = event.grouping_key();
        match first_identity.get(grouping.as_ref()) {
            None => {
                first_identity.insert(grouping.into_owned(), event.identity_hash());
                kept.push(event);
            }
            Some(first) => {
                if *first != event.identity_hash() {
                    return Err(Refusal::ConflictingEventId {
                        event_id: grouping.into_owned(),
                    });
                }
            }
        }
    }
    kept.sort_by(order_cmp);
    Ok(kept)
}

/// Whether text contains an owner path (`/home/`) in any string value at
/// any depth (keys are not inspected).
fn contains_owner_path(value: &PyValue) -> bool {
    match value {
        PyValue::Str(s) => s.contains("/home/"),
        PyValue::Array(items) => items.iter().any(contains_owner_path),
        PyValue::Object(pairs) => pairs.iter().any(|(_, v)| contains_owner_path(v)),
        _ => false,
    }
}

/// Whether `text` is exactly 40 lowercase hex characters.
pub fn is_lower_hex40(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Strict RFC 3339 UTC check: `YYYY-MM-DDTHH:MM:SS[.frac]Z` with valid
/// calendar ranges. A `+00:00` offset does not pass; the trailing `Z` is
/// required.
pub fn is_rfc3339_utc(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || !text.ends_with('Z') {
        return false;
    }
    let parts =
        |range: std::ops::Range<usize>| -> Option<u32> { text.get(range)?.parse::<u32>().ok() };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parts(0..4),
        parts(5..7),
        parts(8..10),
        parts(11..13),
        parts(14..16),
        parts(17..19),
    ) else {
        return false;
    };
    if text.get(4..5) != Some("-")
        || text.get(7..8) != Some("-")
        || text.get(10..11) != Some("T")
        || text.get(13..14) != Some(":")
        || text.get(16..17) != Some(":")
    {
        return false;
    }
    let tail = &text[19..text.len() - 1];
    if !tail.is_empty()
        && !(tail.starts_with('.')
            && tail.len() > 1
            && tail[1..].bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    if !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 60 {
        return false;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return false,
    };
    day >= 1 && day <= days_in_month
}

fn non_blank(event: &RawEvent, field: &str) -> Option<String> {
    match event.get_str(field) {
        Some(text) if !text.trim().is_empty() => Some(text.to_owned()),
        _ => None,
    }
}

/// Validate one event's envelope, returning its codes in check order.
///
/// An empty vector means the event is accepted. Checks are: contextual
/// codes (schema version, dotted host, raw `cwd`, owner path), then the
/// closed envelope rules (required fields, path segments, timestamp,
/// commit shape, per-kind references).
pub fn event_codes(event: &RawEvent) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    if event.get_str("schema") != Some(SCHEMA) {
        codes.push("unknown-envelope-version".to_owned());
    }
    if py_str(event.get("host")).contains('.') {
        codes.push("fqdn-host".to_owned());
    }
    if event.value.contains_key("cwd") {
        codes.push("raw-cwd-forbidden".to_owned());
    }
    if contains_owner_path(&event.value) {
        codes.push("owner-path".to_owned());
    }

    // Closed envelope rules. A missing or wrongly typed required field is
    // reported with the same texts the shared checker uses.
    let kind_raw = event.get("event_type").or_else(|| event.get("kind"));
    let kind = kind_raw
        .and_then(PyValue::as_str)
        .and_then(EventKind::from_wire);
    if kind_raw.is_some() && kind.is_none() {
        codes.push("validate-ledger/v1 event_type is invalid".to_owned());
    }
    for field in [
        "event_id",
        "event_type",
        "emitted_at",
        "team",
        "host",
        "run_id",
    ] {
        if event.get(field).is_none() {
            codes.push(format!("validation ledger event omitted {field}"));
        }
    }
    let producer = event.get("producer");
    let producer_ok = matches!(producer, Some(PyValue::Object(_)));
    if !producer_ok {
        codes.push("validation ledger event omitted producer".to_owned());
    } else if let Some(producer) = producer {
        match producer.get("source").and_then(PyValue::as_str) {
            Some("observed" | "reconstructed" | "imported") => {}
            _ => codes.push("validation ledger event producer source is invalid".to_owned()),
        }
        if producer.get("tool").and_then(PyValue::as_str).is_none() {
            codes.push("validation ledger event omitted producer.tool".to_owned());
        }
        if producer
            .get("tool_version")
            .and_then(PyValue::as_str)
            .is_none()
        {
            codes.push("validation ledger event omitted producer.tool_version".to_owned());
        }
    }
    for field in ["event_id", "emitted_at", "team", "host", "run_id"] {
        if let Some(text) = event.get_str(field) {
            if text.trim().is_empty() {
                codes.push(format!("validation ledger event {field} is empty"));
            }
        }
    }
    if let Some(team) = event.get_str("team") {
        if team.contains('/') || team == "." || team == ".." {
            codes.push("validation ledger event team is not one path segment".to_owned());
        }
    }
    if let Some(host) = event.get_str("host") {
        if host.contains('.') || host.contains('/') || host == "." || host == ".." {
            codes.push("validation ledger event host is not a short path segment".to_owned());
        }
    }
    if let Some(emitted) = event.get_str("emitted_at") {
        if !is_rfc3339_utc(emitted) {
            codes.push("validation ledger event emitted_at is not RFC3339 UTC".to_owned());
        }
    }
    if let Some(commit) = event.get("commit") {
        if !matches!(commit, PyValue::Null) {
            match commit.as_str() {
                Some(text) if is_lower_hex40(text) => {}
                _ => codes.push(
                    "validation ledger event commit is not 40 lowercase hexadecimal characters"
                        .to_owned(),
                ),
            }
        }
    }
    let has = |field: &str| matches!(event.get(field), Some(v) if !matches!(v, PyValue::Null));
    match kind {
        Some(EventKind::RunStart) => {
            if event.get_str("run_id") != event.get_str("event_id") {
                codes.push(
                    "validation ledger run.start must mint run_id equal to event_id".to_owned(),
                );
            }
            if has("enriches") || has("supersedes") || has("reason") {
                codes
                    .push("validation ledger run.start carries a later-event reference".to_owned());
            }
        }
        Some(EventKind::RunResult) => {
            if !has("commit") {
                codes.push("validation ledger run.result omitted commit".to_owned());
            }
            match event.get("outcome") {
                Some(PyValue::Str(name)) if Outcome::from_wire(name).is_some() => {}
                Some(PyValue::Null) | None => {
                    codes.push("validation ledger run.result omitted outcome".to_owned())
                }
                _ => codes.push("validation ledger run.result outcome is invalid".to_owned()),
            }
            if has("enriches") || has("supersedes") || has("reason") {
                codes.push(
                    "validation ledger run.result carries a later-event reference".to_owned(),
                );
            }
        }
        Some(EventKind::RunEnrich) => {
            if non_blank(event, "enriches").is_none() {
                codes.push("validation ledger run.enrich omitted enriches".to_owned());
            }
            if has("supersedes") || has("reason") {
                codes.push("validation ledger run.enrich carries correction fields".to_owned());
            }
        }
        Some(EventKind::RunCorrect) => {
            if non_blank(event, "supersedes").is_none() {
                codes.push("validation ledger run.correct omitted supersedes".to_owned());
            }
            if non_blank(event, "reason").is_none() {
                codes.push("validation ledger run.correct omitted reason".to_owned());
            }
            if has("enriches") {
                codes.push("validation ledger run.correct carries enriches".to_owned());
            }
        }
        Some(EventKind::RunAnnotate) if has("enriches") || has("supersedes") => {
            codes.push(
                "validation ledger run.annotate carries a value-changing reference".to_owned(),
            );
        }
        Some(EventKind::RunAnnotate) => {}
        None => {}
    }
    codes
}

/// Validate every event in union order; the first failure refuses the read.
pub fn check_events(events: &[RawEvent]) -> Result<(), Refusal> {
    for event in events {
        let codes = event_codes(event);
        if !codes.is_empty() {
            return Err(Refusal::InvalidEvent {
                event_id: event.get_str("event_id").map(ToOwned::to_owned),
                codes,
            });
        }
    }
    Ok(())
}
