use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Read as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use serde::de::IgnoredAny;
use serde_json::{Map, Value};

use crate::canonical::{canonical_json, canonical_sha256};
use crate::schema::{
    parse_timestamp, validate_active_record, validate_archive_record, ActiveRecordMeta,
};
use crate::ObserverError;

const EVENT_SCHEMA: u64 = 1;
const STATE_SCHEMA: u64 = 2;
const ZERO_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";
// State-import events contain full snapshots, so the bound leaves substantial
// room above ordinary events while guaranteeing one corrupt file cannot force
// unbounded allocation. Event rows themselves are streamed into SQLite.
pub(crate) const MAX_EVENT_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_JSON_CONTAINER_DEPTH: usize = 127;

const NON_STATE_EVENT_KINDS: &[&str] = &[
    "handoff-read",
    "handoff-removed",
    "handoff-write-completed",
    "handoff-write-intended",
    "handoff-written",
    "legacy-validate-checkout-removed",
    "operation-completed",
    "operation-progress-recorded",
    "ownerless-agent-cache-relocated",
    "ownerless-agent-worktree-removed",
    "ownerless-validate-path-removed",
    "partial-updates-recovered",
    "reclaim-started",
    "recovery-started",
    "retirement-attempted",
    "slot-held",
    "slot-hold-released",
];

/// The replay tip and materialized active/archive counters for one machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaySummary {
    /// Machine shard encoded by the `EVENTS.<machine>` directory name.
    pub machine: String,
    /// Number of validated events in the hash chain.
    pub replay_count: u64,
    /// SHA-256 digest of the final validated event.
    pub tip_sha256: String,
    /// Revision reached by replaying active-state events.
    pub active_revision: u64,
    /// Number of active records after replay.
    pub active_count: u64,
    /// Revision reached by replaying archive-state events.
    pub archive_revision: u64,
    /// Number of archived records after replay.
    pub archive_count: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub(crate) machine: String,
    pub(crate) sequence: u64,
    pub(crate) previous_sha256: String,
    pub(crate) recorded_at: String,
    pub(crate) kind: String,
    pub(crate) payload: Value,
    pub(crate) sha256: String,
}

#[derive(Debug)]
pub(crate) struct ReplayedLog {
    pub(crate) summary: ReplaySummary,
    pub(crate) active_records: BTreeMap<String, Value>,
    pub(crate) archive_records: Vec<Value>,
}

#[derive(Debug)]
struct ActiveEntry {
    value: Value,
    meta: ActiveRecordMeta,
}

#[derive(Default)]
struct State {
    active_revision: Option<u64>,
    active_records: BTreeMap<String, ActiveEntry>,
    active_agents: BTreeMap<String, String>,
    archive_revision: Option<u64>,
    archive_records: Vec<Value>,
    archive_ids: BTreeSet<String>,
    archived_slots: BTreeSet<String>,
}

pub(crate) struct ReplayGuard<'a> {
    pub(crate) replay_count: u64,
    pub(crate) tip_sha256: &'a str,
}

/// Validate and replay one `EVENTS.<machine>` directory without changing it.
pub fn replay(events_dir: &Path) -> Result<ReplaySummary, ObserverError> {
    replay_stream(events_dir, None, |_| Ok(())).map(|replayed| replayed.summary)
}

pub(crate) fn replay_stream(
    events_dir: &Path,
    guard: Option<ReplayGuard<'_>>,
    consume: impl FnMut(&Event) -> Result<(), ObserverError>,
) -> Result<ReplayedLog, ObserverError> {
    replay_stream_after_count(events_dir, guard, || {}, consume)
}

#[cfg(test)]
pub(crate) fn replay_with_post_count_hook(
    events_dir: &Path,
    after_count: impl FnOnce(),
) -> Result<ReplaySummary, ObserverError> {
    replay_stream_after_count(events_dir, None, after_count, |_| Ok(()))
        .map(|replayed| replayed.summary)
}

fn replay_stream_after_count(
    events_dir: &Path,
    guard: Option<ReplayGuard<'_>>,
    after_count: impl FnOnce(),
    mut consume: impl FnMut(&Event) -> Result<(), ObserverError>,
) -> Result<ReplayedLog, ObserverError> {
    let machine = machine_from_directory(events_dir)?;
    let event_count = event_count(events_dir)?;
    if event_count == 0 {
        return Err(ObserverError::invalid(format!(
            "event log {} is empty and has no imported state",
            events_dir.display()
        )));
    }
    if let Some(guard) = &guard {
        if event_count < guard.replay_count {
            return Err(ObserverError::invalid(format!(
                "refusing to replace a {0}-event index with an older {event_count}-event replay",
                guard.replay_count
            )));
        }
    }
    after_count();

    let mut previous = ZERO_DIGEST.to_owned();
    let mut state = State::default();
    for expected_sequence in 1..=event_count {
        let path = events_dir.join(format!("{expected_sequence:020}.json"));
        let event = load_event(&path, &machine, expected_sequence, &previous)?;
        if guard
            .as_ref()
            .is_some_and(|guard| guard.replay_count == expected_sequence)
            && guard
                .as_ref()
                .is_some_and(|guard| guard.tip_sha256 != event.sha256)
        {
            return Err(ObserverError::invalid(
                "event replay does not extend the indexed hash-chain tip",
            ));
        }
        apply_event(&event, &mut state)?;
        consume(&event)?;
        previous.clone_from(&event.sha256);
    }

    let active_revision = state
        .active_revision
        .ok_or_else(|| ObserverError::invalid("event log has no complete imported active state"))?;
    let archive_revision = state.archive_revision.ok_or_else(|| {
        ObserverError::invalid("event log has no complete imported archive state")
    })?;
    let active_count = u64::try_from(state.active_records.len())
        .map_err(|_| ObserverError::invalid("active record count does not fit in u64"))?;
    let archive_count = u64::try_from(state.archive_records.len())
        .map_err(|_| ObserverError::invalid("archive record count does not fit in u64"))?;
    Ok(ReplayedLog {
        summary: ReplaySummary {
            machine,
            replay_count: event_count,
            tip_sha256: previous,
            active_revision,
            active_count,
            archive_revision,
            archive_count,
        },
        active_records: state
            .active_records
            .into_iter()
            .map(|(slot, entry)| (slot, entry.value))
            .collect(),
        archive_records: state.archive_records,
    })
}

fn machine_from_directory(events_dir: &Path) -> Result<String, ObserverError> {
    let metadata = fs::symlink_metadata(events_dir).map_err(|error| {
        ObserverError::with_source(
            format!("cannot inspect event directory {}", events_dir.display()),
            error,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ObserverError::invalid(format!(
            "event log is not a real directory: {}",
            events_dir.display()
        )));
    }
    let name = events_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ObserverError::invalid("event directory name is not UTF-8"))?;
    let machine = name.strip_prefix("EVENTS.").ok_or_else(|| {
        ObserverError::invalid(format!(
            "event directory must be named EVENTS.<machine>: {}",
            events_dir.display()
        ))
    })?;
    validate_name(machine, "machine")?;
    Ok(machine.to_owned())
}

fn event_count(events_dir: &Path) -> Result<u64, ObserverError> {
    let entries = fs::read_dir(events_dir).map_err(|error| {
        ObserverError::with_source(
            format!("cannot read event directory {}", events_dir.display()),
            error,
        )
    })?;
    let mut complete = 0_u64;
    let mut minimum = u64::MAX;
    let mut maximum = 0_u64;
    for entry in entries {
        let entry = entry.map_err(|error| {
            ObserverError::with_source(
                format!("cannot read an entry in {}", events_dir.display()),
                error,
            )
        })?;
        let name = entry.file_name().into_string().map_err(|_| {
            ObserverError::invalid(format!(
                "event log contains a non-UTF-8 entry in {}",
                events_dir.display()
            ))
        })?;
        if is_event_filename(&name) {
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                ObserverError::with_source(
                    format!("cannot inspect event file {}", entry.path().display()),
                    error,
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ObserverError::invalid(format!(
                    "event is not a real file: {}",
                    entry.path().display()
                )));
            }
            let sequence = name[..20].parse::<u64>().map_err(|_| {
                ObserverError::invalid(format!(
                    "event filename has an out-of-range sequence: {}",
                    entry.path().display()
                ))
            })?;
            complete = complete
                .checked_add(1)
                .ok_or_else(|| ObserverError::invalid("event file count overflow"))?;
            minimum = minimum.min(sequence);
            maximum = maximum.max(sequence);
        } else if !is_temporary_event_filename(&name) {
            return Err(ObserverError::invalid(format!(
                "event log contains an unexpected entry: {}",
                entry.path().display()
            )));
        }
    }
    if complete != 0 && (minimum != 1 || maximum != complete) {
        return Err(ObserverError::invalid(format!(
            "event log has a sequence gap in {}",
            events_dir.display()
        )));
    }
    Ok(complete)
}

fn is_event_filename(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 25
        && bytes[..20].iter().all(u8::is_ascii_digit)
        && bytes.get(20..) == Some(b".json")
}

fn is_temporary_event_filename(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 30
        && bytes[..20].iter().all(u8::is_ascii_digit)
        && bytes.get(20..30) == Some(b".json.tmp.")
}

fn load_event(
    path: &Path,
    machine: &str,
    expected_sequence: u64,
    previous: &str,
) -> Result<Event, ObserverError> {
    let expected_name = format!("{expected_sequence:020}.json");
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(ObserverError::invalid(format!(
            "event log has a sequence gap at {}",
            path.display()
        )));
    }
    let file = OpenOptions::new()
        .read(true)
        // A file that was regular during directory enumeration can be
        // replaced by a FIFO before this open.  O_NONBLOCK makes that race
        // fail closed in the post-open regular-file check instead of hanging.
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| {
            ObserverError::with_source(format!("cannot open event {}", path.display()), error)
        })?;
    if !file
        .metadata()
        .map_err(|error| {
            ObserverError::with_source(format!("cannot inspect event {}", path.display()), error)
        })?
        .is_file()
    {
        return Err(ObserverError::invalid(format!(
            "event is not a regular file: {}",
            path.display()
        )));
    }
    let mut contents = Vec::new();
    file.take(MAX_EVENT_BYTES + 1)
        .read_to_end(&mut contents)
        .map_err(|error| {
            ObserverError::with_source(format!("cannot read event {}", path.display()), error)
        })?;
    if u64::try_from(contents.len()).unwrap_or(u64::MAX) > MAX_EVENT_BYTES {
        return Err(ObserverError::invalid(format!(
            "event exceeds the {MAX_EVENT_BYTES}-byte replay limit: {}",
            path.display()
        )));
    }
    validate_and_normalize_json_domain(&mut contents, path)?;
    let value: Value = serde_json::from_slice(&contents).map_err(|error| {
        ObserverError::with_source(format!("cannot parse event {}", path.display()), error)
    })?;
    let object = value.as_object().ok_or_else(|| {
        ObserverError::invalid(format!("event {} must be a JSON object", path.display()))
    })?;
    exact_keys(
        object,
        &[
            "schema",
            "machine",
            "sequence",
            "previous_sha256",
            "recorded_at",
            "kind",
            "payload",
            "sha256",
        ],
        "event envelope",
    )?;
    if unsigned(&object["schema"], "event.schema")? != EVENT_SCHEMA {
        return Err(ObserverError::invalid(format!(
            "unsupported event schema in {}",
            path.display()
        )));
    }
    if string(&object["machine"], "event.machine")? != machine {
        return Err(ObserverError::invalid(format!(
            "event belongs to another machine: {}",
            path.display()
        )));
    }
    let sequence = unsigned(&object["sequence"], "event.sequence")?;
    if sequence != expected_sequence {
        return Err(ObserverError::invalid(format!(
            "event sequence does not match its filename: {}",
            path.display()
        )));
    }
    let recorded_previous = string(&object["previous_sha256"], "event.previous_sha256")?;
    if recorded_previous != previous {
        return Err(ObserverError::invalid(format!(
            "event hash chain is broken at {}",
            path.display()
        )));
    }
    let recorded_at = string(&object["recorded_at"], "event.recorded_at")?;
    parse_timestamp(recorded_at, "event.recorded_at")?;
    let kind = string(&object["kind"], "event.kind")?;
    if kind.is_empty() {
        return Err(ObserverError::invalid("event.kind must not be empty"));
    }
    if !object["payload"].is_object() {
        return Err(ObserverError::invalid(
            "event.payload must be a JSON object",
        ));
    }
    let recorded_digest = string(&object["sha256"], "event.sha256")?;
    let mut core = object.clone();
    core.remove("sha256");
    let computed_digest = canonical_sha256(&Value::Object(core))?;
    if recorded_digest != computed_digest {
        return Err(ObserverError::invalid(format!(
            "event digest does not match its content: {}",
            path.display()
        )));
    }
    Ok(Event {
        machine: machine.to_owned(),
        sequence,
        previous_sha256: recorded_previous.to_owned(),
        recorded_at: recorded_at.to_owned(),
        kind: kind.to_owned(),
        payload: object["payload"].clone(),
        sha256: recorded_digest.to_owned(),
    })
}

/// Enforce the JSON subset that can be represented and walked safely before
/// decoding. The Python authority accepts a wider language, but current event
/// schemas use shallow structures and non-negative integers. The observer
/// additionally accepts up to [`MAX_JSON_CONTAINER_DEPTH`] nested containers,
/// exact signed/unsigned 64-bit integers, and finite binary64 floats, whose
/// canonical spelling is handled by `canonical::python_number`.
///
/// Python parses the integer spelling `-0` as integer zero, while `serde_json`
/// classifies it as negative floating zero. Replacing its minus sign with JSON
/// whitespace before decoding preserves the authoritative value without
/// changing the event file. Malformed number candidates are left untouched so
/// the JSON parser, rather than this domain check, classifies their grammar.
fn validate_and_normalize_json_domain(
    contents: &mut [u8],
    path: &Path,
) -> Result<(), ObserverError> {
    validate_json_structure_domain(contents, path)?;
    serde_json::from_slice::<IgnoredAny>(contents).map_err(|error| {
        ObserverError::with_source(format!("cannot parse event {}", path.display()), error)
    })?;

    normalize_json_numbers(contents, path)
}

fn validate_json_structure_domain(contents: &[u8], path: &Path) -> Result<(), ObserverError> {
    let mut offset = 0;
    let mut depth = 0_usize;
    while offset < contents.len() {
        match contents[offset] {
            b'"' => {
                offset += 1;
                while offset < contents.len() {
                    match contents[offset] {
                        b'\\' if contents.get(offset + 1) == Some(&b'u') => {
                            let Some(unit) = unicode_escape(contents, offset) else {
                                offset = (offset + 2).min(contents.len());
                                continue;
                            };
                            if (0xd800..=0xdbff).contains(&unit) {
                                let second = offset + 6;
                                if unicode_escape(contents, second)
                                    .is_some_and(|unit| (0xdc00..=0xdfff).contains(&unit))
                                {
                                    offset += 12;
                                    continue;
                                }
                                return Err(ObserverError::invalid(format!(
                                    "event contains a lone Unicode surrogate escape at byte {offset}: {}",
                                    path.display()
                                )));
                            }
                            if (0xdc00..=0xdfff).contains(&unit) {
                                return Err(ObserverError::invalid(format!(
                                    "event contains a lone Unicode surrogate escape at byte {offset}: {}",
                                    path.display()
                                )));
                            }
                            offset += 6;
                        }
                        b'\\' => offset = (offset + 2).min(contents.len()),
                        b'"' => {
                            offset += 1;
                            break;
                        }
                        _ => offset += 1,
                    }
                }
            }
            b'{' | b'[' => {
                depth += 1;
                if depth > MAX_JSON_CONTAINER_DEPTH {
                    return Err(ObserverError::invalid(format!(
                        "event exceeds the {MAX_JSON_CONTAINER_DEPTH}-container JSON nesting limit: {}",
                        path.display()
                    )));
                }
                offset += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                offset += 1;
            }
            _ => offset += 1,
        }
    }
    Ok(())
}

fn normalize_json_numbers(contents: &mut [u8], path: &Path) -> Result<(), ObserverError> {
    let mut offset = 0;
    while offset < contents.len() {
        match contents[offset] {
            b'"' => {
                offset += 1;
                while offset < contents.len() {
                    match contents[offset] {
                        b'\\' => offset = (offset + 2).min(contents.len()),
                        b'"' => {
                            offset += 1;
                            break;
                        }
                        _ => offset += 1,
                    }
                }
            }
            byte @ (b'-' | b'0'..=b'9')
                if byte != b'-' || contents.get(offset + 1).is_some_and(u8::is_ascii_digit) =>
            {
                let start = offset;
                offset += 1;
                while offset < contents.len()
                    && matches!(
                        contents[offset],
                        b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'
                    )
                {
                    offset += 1;
                }
                let token = std::str::from_utf8(&contents[start..offset]).map_err(|error| {
                    ObserverError::with_source(
                        format!(
                            "event contains an invalid number at byte {start}: {}",
                            path.display()
                        ),
                        error,
                    )
                })?;
                if !is_json_number(token) {
                    continue;
                }
                if token.contains(['.', 'e', 'E']) {
                    if !token.parse::<f64>().is_ok_and(f64::is_finite) {
                        return Err(ObserverError::invalid(format!(
                            "event contains a non-finite number at byte {start}: {}",
                            path.display()
                        )));
                    }
                } else {
                    let exact = if token.starts_with('-') {
                        token.parse::<i64>().is_ok()
                    } else {
                        token.parse::<u64>().is_ok()
                    };
                    if !exact {
                        return Err(ObserverError::invalid(format!(
                            "event contains an integer outside the lossless observer domain at byte {start}: {}",
                            path.display()
                        )));
                    }
                    if token == "-0" {
                        contents[start] = b' ';
                    }
                }
            }
            _ => offset += 1,
        }
    }
    Ok(())
}

fn is_json_number(token: &str) -> bool {
    let bytes = token.as_bytes();
    let mut offset = usize::from(bytes.first() == Some(&b'-'));
    match bytes.get(offset) {
        Some(b'0') => offset += 1,
        Some(b'1'..=b'9') => {
            offset += 1;
            while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
                offset += 1;
            }
        }
        _ => return false,
    }
    if bytes.get(offset) == Some(&b'.') {
        offset += 1;
        let fraction_start = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if offset == fraction_start {
            return false;
        }
    }
    if matches!(bytes.get(offset), Some(b'e' | b'E')) {
        offset += 1;
        if matches!(bytes.get(offset), Some(b'+' | b'-')) {
            offset += 1;
        }
        let exponent_start = offset;
        while bytes.get(offset).is_some_and(u8::is_ascii_digit) {
            offset += 1;
        }
        if offset == exponent_start {
            return false;
        }
    }
    offset == bytes.len()
}

fn unicode_escape(contents: &[u8], offset: usize) -> Option<u16> {
    if contents.get(offset..offset + 2)? != b"\\u" {
        return None;
    }
    let digits = contents.get(offset + 2..offset + 6)?;
    digits.iter().try_fold(0_u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => u16::from(byte - b'0'),
            b'a'..=b'f' => u16::from(byte - b'a') + 10,
            b'A'..=b'F' => u16::from(byte - b'A') + 10,
            _ => return None,
        };
        Some(value * 16 + digit)
    })
}

fn apply_event(event: &Event, state: &mut State) -> Result<(), ObserverError> {
    match event.kind.as_str() {
        "state-imported" => apply_import(event, state),
        "active-state-recorded" => apply_active(event, state),
        "archive-state-recorded" => apply_archive(event, state),
        kind if NON_STATE_EVENT_KINDS.contains(&kind) => Ok(()),
        kind => Err(ObserverError::invalid(format!(
            "unknown event kind {kind:?} at sequence {}",
            event.sequence
        ))),
    }
}

fn apply_import(event: &Event, state: &mut State) -> Result<(), ObserverError> {
    if state.active_revision.is_some() || state.archive_revision.is_some() {
        return Err(ObserverError::invalid(
            "event log imports state more than once",
        ));
    }
    let payload = object(&event.payload, "state-imported payload")?;
    exact_keys(
        payload,
        &["active", "archive", "holds"],
        "state-imported payload",
    )?;
    if !payload["holds"].is_array() {
        return Err(ObserverError::invalid(
            "state-imported payload.holds must be a JSON array",
        ));
    }

    let active = object(&payload["active"], "imported active state")?;
    exact_keys(
        active,
        &["schema", "machine", "revision", "slots"],
        "imported active state",
    )?;
    validate_state_header(active, &event.machine, "imported active state")?;
    let slots = array(&active["slots"], "imported active state.slots")?;
    for record in slots {
        let validated = validate_active_record(record, &event.machine, "imported active record")?;
        let meta = validated.meta;
        let slot = meta.slot.clone();
        if meta.has_unique_agent() {
            if let Some(existing) = state.active_agents.get(&meta.agent) {
                return Err(ObserverError::invalid(format!(
                    "agent {} owns both active slots {existing} and {slot}",
                    meta.agent
                )));
            }
            state.active_agents.insert(meta.agent.clone(), slot.clone());
        }
        if state
            .active_records
            .insert(
                slot.clone(),
                ActiveEntry {
                    value: validated.normalized,
                    meta,
                },
            )
            .is_some()
        {
            return Err(ObserverError::invalid(format!(
                "duplicate active slot in imported state: {slot}"
            )));
        }
    }
    state.active_revision = Some(unsigned(
        &active["revision"],
        "imported active state.revision",
    )?);

    let archive = object(&payload["archive"], "imported archive state")?;
    exact_keys(
        archive,
        &["schema", "machine", "revision", "records"],
        "imported archive state",
    )?;
    validate_state_header(archive, &event.machine, "imported archive state")?;
    let records = array(&archive["records"], "imported archive state.records")?;
    for record in records {
        let meta = validate_archive_record(record, &event.machine, "imported archive record")?;
        insert_archive_record(record, meta, state)?;
    }
    state.archive_revision = Some(unsigned(
        &archive["revision"],
        "imported archive state.revision",
    )?);
    Ok(())
}

fn apply_active(event: &Event, state: &mut State) -> Result<(), ObserverError> {
    let current_revision = state
        .active_revision
        .ok_or_else(|| ObserverError::invalid("event records active state before its import"))?;
    let payload = object(&event.payload, "active-state-recorded payload")?;
    exact_keys(
        payload,
        &[
            "action",
            "slot",
            "previous_revision",
            "revision",
            "previous_record_sha256",
            "record",
            "evidence",
        ],
        "active-state-recorded payload",
    )?;
    let action = string(&payload["action"], "active-state-recorded action")?;
    if action.is_empty() {
        return Err(ObserverError::invalid(
            "active-state-recorded action must not be empty",
        ));
    }
    let slot = string(&payload["slot"], "active-state-recorded slot")?;
    validate_name(slot, "active-state-recorded slot")?;
    continued_revision(
        current_revision,
        &payload["previous_revision"],
        &payload["revision"],
        "active",
    )?;
    if !payload["evidence"].is_object() {
        return Err(ObserverError::invalid(
            "active-state-recorded evidence must be a JSON object",
        ));
    }

    let current = state.active_records.get(slot);
    let current_meta = current.map(|entry| entry.meta.clone());
    let expected_previous = current
        .map(|entry| canonical_sha256(&entry.value))
        .transpose()?;
    match (&payload["previous_record_sha256"], &expected_previous) {
        (Value::Null, None) => {}
        (Value::String(recorded), Some(expected)) if recorded == expected => {}
        _ => {
            return Err(ObserverError::invalid(format!(
                "active-state-recorded previous record does not match slot {slot}"
            )))
        }
    }

    if action == "absent-validation-row-recovered" {
        let current = current_meta.as_ref().ok_or_else(|| {
            ObserverError::invalid("absent-validation-row recovery must remove one active row")
        })?;
        if !payload["record"].is_null() {
            return Err(ObserverError::invalid(
                "absent-validation-row recovery must remove one active row",
            ));
        }
        let evidence = validation_recovery_evidence(
            &payload["evidence"],
            "active validation recovery evidence",
        )?;
        if Some(evidence.source_record_sha256.as_str()) != expected_previous.as_deref()
            || !evidence
                .archive_id
                .starts_with(&format!("{}:{slot}:{}:", event.machine, current.generation))
        {
            return Err(ObserverError::invalid(
                "active validation recovery evidence does not match its source record",
            ));
        }
    } else if action == "absent-agent-row-recovered" {
        let current = current_meta.as_ref().ok_or_else(|| {
            ObserverError::invalid("absent-agent-row recovery must remove one active agent row")
        })?;
        if current.slot_type != "agent" || !payload["record"].is_null() {
            return Err(ObserverError::invalid(
                "absent-agent-row recovery must remove one active agent row",
            ));
        }
        let evidence =
            agent_recovery_evidence(&payload["evidence"], "active agent recovery evidence")?;
        if Some(evidence.source_record_sha256.as_str()) != expected_previous.as_deref()
            || !evidence
                .archive_id
                .starts_with(&format!("{}:{slot}:{}:", event.machine, current.generation))
        {
            return Err(ObserverError::invalid(
                "active agent recovery evidence does not match its source record",
            ));
        }
    }

    let next = if payload["record"].is_null() {
        if current_meta.is_none() {
            return Err(ObserverError::invalid(format!(
                "active-state-recorded removes absent slot {slot}"
            )));
        }
        None
    } else {
        let validated = validate_active_record(
            &payload["record"],
            &event.machine,
            "active-state-recorded record",
        )?;
        let meta = validated.meta;
        if meta.slot != slot {
            return Err(ObserverError::invalid(format!(
                "active-state-recorded record does not match slot {slot}"
            )));
        }
        if meta.has_unique_agent()
            && state
                .active_agents
                .get(&meta.agent)
                .is_some_and(|existing| existing != slot)
        {
            return Err(ObserverError::invalid(format!(
                "agent {} already owns another active slot",
                meta.agent
            )));
        }
        Some(ActiveEntry {
            value: validated.normalized,
            meta,
        })
    };
    if let Some(current) = state.active_records.remove(slot) {
        if current.meta.has_unique_agent()
            && state
                .active_agents
                .get(&current.meta.agent)
                .map(String::as_str)
                == Some(slot)
        {
            state.active_agents.remove(&current.meta.agent);
        }
    }
    if let Some(next) = next {
        if next.meta.has_unique_agent() {
            state
                .active_agents
                .insert(next.meta.agent.clone(), slot.to_owned());
        }
        state.active_records.insert(slot.to_owned(), next);
    }
    state.active_revision = Some(current_revision + 1);
    Ok(())
}

fn apply_archive(event: &Event, state: &mut State) -> Result<(), ObserverError> {
    let current_revision = state
        .archive_revision
        .ok_or_else(|| ObserverError::invalid("event records archive state before its import"))?;
    let payload = object(&event.payload, "archive-state-recorded payload")?;
    exact_keys(
        payload,
        &[
            "action",
            "slot",
            "previous_revision",
            "revision",
            "record",
            "evidence",
        ],
        "archive-state-recorded payload",
    )?;
    let action = string(&payload["action"], "archive-state-recorded action")?;
    if action.is_empty() {
        return Err(ObserverError::invalid(
            "archive-state-recorded action must not be empty",
        ));
    }
    let slot = string(&payload["slot"], "archive-state-recorded slot")?;
    validate_name(slot, "archive-state-recorded slot")?;
    continued_revision(
        current_revision,
        &payload["previous_revision"],
        &payload["revision"],
        "archive",
    )?;
    if !payload["evidence"].is_object() {
        return Err(ObserverError::invalid(
            "archive-state-recorded evidence must be a JSON object",
        ));
    }
    let meta = validate_archive_record(
        &payload["record"],
        &event.machine,
        "archive-state-recorded record",
    )?;
    if meta.slot != slot {
        return Err(ObserverError::invalid(format!(
            "archive-state-recorded record does not match slot {slot}"
        )));
    }
    if action == "absent-validation-row-recovered" {
        let evidence = validation_recovery_evidence(
            &payload["evidence"],
            "archive validation recovery evidence",
        )?;
        if meta.slot_type != "validate"
            || meta.physical_storage != "removed"
            || evidence.archive_id != meta.archive_id
        {
            return Err(ObserverError::invalid(
                "archive validation recovery evidence does not match its record",
            ));
        }
    } else if action == "absent-agent-row-recovered" {
        let evidence =
            agent_recovery_evidence(&payload["evidence"], "archive agent recovery evidence")?;
        if meta.slot_type != "agent"
            || meta.physical_storage != "removed"
            || evidence.archive_id != meta.archive_id
        {
            return Err(ObserverError::invalid(
                "archive agent recovery evidence does not match its record",
            ));
        }
    }
    insert_archive_record(&payload["record"], meta, state)?;
    state.archive_revision = Some(current_revision + 1);
    Ok(())
}

fn insert_archive_record(
    record: &Value,
    meta: crate::schema::ArchiveRecordMeta,
    state: &mut State,
) -> Result<(), ObserverError> {
    if !state.archive_ids.insert(meta.archive_id.clone()) {
        return Err(ObserverError::invalid(format!(
            "duplicate archive_id in event log: {}",
            meta.archive_id
        )));
    }
    if !state.archived_slots.insert(meta.slot.clone()) {
        return Err(ObserverError::invalid(format!(
            "duplicate archived slot in event log: {}",
            meta.slot
        )));
    }
    state.archive_records.push(record.clone());
    Ok(())
}

fn validate_state_header(
    state: &Map<String, Value>,
    machine: &str,
    label: &str,
) -> Result<(), ObserverError> {
    if unsigned(&state["schema"], &format!("{label}.schema"))? != STATE_SCHEMA {
        return Err(ObserverError::invalid(format!(
            "unsupported state schema in {label}"
        )));
    }
    if string(&state["machine"], &format!("{label}.machine"))? != machine {
        return Err(ObserverError::invalid(format!(
            "machine mismatch in {label}"
        )));
    }
    unsigned(&state["revision"], &format!("{label}.revision"))?;
    Ok(())
}

struct RecoveryEvidence {
    archive_id: String,
    source_record_sha256: String,
}

fn validation_recovery_evidence(
    value: &Value,
    label: &str,
) -> Result<RecoveryEvidence, ObserverError> {
    let evidence = object(value, label)?;
    exact_keys(
        evidence,
        &["archive_id", "source_record_sha256", "validation_outcome"],
        label,
    )?;
    if string(
        &evidence["validation_outcome"],
        &format!("{label}.validation_outcome"),
    )? != "unknown"
    {
        return Err(ObserverError::invalid(format!(
            "{label}.validation_outcome must be unknown"
        )));
    }
    recovery_identity(evidence, label)
}

fn agent_recovery_evidence(value: &Value, label: &str) -> Result<RecoveryEvidence, ObserverError> {
    let evidence = object(value, label)?;
    exact_keys(
        evidence,
        &["archive_id", "source_record_sha256", "physical_storage"],
        label,
    )?;
    if string(
        &evidence["physical_storage"],
        &format!("{label}.physical_storage"),
    )? != "externally-absent"
    {
        return Err(ObserverError::invalid(format!(
            "{label}.physical_storage must be externally-absent"
        )));
    }
    recovery_identity(evidence, label)
}

fn recovery_identity(
    evidence: &Map<String, Value>,
    label: &str,
) -> Result<RecoveryEvidence, ObserverError> {
    let archive_id = string(&evidence["archive_id"], &format!("{label}.archive_id"))?;
    let source_record_sha256 = string(
        &evidence["source_record_sha256"],
        &format!("{label}.source_record_sha256"),
    )?;
    if source_record_sha256.len() != 64
        || !source_record_sha256
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
    {
        return Err(ObserverError::invalid(format!(
            "{label}.source_record_sha256 is not a lowercase SHA-256 digest"
        )));
    }
    Ok(RecoveryEvidence {
        archive_id: archive_id.to_owned(),
        source_record_sha256: source_record_sha256.to_owned(),
    })
}

fn continued_revision(
    current: u64,
    previous: &Value,
    revision: &Value,
    label: &str,
) -> Result<(), ObserverError> {
    let recorded_previous = unsigned(previous, &format!("{label} previous_revision"))?;
    let recorded_revision = unsigned(revision, &format!("{label} revision"))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| ObserverError::invalid(format!("{label} revision overflow")))?;
    if recorded_previous != current || recorded_revision != next {
        return Err(ObserverError::invalid(format!(
            "{label} revision does not continue the derived state"
        )));
    }
    Ok(())
}

fn exact_keys(
    value: &Map<String, Value>,
    expected: &[&str],
    label: &str,
) -> Result<(), ObserverError> {
    let actual = value.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
        let unknown = actual.difference(&expected).copied().collect::<Vec<_>>();
        return Err(ObserverError::invalid(format!(
            "{label} has invalid fields: missing {missing:?}; unknown {unknown:?}"
        )));
    }
    Ok(())
}

fn object<'a>(value: &'a Value, label: &str) -> Result<&'a Map<String, Value>, ObserverError> {
    value
        .as_object()
        .ok_or_else(|| ObserverError::invalid(format!("{label} must be a JSON object")))
}

fn array<'a>(value: &'a Value, label: &str) -> Result<&'a [Value], ObserverError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| ObserverError::invalid(format!("{label} must be a JSON array")))
}

fn string<'a>(value: &'a Value, label: &str) -> Result<&'a str, ObserverError> {
    value
        .as_str()
        .ok_or_else(|| ObserverError::invalid(format!("{label} must be a string")))
}

fn unsigned(value: &Value, label: &str) -> Result<u64, ObserverError> {
    value
        .as_u64()
        .ok_or_else(|| ObserverError::invalid(format!("{label} must be a non-negative integer")))
}

fn validate_name(value: &str, label: &str) -> Result<(), ObserverError> {
    let bytes = value.as_bytes();
    let first_is_valid = bytes.first().is_some_and(u8::is_ascii_alphanumeric);
    let rest_is_valid = bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if bytes.len() > 64 || !first_is_valid || !rest_is_valid {
        return Err(ObserverError::invalid(format!(
            "{label} is not a valid name: {value:?}"
        )));
    }
    Ok(())
}

pub(crate) fn canonical_payload(event: &Event) -> Result<String, ObserverError> {
    canonical_json(&event.payload)
}
