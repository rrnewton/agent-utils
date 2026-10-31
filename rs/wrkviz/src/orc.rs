//! Import Orc SQLite session lineages:
//! `py/wrkviz/orc.py` at parity. This module lands in
//! stages: manifest record types first, then discovery
//! and snapshotting, then team assembly.

use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::json::py_repr_str;
use crate::team_data::{Event, ToolCall, Turn};

/// The `auxiliary-policy-legacy` policy/state constant.
pub const AUXILIARY_POLICY_LEGACY: &str = "legacy-message-prefix-v1";
/// The `auxiliary-policy-stable-spawns` policy/state constant.
pub const AUXILIARY_POLICY_STABLE_SPAWNS: &str = "stable-spawn-subset-v1";
/// The `auxiliary-policy-not-applicable` policy/state constant.
pub const AUXILIARY_POLICY_NOT_APPLICABLE: &str = "not-applicable";
/// The `auxiliary-degradation-reason` policy/state constant.
pub const AUXILIARY_DEGRADATION_REASON: &str =
    "conversation-history-rewritten-stable-spawns-preserved";
/// The `prefix-override-policy` policy/state constant.
pub const PREFIX_OVERRIDE_POLICY: &str = "operator-accepted-prefix-rewrite-v1";
/// The `prefix-override-degradation-reason` policy/state constant.
pub const PREFIX_OVERRIDE_DEGRADATION_REASON: &str =
    "append-prefix-rewritten-operator-accepted-rows-preserved";
/// The `task-projection-root` policy/state constant.
pub const TASK_PROJECTION_ROOT: &str = ".projections";
/// The `task-projection-policy` policy/state constant.
pub const TASK_PROJECTION_POLICY: &str = "frozen-note-provenance-v2";
/// The `task-history-policy` policy/state constant.
pub const TASK_HISTORY_POLICY: &str = "frozen-note-history-v3";
/// The `task-projection-degradation-reason` policy/state constant.
pub const TASK_PROJECTION_DEGRADATION_REASON: &str = "task-metadata-rewritten-enrichment-preserved";
/// The `task-history-degradation-reason` policy/state constant.
pub const TASK_HISTORY_DEGRADATION_REASON: &str = "task-source-rewritten-frozen-history-preserved";
/// The `semantic-identity-legacy` policy/state constant.
pub const SEMANTIC_IDENTITY_LEGACY: &str = "legacy-raw-v1";
/// The `semantic-identity-deterministic` policy/state constant.
pub const SEMANTIC_IDENTITY_DETERMINISTIC: &str = "normalized-v2";
/// The `source-state-live` policy/state constant.
pub const SOURCE_STATE_LIVE: &str = "live";
/// The `source-state-detached` policy/state constant.
pub const SOURCE_STATE_DETACHED: &str = "detached";
/// The `snapshot-object-root` policy/state constant.
pub const SNAPSHOT_OBJECT_ROOT: &str = ".objects";

/// See the module documentation.
pub fn empty_sha256() -> String {
    sha256_hex(b"")
}

/// See the module documentation.
pub fn empty_task_rewrite_sha256() -> String {
    sha256_hex(b"task-rewrite:none")
}

/// See the module documentation.
pub fn sha256_hex(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

// ---------------------------------------------------------------- helpers

/// See the enclosing record's documentation.
pub fn required_string(value: Option<&Value>, where_: &str) -> Result<String, String> {
    match value {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        _ => Err(format!("{where_}: expected a non-empty string")),
    }
}

/// See the enclosing record's documentation.
pub fn optional_string(value: Option<&Value>, where_: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        _ => Err(format!("{where_}: expected a string or null")),
    }
}

/// See the enclosing record's documentation.
pub fn string_value(value: Option<&Value>, where_: &str) -> Result<String, String> {
    match value {
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(format!("{where_}: expected a string")),
    }
}

/// See the enclosing record's documentation.
pub fn boolean(value: Option<&Value>, where_: &str) -> Result<bool, String> {
    match value {
        Some(Value::Bool(flag)) => Ok(*flag),
        _ => Err(format!("{where_}: expected a boolean")),
    }
}

fn integer(value: Option<&Value>, where_: &str) -> Result<i64, String> {
    match value {
        Some(Value::Bool(_)) | None | Some(Value::Null) => {
            Err(format!("{where_}: expected an integer"))
        }
        Some(other) => {
            crate::json::as_int(other, where_).map_err(|_| format!("{where_}: expected an integer"))
        }
    }
}

/// See the enclosing record's documentation.
/// See the module documentation.
pub fn nonnegative_big(value: Option<&Value>, where_: &str) -> Result<BigUint, String> {
    // Append watermarks pack two 64-bit ids and a tag bit.
    match value {
        Some(Value::Number(number)) => BigUint::from_decimal(&number.to_string())
            .ok_or_else(|| format!("{where_}: expected a non-negative integer")),
        _ => Err(format!("{where_}: expected a non-negative integer")),
    }
}

/// A JSON number holding a BigUint exactly.
pub struct JsonNumber;

impl JsonNumber {
    /// See the enclosing record's documentation.
    pub fn from_decimal(value: &BigUint) -> Value {
        serde_json::from_str::<Value>(&value.to_decimal()).unwrap_or(Value::Null)
    }
}

/// See the module documentation.
pub fn nonnegative_integer(value: Option<&Value>, where_: &str) -> Result<i64, String> {
    let held =
        integer(value, where_).map_err(|_| format!("{where_}: expected a non-negative integer"))?;
    if held < 0 {
        return Err(format!("{where_}: expected a non-negative integer"));
    }
    Ok(held)
}

/// See the enclosing record's documentation.
pub fn signed_integer(value: Option<&Value>, where_: &str) -> Result<i64, String> {
    integer(value, where_)
}

/// See the enclosing record's documentation.
pub fn positive_integer(value: Option<&Value>, where_: &str) -> Result<i64, String> {
    let held =
        integer(value, where_).map_err(|_| format!("{where_}: expected a positive integer"))?;
    if held <= 0 {
        return Err(format!("{where_}: expected a positive integer"));
    }
    Ok(held)
}

/// See the module documentation.
pub fn sha256_string(value: Option<&Value>, where_: &str) -> Result<String, String> {
    let text = required_string(value, where_)?;
    let bytes = text.as_bytes();
    if bytes.len() != 64
        || !bytes
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
    {
        return Err(format!("{where_}: expected a SHA-256 digest"));
    }
    Ok(text)
}

/// See the module documentation.
pub fn mapping<'a>(
    value: Option<&'a Value>,
    where_: &str,
) -> Result<&'a Map<String, Value>, String> {
    value
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{where_}: expected an object"))
}

/// See the enclosing record's documentation.
pub fn mapping_owned(value: Option<&Value>, where_: &str) -> Result<Map<String, Value>, String> {
    mapping(value, where_).cloned()
}

/// See the module documentation.
pub fn array<'a>(value: Option<&'a Value>, where_: &str) -> Result<&'a Vec<Value>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{where_}: expected an array"))
}

/// See the enclosing record's documentation.
pub fn require_exact_keys(
    raw: &Map<String, Value>,
    expected: &[&str],
    where_: &str,
) -> Result<(), String> {
    let mut missing: Vec<&str> = expected
        .iter()
        .filter(|key| !raw.contains_key(**key))
        .copied()
        .collect();
    let mut unknown: Vec<&str> = raw
        .keys()
        .map(String::as_str)
        .filter(|key| !expected.contains(key))
        .collect();
    if missing.is_empty() && unknown.is_empty() {
        return Ok(());
    }
    missing.sort();
    unknown.sort();
    let mut details: Vec<String> = Vec::new();
    if !missing.is_empty() {
        details.push(format!("missing {}", missing.join(", ")));
    }
    if !unknown.is_empty() {
        details.push(format!("unknown {}", unknown.join(", ")));
    }
    Err(format!("{where_}: invalid fields ({})", details.join("; ")))
}

fn key_set(raw: &Map<String, Value>) -> Vec<String> {
    let mut keys: Vec<String> = raw.keys().cloned().collect();
    keys.sort();
    keys
}

fn exact_field_sets(raw: &Map<String, Value>, sets: &[&[&str]]) -> bool {
    let actual = key_set(raw);
    sets.iter().any(|set| {
        let mut wanted: Vec<&str> = set.to_vec();
        wanted.sort();
        actual.iter().map(String::as_str).collect::<Vec<_>>() == wanted
    })
}

/// Filtered segments of a safe snapshot-relative path.
pub fn safe_relative_parts(value: &str) -> Result<Vec<String>, String> {
    let parts: Vec<String> = value
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect();
    if value.starts_with('/') || parts.is_empty() || parts.iter().any(|part| part == "..") {
        return Err(format!("unsafe Orc snapshot path {}", py_repr_str(value)));
    }
    Ok(parts)
}

/// See the enclosing record's documentation.
pub fn safe_relative(value: &str) -> Result<(), String> {
    safe_relative_parts(value).map(|_| ())
}

/// `[A-Za-z0-9][A-Za-z0-9._-]*`, at most 255 characters.
pub fn safe_component(value: &str, where_: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let ok = value.chars().count() <= 255
        && !bytes.is_empty()
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(value.to_string())
    } else {
        Err(format!(
            "{where_}: unsafe path component {}",
            py_repr_str(value)
        ))
    }
}

/// See the enclosing record's documentation.
pub fn snapshot_object_relative(sha256: &str) -> Result<String, String> {
    sha256_string(
        Some(&Value::String(sha256.to_string())),
        "snapshot object digest",
    )?;
    Ok(format!(
        "{SNAPSHOT_OBJECT_ROOT}/{}/{sha256}.db",
        &sha256[..2]
    ))
}

/// See the enclosing record's documentation.
pub fn task_projection_relative(sha256: &str) -> Result<String, String> {
    sha256_string(
        Some(&Value::String(sha256.to_string())),
        "task projection digest",
    )?;
    Ok(format!(
        "{TASK_PROJECTION_ROOT}/{}/{sha256}.json",
        &sha256[..2]
    ))
}

/// A non-negative integer wider than any machine word:
/// packed session watermarks carry a tag bit at position
/// 128, and manifest integers are unbounded in the oracle.
/// Four little-endian limbs are far beyond any real value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BigUint(pub [u64; 4]);

impl BigUint {
    /// See the enclosing record's documentation.
    pub fn from_u64(value: u64) -> BigUint {
        BigUint([value, 0, 0, 0])
    }

    /// See the enclosing record's documentation.
    pub fn from_u128(value: u128) -> BigUint {
        BigUint([value as u64, (value >> 64) as u64, 0, 0])
    }

    /// See the enclosing record's documentation.
    pub fn zero() -> BigUint {
        BigUint([0, 0, 0, 0])
    }

    /// Pack content and message watermarks under the tag.
    pub fn packed(content: u64, message: u64) -> BigUint {
        BigUint([message, content, 1, 0])
    }

    /// Whether the session-state tag bit is set.
    pub fn has_session_tag(&self) -> bool {
        self.0[2] & 1 == 1
    }

    /// (content, message) watermarks with the tag removed.
    pub fn unpacked(&self) -> (u64, u64) {
        (self.0[1], self.0[0])
    }

    /// The value as u64 when it fits.
    pub fn as_u64(&self) -> Option<u64> {
        if self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0 {
            Some(self.0[0])
        } else {
            None
        }
    }

    /// Decimal digits (canonical JSON spelling).
    pub fn to_decimal(&self) -> String {
        if self.0.iter().all(|limb| *limb == 0) {
            return "0".to_string();
        }
        let mut limbs = self.0;
        let mut chunks: Vec<u64> = Vec::new();
        loop {
            let mut remainder: u128 = 0;
            for index in (0..4).rev() {
                let current = (remainder << 64) | u128::from(limbs[index]);
                let quotient = current / 10_000_000_000_000_000_000u128;
                remainder = current % 10_000_000_000_000_000_000u128;
                limbs[index] = quotient as u64;
            }
            chunks.push(remainder as u64);
            if limbs.iter().all(|limb| *limb == 0) {
                break;
            }
        }
        let mut out = chunks.last().copied().unwrap_or(0).to_string();
        for chunk in chunks.iter().rev().skip(1) {
            out.push_str(&format!("{chunk:019}"));
        }
        out
    }

    /// Parse canonical decimal digits.
    pub fn from_decimal(text: &str) -> Option<BigUint> {
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let mut value = BigUint::zero();
        for byte in text.bytes() {
            // value = value * 10 + digit, limb-wise.
            let mut carry: u128 = u128::from(byte - b'0');
            for limb in value.0.iter_mut() {
                let current = u128::from(*limb) * 10 + carry;
                *limb = current as u64;
                carry = current >> 64;
            }
            if carry != 0 {
                return None;
            }
        }
        Some(value)
    }
}

impl PartialOrd for BigUint {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BigUint {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        for index in (0..4).rev() {
            match self.0[index].cmp(&other.0[index]) {
                std::cmp::Ordering::Equal => continue,
                held => return held,
            }
        }
        std::cmp::Ordering::Equal
    }
}

impl std::fmt::Display for BigUint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_decimal())
    }
}

// ---------------------------------------------------------------- records

/// Bounded provenance for the rewritable conversation
/// projection.
#[derive(Clone, Debug, PartialEq)]
pub struct AuxiliaryStatus {
    /// See the enclosing record's documentation.
    pub policy: String,
    /// See the enclosing record's documentation.
    pub message_count: i64,
    /// See the enclosing record's documentation.
    pub message_sha256: String,
    /// See the enclosing record's documentation.
    pub stable_spawn_count: i64,
    /// See the enclosing record's documentation.
    pub stable_spawn_sha256: String,
    /// See the enclosing record's documentation.
    pub rewrite_count: i64,
    /// See the enclosing record's documentation.
    pub last_rewrite_at: Option<String>,
    /// See the enclosing record's documentation.
    pub degraded: bool,
    /// See the enclosing record's documentation.
    pub degradation_reason: Option<String>,
}

impl AuxiliaryStatus {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("policy".into(), Value::String(self.policy.clone()));
        out.insert("message_count".into(), self.message_count.into());
        out.insert(
            "message_sha256".into(),
            Value::String(self.message_sha256.clone()),
        );
        out.insert("stable_spawn_count".into(), self.stable_spawn_count.into());
        out.insert(
            "stable_spawn_sha256".into(),
            Value::String(self.stable_spawn_sha256.clone()),
        );
        out.insert("rewrite_count".into(), self.rewrite_count.into());
        out.insert(
            "last_rewrite_at".into(),
            self.last_rewrite_at
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("degraded".into(), Value::Bool(self.degraded));
        out.insert(
            "degradation_reason".into(),
            self.degradation_reason
                .clone()
                .map_or(Value::Null, Value::String),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<AuxiliaryStatus, String> {
        require_exact_keys(
            raw,
            &[
                "policy",
                "message_count",
                "message_sha256",
                "stable_spawn_count",
                "stable_spawn_sha256",
                "rewrite_count",
                "last_rewrite_at",
                "degraded",
                "degradation_reason",
            ],
            where_,
        )?;
        let policy = required_string(raw.get("policy"), &format!("{where_}.policy"))?;
        if policy != AUXILIARY_POLICY_STABLE_SPAWNS && policy != AUXILIARY_POLICY_NOT_APPLICABLE {
            return Err(format!(
                "{where_}.policy: unsupported policy {}",
                py_repr_str(&policy)
            ));
        }
        let message_sha256 = sha256_string(
            raw.get("message_sha256"),
            &format!("{where_}.message_sha256"),
        )?;
        let stable_spawn_sha256 = sha256_string(
            raw.get("stable_spawn_sha256"),
            &format!("{where_}.stable_spawn_sha256"),
        )?;
        let rewrite_count =
            nonnegative_integer(raw.get("rewrite_count"), &format!("{where_}.rewrite_count"))?;
        let last_rewrite_at = optional_string(
            raw.get("last_rewrite_at"),
            &format!("{where_}.last_rewrite_at"),
        )?;
        let degraded = boolean(raw.get("degraded"), &format!("{where_}.degraded"))?;
        let degradation_reason = optional_string(
            raw.get("degradation_reason"),
            &format!("{where_}.degradation_reason"),
        )?;
        if rewrite_count == 0
            && (last_rewrite_at.is_some() || degraded || degradation_reason.is_some())
        {
            return Err(format!(
                "{where_}: zero rewrites cannot carry degradation metadata"
            ));
        }
        if rewrite_count > 0
            && (last_rewrite_at.is_none()
                || !degraded
                || degradation_reason.as_deref() != Some(AUXILIARY_DEGRADATION_REASON))
        {
            return Err(format!(
                "{where_}: rewrite history requires complete degradation metadata"
            ));
        }
        let message_count =
            nonnegative_integer(raw.get("message_count"), &format!("{where_}.message_count"))?;
        let stable_spawn_count = nonnegative_integer(
            raw.get("stable_spawn_count"),
            &format!("{where_}.stable_spawn_count"),
        )?;
        if policy == AUXILIARY_POLICY_NOT_APPLICABLE
            && (rewrite_count != 0
                || message_count != 0
                || stable_spawn_count != 0
                || message_sha256 != empty_sha256()
                || stable_spawn_sha256 != empty_sha256())
        {
            return Err(format!(
                "{where_}: not-applicable auxiliary provenance must be empty"
            ));
        }
        Ok(AuxiliaryStatus {
            policy,
            message_count,
            message_sha256,
            stable_spawn_count,
            stable_spawn_sha256,
            rewrite_count,
            last_rewrite_at,
            degraded,
            degradation_reason,
        })
    }
}

/// One guarded column whose stored value differs from the
/// recorded append prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefixColumnChange {
    /// See the enclosing record's documentation.
    pub column: String,
    /// See the enclosing record's documentation.
    pub previous: String,
    /// See the enclosing record's documentation.
    pub observed: String,
    /// See the enclosing record's documentation.
    pub bounded: bool,
    /// See the enclosing record's documentation.
    pub json_paths: Vec<String>,
    /// See the enclosing record's documentation.
    pub json_paths_bounded: bool,
}

impl PrefixColumnChange {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("column".into(), Value::String(self.column.clone()));
        out.insert("previous".into(), Value::String(self.previous.clone()));
        out.insert("observed".into(), Value::String(self.observed.clone()));
        out.insert("bounded".into(), Value::Bool(self.bounded));
        out.insert(
            "json_paths".into(),
            Value::Array(self.json_paths.iter().cloned().map(Value::String).collect()),
        );
        out.insert(
            "json_paths_bounded".into(),
            Value::Bool(self.json_paths_bounded),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<PrefixColumnChange, String> {
        require_exact_keys(
            raw,
            &[
                "column",
                "previous",
                "observed",
                "bounded",
                "json_paths",
                "json_paths_bounded",
            ],
            where_,
        )?;
        let mut json_paths = Vec::new();
        for (index, item) in array(raw.get("json_paths"), &format!("{where_}.json_paths"))?
            .iter()
            .enumerate()
        {
            json_paths.push(required_string(
                Some(item),
                &format!("{where_}.json_paths[{index}]"),
            )?);
        }
        Ok(PrefixColumnChange {
            column: required_string(raw.get("column"), &format!("{where_}.column"))?,
            previous: string_value(raw.get("previous"), &format!("{where_}.previous"))?,
            observed: string_value(raw.get("observed"), &format!("{where_}.observed"))?,
            bounded: boolean(raw.get("bounded"), &format!("{where_}.bounded"))?,
            json_paths,
            json_paths_bounded: boolean(
                raw.get("json_paths_bounded"),
                &format!("{where_}.json_paths_bounded"),
            )?,
        })
    }

    /// See the enclosing record's documentation.
    pub fn describe(&self) -> String {
        let mut fields = String::new();
        if !self.json_paths.is_empty() {
            fields = format!(" .{}", self.json_paths.join(", ."));
            if self.json_paths_bounded {
                fields += ", ...";
            }
        }
        format!(
            "{}{}: {} -> {}",
            self.column, fields, self.previous, self.observed
        )
    }
}

/// One row at or below the append watermark whose guarded
/// columns were rewritten.
#[derive(Clone, Debug, PartialEq)]
pub struct PrefixRowChange {
    /// See the enclosing record's documentation.
    pub table: String,
    /// See the enclosing record's documentation.
    pub row_id: i64,
    /// See the enclosing record's documentation.
    pub columns: Vec<PrefixColumnChange>,
}

impl PrefixRowChange {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("table".into(), Value::String(self.table.clone()));
        out.insert("row_id".into(), self.row_id.into());
        out.insert(
            "columns".into(),
            Value::Array(
                self.columns
                    .iter()
                    .map(PrefixColumnChange::to_json)
                    .collect(),
            ),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<PrefixRowChange, String> {
        require_exact_keys(raw, &["table", "row_id", "columns"], where_)?;
        let mut columns = Vec::new();
        for (index, item) in array(raw.get("columns"), &format!("{where_}.columns"))?
            .iter()
            .enumerate()
        {
            let held = format!("{where_}.columns[{index}]");
            columns.push(PrefixColumnChange::from_json(
                mapping(Some(item), &held)?,
                &held,
            )?);
        }
        if columns.is_empty() {
            return Err(format!(
                "{where_}: a recorded row change must name a column"
            ));
        }
        Ok(PrefixRowChange {
            table: required_string(raw.get("table"), &format!("{where_}.table"))?,
            row_id: signed_integer(raw.get("row_id"), &format!("{where_}.row_id"))?,
            columns,
        })
    }

    /// See the enclosing record's documentation.
    pub fn describe(&self) -> String {
        let rendered: Vec<String> = self
            .columns
            .iter()
            .map(PrefixColumnChange::describe)
            .collect();
        format!(
            "{} row {}: {}",
            self.table,
            self.row_id,
            rendered.join("; ")
        )
    }
}

/// An operator's recorded decision to re-baseline one
/// source's append-prefix digest.
#[derive(Clone, Debug, PartialEq)]
pub struct AppendPrefixOverride {
    /// See the enclosing record's documentation.
    pub policy: String,
    /// See the enclosing record's documentation.
    pub source_path: String,
    /// See the enclosing record's documentation.
    pub accepted_at: String,
    /// See the enclosing record's documentation.
    pub override_count: i64,
    /// See the enclosing record's documentation.
    pub previous_append_prefix_sha256: String,
    /// See the enclosing record's documentation.
    pub observed_append_prefix_sha256: String,
    /// See the enclosing record's documentation.
    pub superseded_snapshot_path: String,
    /// See the enclosing record's documentation.
    pub superseded_sha256: String,
    /// See the enclosing record's documentation.
    pub changed_row_count: i64,
    /// See the enclosing record's documentation.
    pub changed_rows: Vec<PrefixRowChange>,
    /// See the enclosing record's documentation.
    pub changed_rows_bounded: bool,
    /// See the enclosing record's documentation.
    pub degraded: bool,
    /// See the enclosing record's documentation.
    pub degradation_reason: String,
}

impl AppendPrefixOverride {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("policy".into(), Value::String(self.policy.clone()));
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert(
            "accepted_at".into(),
            Value::String(self.accepted_at.clone()),
        );
        out.insert("override_count".into(), self.override_count.into());
        out.insert(
            "previous_append_prefix_sha256".into(),
            Value::String(self.previous_append_prefix_sha256.clone()),
        );
        out.insert(
            "observed_append_prefix_sha256".into(),
            Value::String(self.observed_append_prefix_sha256.clone()),
        );
        out.insert(
            "superseded_snapshot_path".into(),
            Value::String(self.superseded_snapshot_path.clone()),
        );
        out.insert(
            "superseded_sha256".into(),
            Value::String(self.superseded_sha256.clone()),
        );
        out.insert("changed_row_count".into(), self.changed_row_count.into());
        out.insert(
            "changed_rows".into(),
            Value::Array(
                self.changed_rows
                    .iter()
                    .map(PrefixRowChange::to_json)
                    .collect(),
            ),
        );
        out.insert(
            "changed_rows_bounded".into(),
            Value::Bool(self.changed_rows_bounded),
        );
        out.insert("degraded".into(), Value::Bool(self.degraded));
        out.insert(
            "degradation_reason".into(),
            Value::String(self.degradation_reason.clone()),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(
        raw: &Map<String, Value>,
        where_: &str,
    ) -> Result<AppendPrefixOverride, String> {
        require_exact_keys(
            raw,
            &[
                "policy",
                "source_path",
                "accepted_at",
                "override_count",
                "previous_append_prefix_sha256",
                "observed_append_prefix_sha256",
                "superseded_snapshot_path",
                "superseded_sha256",
                "changed_row_count",
                "changed_rows",
                "changed_rows_bounded",
                "degraded",
                "degradation_reason",
            ],
            where_,
        )?;
        let policy = required_string(raw.get("policy"), &format!("{where_}.policy"))?;
        if policy != PREFIX_OVERRIDE_POLICY {
            return Err(format!(
                "{where_}.policy: unsupported policy {}",
                py_repr_str(&policy)
            ));
        }
        let override_count = nonnegative_integer(
            raw.get("override_count"),
            &format!("{where_}.override_count"),
        )?;
        if override_count < 1 {
            return Err(format!(
                "{where_}: a recorded override must have happened at least once"
            ));
        }
        let degraded = boolean(raw.get("degraded"), &format!("{where_}.degraded"))?;
        let degradation_reason = required_string(
            raw.get("degradation_reason"),
            &format!("{where_}.degradation_reason"),
        )?;
        if !degraded || degradation_reason != PREFIX_OVERRIDE_DEGRADATION_REASON {
            return Err(format!(
                "{where_}: an accepted prefix rewrite must record its degradation"
            ));
        }
        let previous_digest = sha256_string(
            raw.get("previous_append_prefix_sha256"),
            &format!("{where_}.previous_append_prefix_sha256"),
        )?;
        let observed_digest = sha256_string(
            raw.get("observed_append_prefix_sha256"),
            &format!("{where_}.observed_append_prefix_sha256"),
        )?;
        if previous_digest == observed_digest {
            return Err(format!(
                "{where_}: an accepted prefix rewrite must record two different digests"
            ));
        }
        let superseded_sha256 = sha256_string(
            raw.get("superseded_sha256"),
            &format!("{where_}.superseded_sha256"),
        )?;
        let superseded_snapshot_path = required_string(
            raw.get("superseded_snapshot_path"),
            &format!("{where_}.superseded_snapshot_path"),
        )?;
        safe_relative(&superseded_snapshot_path)?;
        let expected_superseded = snapshot_object_relative(&superseded_sha256)?;
        if superseded_snapshot_path.starts_with(&format!("{SNAPSHOT_OBJECT_ROOT}/"))
            && superseded_snapshot_path != expected_superseded
        {
            return Err(format!(
                "{where_}.superseded_snapshot_path: expected content-addressed path {}",
                py_repr_str(&expected_superseded)
            ));
        }
        let mut changed_rows = Vec::new();
        for (index, item) in array(raw.get("changed_rows"), &format!("{where_}.changed_rows"))?
            .iter()
            .enumerate()
        {
            let held = format!("{where_}.changed_rows[{index}]");
            changed_rows.push(PrefixRowChange::from_json(
                mapping(Some(item), &held)?,
                &held,
            )?);
        }
        let changed_row_count = nonnegative_integer(
            raw.get("changed_row_count"),
            &format!("{where_}.changed_row_count"),
        )?;
        let changed_rows_bounded = boolean(
            raw.get("changed_rows_bounded"),
            &format!("{where_}.changed_rows_bounded"),
        )?;
        if changed_row_count < 1
            || changed_row_count < changed_rows.len() as i64
            || changed_rows_bounded != ((changed_rows.len() as i64) < changed_row_count)
        {
            return Err(format!(
                "{where_}: recorded row evidence disagrees with its own row count"
            ));
        }
        Ok(AppendPrefixOverride {
            policy,
            source_path: required_string(raw.get("source_path"), &format!("{where_}.source_path"))?,
            accepted_at: required_string(raw.get("accepted_at"), &format!("{where_}.accepted_at"))?,
            override_count,
            previous_append_prefix_sha256: previous_digest,
            observed_append_prefix_sha256: observed_digest,
            superseded_snapshot_path,
            superseded_sha256,
            changed_row_count,
            changed_rows,
            changed_rows_bounded,
            degraded,
            degradation_reason,
        })
    }

    /// See the enclosing record's documentation.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "accepted append-prefix rewrite for {}: {} row(s) changed at or below the append watermark (override #{}, {})",
            self.source_path,
            self.changed_row_count,
            self.override_count,
            self.degradation_reason
        )];
        lines.extend(self.changed_rows.iter().map(PrefixRowChange::describe));
        if self.changed_rows_bounded {
            lines.push(format!(
                "... {} further changed row(s) not shown",
                self.changed_row_count - self.changed_rows.len() as i64
            ));
        }
        lines.push(format!(
            "pre-rewrite snapshot retained for comparison: {} (reclaimed by the next accepted override on this source)",
            self.superseded_snapshot_path
        ));
        lines
    }
}

/// Pointer and provenance for frozen per-note task
/// enrichment.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskProjection {
    /// See the enclosing record's documentation.
    pub policy: String,
    /// See the enclosing record's documentation.
    pub path: String,
    /// See the enclosing record's documentation.
    pub note_count: i64,
    /// See the enclosing record's documentation.
    pub sha256: String,
    /// See the enclosing record's documentation.
    pub observed_enrichment_sha256: String,
    /// See the enclosing record's documentation.
    pub observed_note_rewrite_sha256: String,
    /// See the enclosing record's documentation.
    pub missing_note_count: i64,
    /// See the enclosing record's documentation.
    pub missing_note_ids_sha256: String,
    /// See the enclosing record's documentation.
    pub observed_note_sequence: Option<i64>,
    /// See the enclosing record's documentation.
    pub unobserved_note_id_gap_count: Option<i64>,
    /// See the enclosing record's documentation.
    pub unobserved_note_id_gap_sha256: Option<String>,
    /// See the enclosing record's documentation.
    pub rewrite_count: i64,
    /// See the enclosing record's documentation.
    pub last_rewrite_at: Option<String>,
    /// See the enclosing record's documentation.
    pub degraded: bool,
    /// See the enclosing record's documentation.
    pub degradation_reason: Option<String>,
}

const TASK_LEGACY_FIELDS: &[&str] = &[
    "policy",
    "path",
    "note_count",
    "sha256",
    "observed_enrichment_sha256",
    "rewrite_count",
    "last_rewrite_at",
    "degraded",
    "degradation_reason",
];

impl TaskProjection {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("policy".into(), Value::String(self.policy.clone()));
        out.insert("path".into(), Value::String(self.path.clone()));
        out.insert("note_count".into(), self.note_count.into());
        out.insert("sha256".into(), Value::String(self.sha256.clone()));
        out.insert(
            "observed_enrichment_sha256".into(),
            Value::String(self.observed_enrichment_sha256.clone()),
        );
        out.insert(
            "observed_note_rewrite_sha256".into(),
            Value::String(self.observed_note_rewrite_sha256.clone()),
        );
        out.insert("missing_note_count".into(), self.missing_note_count.into());
        out.insert(
            "missing_note_ids_sha256".into(),
            Value::String(self.missing_note_ids_sha256.clone()),
        );
        out.insert(
            "observed_note_sequence".into(),
            self.observed_note_sequence.map_or(Value::Null, Value::from),
        );
        out.insert(
            "unobserved_note_id_gap_count".into(),
            self.unobserved_note_id_gap_count
                .map_or(Value::Null, Value::from),
        );
        out.insert(
            "unobserved_note_id_gap_sha256".into(),
            self.unobserved_note_id_gap_sha256
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("rewrite_count".into(), self.rewrite_count.into());
        out.insert(
            "last_rewrite_at".into(),
            self.last_rewrite_at
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("degraded".into(), Value::Bool(self.degraded));
        out.insert(
            "degradation_reason".into(),
            self.degradation_reason
                .clone()
                .map_or(Value::Null, Value::String),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<TaskProjection, String> {
        let mut prior: Vec<&str> = TASK_LEGACY_FIELDS.to_vec();
        prior.extend([
            "observed_note_rewrite_sha256",
            "missing_note_count",
            "missing_note_ids_sha256",
        ]);
        let mut sequence: Vec<&str> = prior.clone();
        sequence.push("observed_note_sequence");
        let mut current: Vec<&str> = sequence.clone();
        current.extend([
            "unobserved_note_id_gap_count",
            "unobserved_note_id_gap_sha256",
        ]);
        if !exact_field_sets(raw, &[TASK_LEGACY_FIELDS, &prior, &sequence, &current]) {
            require_exact_keys(raw, &current, where_)?;
        }
        let policy = required_string(raw.get("policy"), &format!("{where_}.policy"))?;
        if policy != TASK_PROJECTION_POLICY && policy != TASK_HISTORY_POLICY {
            return Err(format!(
                "{where_}.policy: unsupported policy {}",
                py_repr_str(&policy)
            ));
        }
        let sha256 = sha256_string(raw.get("sha256"), &format!("{where_}.sha256"))?;
        let path = required_string(raw.get("path"), &format!("{where_}.path"))?;
        if path != task_projection_relative(&sha256)? {
            return Err(format!(
                "{where_}.path: invalid content-addressed projection path"
            ));
        }
        let rewrite_count =
            nonnegative_integer(raw.get("rewrite_count"), &format!("{where_}.rewrite_count"))?;
        let last_rewrite_at = optional_string(
            raw.get("last_rewrite_at"),
            &format!("{where_}.last_rewrite_at"),
        )?;
        let degraded = boolean(raw.get("degraded"), &format!("{where_}.degraded"))?;
        let degradation_reason = optional_string(
            raw.get("degradation_reason"),
            &format!("{where_}.degradation_reason"),
        )?;
        if rewrite_count == 0
            && (last_rewrite_at.is_some() || degraded || degradation_reason.is_some())
        {
            return Err(format!(
                "{where_}: zero rewrites cannot carry degradation metadata"
            ));
        }
        if rewrite_count > 0
            && (last_rewrite_at.is_none()
                || !degraded
                || !matches!(
                    degradation_reason.as_deref(),
                    Some(TASK_PROJECTION_DEGRADATION_REASON)
                        | Some(TASK_HISTORY_DEGRADATION_REASON)
                ))
        {
            return Err(format!(
                "{where_}: rewrite history requires complete degradation metadata"
            ));
        }
        Ok(TaskProjection {
            policy,
            path,
            note_count: nonnegative_integer(
                raw.get("note_count"),
                &format!("{where_}.note_count"),
            )?,
            sha256: sha256.clone(),
            observed_enrichment_sha256: sha256_string(
                raw.get("observed_enrichment_sha256"),
                &format!("{where_}.observed_enrichment_sha256"),
            )?,
            observed_note_rewrite_sha256: if raw.contains_key("observed_note_rewrite_sha256") {
                sha256_string(
                    raw.get("observed_note_rewrite_sha256"),
                    &format!("{where_}.observed_note_rewrite_sha256"),
                )?
            } else {
                empty_task_rewrite_sha256()
            },
            missing_note_count: if raw.contains_key("missing_note_count") {
                nonnegative_integer(
                    raw.get("missing_note_count"),
                    &format!("{where_}.missing_note_count"),
                )?
            } else {
                0
            },
            missing_note_ids_sha256: if raw.contains_key("missing_note_ids_sha256") {
                sha256_string(
                    raw.get("missing_note_ids_sha256"),
                    &format!("{where_}.missing_note_ids_sha256"),
                )?
            } else {
                sha256_hex(b"[]")
            },
            observed_note_sequence: if raw.contains_key("observed_note_sequence") {
                Some(nonnegative_integer(
                    raw.get("observed_note_sequence"),
                    &format!("{where_}.observed_note_sequence"),
                )?)
            } else {
                None
            },
            unobserved_note_id_gap_count: if raw.contains_key("unobserved_note_id_gap_count") {
                Some(nonnegative_integer(
                    raw.get("unobserved_note_id_gap_count"),
                    &format!("{where_}.unobserved_note_id_gap_count"),
                )?)
            } else {
                None
            },
            unobserved_note_id_gap_sha256: if raw.contains_key("unobserved_note_id_gap_sha256") {
                Some(sha256_string(
                    raw.get("unobserved_note_id_gap_sha256"),
                    &format!("{where_}.unobserved_note_id_gap_sha256"),
                )?)
            } else {
                None
            },
            rewrite_count,
            last_rewrite_at,
            degraded,
            degradation_reason,
        })
    }
}

/// Tables a continuation boundary's row ordinal can index.
pub const CONTINUATION_BOUNDARY_TABLES: [&str; 2] = ["messages", "content_blocks"];

/// One explicitly configured successor coordinator.
#[derive(Clone, Debug, PartialEq)]
pub struct ContinuationSpec {
    /// See the enclosing record's documentation.
    pub session_id: String,
    /// See the enclosing record's documentation.
    pub start_message_id: Option<String>,
}

impl ContinuationSpec {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("session_id".into(), Value::String(self.session_id.clone()));
        out.insert(
            "start_message_id".into(),
            self.start_message_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<ContinuationSpec, String> {
        require_exact_keys(raw, &["session_id", "start_message_id"], where_)?;
        Ok(ContinuationSpec {
            session_id: safe_component(
                &required_string(raw.get("session_id"), &format!("{where_}.session_id"))?,
                &format!("{where_}.session_id"),
            )?,
            start_message_id: optional_string(
                raw.get("start_message_id"),
                &format!("{where_}.start_message_id"),
            )?,
        })
    }

    /// See the enclosing record's documentation.
    pub fn from_value(raw: &Value, where_: &str) -> Result<ContinuationSpec, String> {
        if raw.as_str().is_some() {
            return Ok(ContinuationSpec {
                session_id: safe_component(&required_string(Some(raw), where_)?, where_)?,
                start_message_id: None,
            });
        }
        ContinuationSpec::from_json(mapping(Some(raw), where_)?, where_)
    }
}

/// One explicit, frozen transition between parentless
/// coordinators.
#[derive(Clone, Debug, PartialEq)]
pub struct ContinuationLink {
    /// See the enclosing record's documentation.
    pub predecessor_session_id: String,
    /// See the enclosing record's documentation.
    pub session_id: String,
    /// See the enclosing record's documentation.
    pub predecessor_source_path: String,
    /// See the enclosing record's documentation.
    pub predecessor_source_line: i64,
    /// See the enclosing record's documentation.
    pub predecessor_at_ms: i64,
    /// See the enclosing record's documentation.
    pub source_path: String,
    /// See the enclosing record's documentation.
    pub start_message_id: Option<String>,
    /// See the enclosing record's documentation.
    pub start_source_line: Option<i64>,
    /// See the enclosing record's documentation.
    pub started_at_ms: i64,
    /// See the enclosing record's documentation.
    pub gap_ms: i64,
    /// See the enclosing record's documentation.
    pub predecessor_source_table: Option<String>,
}

const LINK_LEGACY_FIELDS: &[&str] = &[
    "predecessor_session_id",
    "session_id",
    "predecessor_source_path",
    "predecessor_source_line",
    "predecessor_at_ms",
    "source_path",
    "started_at_ms",
    "gap_ms",
];

impl ContinuationLink {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "predecessor_session_id".into(),
            Value::String(self.predecessor_session_id.clone()),
        );
        out.insert("session_id".into(), Value::String(self.session_id.clone()));
        out.insert(
            "predecessor_source_path".into(),
            Value::String(self.predecessor_source_path.clone()),
        );
        out.insert(
            "predecessor_source_line".into(),
            self.predecessor_source_line.into(),
        );
        out.insert(
            "predecessor_source_table".into(),
            self.predecessor_source_table
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("predecessor_at_ms".into(), self.predecessor_at_ms.into());
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert(
            "start_message_id".into(),
            self.start_message_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "start_source_line".into(),
            self.start_source_line.map_or(Value::Null, Value::from),
        );
        out.insert("started_at_ms".into(), self.started_at_ms.into());
        out.insert("gap_ms".into(), self.gap_ms.into());
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(raw: &Map<String, Value>, where_: &str) -> Result<ContinuationLink, String> {
        let mut bounded: Vec<&str> = LINK_LEGACY_FIELDS.to_vec();
        bounded.extend(["start_message_id", "start_source_line"]);
        let mut current: Vec<&str> = bounded.clone();
        current.push("predecessor_source_table");
        if !exact_field_sets(raw, &[LINK_LEGACY_FIELDS, &bounded, &current]) {
            require_exact_keys(raw, &current, where_)?;
        }
        let predecessor_session_id = safe_component(
            &required_string(
                raw.get("predecessor_session_id"),
                &format!("{where_}.predecessor_session_id"),
            )?,
            &format!("{where_}.predecessor_session_id"),
        )?;
        let session_id = safe_component(
            &required_string(raw.get("session_id"), &format!("{where_}.session_id"))?,
            &format!("{where_}.session_id"),
        )?;
        let predecessor_source_path = required_string(
            raw.get("predecessor_source_path"),
            &format!("{where_}.predecessor_source_path"),
        )?;
        let source_path =
            required_string(raw.get("source_path"), &format!("{where_}.source_path"))?;
        safe_relative(&predecessor_source_path)?;
        safe_relative(&source_path)?;
        let start_message_id = optional_string(
            raw.get("start_message_id"),
            &format!("{where_}.start_message_id"),
        )?;
        let start_source_line = match raw.get("start_source_line") {
            None | Some(Value::Null) => None,
            Some(other) => Some(nonnegative_integer(
                Some(other),
                &format!("{where_}.start_source_line"),
            )?),
        };
        let predecessor_source_line = nonnegative_integer(
            raw.get("predecessor_source_line"),
            &format!("{where_}.predecessor_source_line"),
        )?;
        let predecessor_source_table = optional_string(
            raw.get("predecessor_source_table"),
            &format!("{where_}.predecessor_source_table"),
        )?;
        if let Some(table) = &predecessor_source_table {
            if !CONTINUATION_BOUNDARY_TABLES.contains(&table.as_str()) {
                return Err(format!(
                    "{where_}.predecessor_source_table: unsupported Orc boundary table {}",
                    py_repr_str(table)
                ));
            }
        }
        let predecessor_at_ms = nonnegative_integer(
            raw.get("predecessor_at_ms"),
            &format!("{where_}.predecessor_at_ms"),
        )?;
        let started_at_ms =
            nonnegative_integer(raw.get("started_at_ms"), &format!("{where_}.started_at_ms"))?;
        let gap_ms = nonnegative_integer(raw.get("gap_ms"), &format!("{where_}.gap_ms"))?;
        if predecessor_session_id == session_id
            || predecessor_source_line == 0
            || predecessor_at_ms == 0
            || started_at_ms == 0
            || gap_ms == 0
            || gap_ms != started_at_ms - predecessor_at_ms
            || start_message_id.is_none() != start_source_line.is_none()
            || start_source_line == Some(0)
        {
            return Err(format!("{where_}: malformed Orc continuation record"));
        }
        Ok(ContinuationLink {
            predecessor_session_id,
            session_id,
            predecessor_source_path,
            predecessor_source_line,
            predecessor_at_ms,
            source_path,
            start_message_id,
            start_source_line,
            started_at_ms,
            gap_ms,
            predecessor_source_table,
        })
    }
}

/// One consistent SQLite backup and its logical
/// append-prefix evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceCopy {
    /// See the enclosing record's documentation.
    pub source_path: String,
    /// See the enclosing record's documentation.
    pub snapshot_path: String,
    /// See the enclosing record's documentation.
    pub kind: String,
    /// See the enclosing record's documentation.
    pub owner_session_id: String,
    /// See the enclosing record's documentation.
    pub lineage_root_session_id: Option<String>,
    /// See the enclosing record's documentation.
    pub source_size: i64,
    /// See the enclosing record's documentation.
    pub snapshot_size: i64,
    /// See the enclosing record's documentation.
    pub sha256: String,
    /// See the enclosing record's documentation.
    pub append_count: i64,
    /// See the enclosing record's documentation.
    pub append_max_id: BigUint,
    /// See the enclosing record's documentation.
    pub append_prefix_sha256: String,
    /// See the enclosing record's documentation.
    pub semantic_identity_mode: String,
    /// See the enclosing record's documentation.
    pub semantic_sha256: String,
    /// See the enclosing record's documentation.
    pub semantic_complete_bytes: i64,
    /// See the enclosing record's documentation.
    pub canonical_semantic_sha256: Option<String>,
    /// See the enclosing record's documentation.
    pub canonical_semantic_complete_bytes: Option<i64>,
    /// See the enclosing record's documentation.
    pub semantic_alias_baseline_path: Option<String>,
    /// See the enclosing record's documentation.
    pub semantic_baseline_path: Option<String>,
    /// See the enclosing record's documentation.
    pub source_state: String,
    /// See the enclosing record's documentation.
    pub task_source_ordinal: Option<i64>,
    /// See the enclosing record's documentation.
    pub auxiliary: AuxiliaryStatus,
    /// See the enclosing record's documentation.
    pub task_projection: Option<TaskProjection>,
    /// See the enclosing record's documentation.
    pub captured_at: String,
    /// See the enclosing record's documentation.
    pub append_prefix_override: Option<AppendPrefixOverride>,
    /// See the enclosing record's documentation.
    pub provider_schema_version: Option<i64>,
}

const COPY_LEGACY_FIELDS: &[&str] = &[
    "source_path",
    "snapshot_path",
    "kind",
    "owner_session_id",
    "source_size",
    "snapshot_size",
    "sha256",
    "append_count",
    "append_max_id",
    "append_prefix_sha256",
    "semantic_identity_mode",
    "semantic_sha256",
    "semantic_complete_bytes",
    "semantic_baseline_path",
    "source_state",
    "task_source_ordinal",
    "auxiliary",
    "task_projection",
    "captured_at",
];

impl SourceCopy {
    /// See the enclosing record's documentation.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert(
            "source_path".into(),
            Value::String(self.source_path.clone()),
        );
        out.insert(
            "snapshot_path".into(),
            Value::String(self.snapshot_path.clone()),
        );
        out.insert("kind".into(), Value::String(self.kind.clone()));
        out.insert(
            "owner_session_id".into(),
            Value::String(self.owner_session_id.clone()),
        );
        out.insert(
            "lineage_root_session_id".into(),
            self.lineage_root_session_id
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert("source_size".into(), self.source_size.into());
        out.insert("snapshot_size".into(), self.snapshot_size.into());
        out.insert("sha256".into(), Value::String(self.sha256.clone()));
        out.insert("append_count".into(), self.append_count.into());
        out.insert(
            "append_max_id".into(),
            JsonNumber::from_decimal(&self.append_max_id),
        );
        out.insert(
            "append_prefix_sha256".into(),
            Value::String(self.append_prefix_sha256.clone()),
        );
        out.insert(
            "semantic_identity_mode".into(),
            Value::String(self.semantic_identity_mode.clone()),
        );
        out.insert(
            "semantic_sha256".into(),
            Value::String(self.semantic_sha256.clone()),
        );
        out.insert(
            "semantic_complete_bytes".into(),
            self.semantic_complete_bytes.into(),
        );
        out.insert(
            "canonical_semantic_sha256".into(),
            self.canonical_semantic_sha256
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "canonical_semantic_complete_bytes".into(),
            self.canonical_semantic_complete_bytes
                .map_or(Value::Null, Value::from),
        );
        out.insert(
            "semantic_alias_baseline_path".into(),
            self.semantic_alias_baseline_path
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "semantic_baseline_path".into(),
            self.semantic_baseline_path
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "source_state".into(),
            Value::String(self.source_state.clone()),
        );
        out.insert(
            "task_source_ordinal".into(),
            self.task_source_ordinal.map_or(Value::Null, Value::from),
        );
        out.insert("auxiliary".into(), self.auxiliary.to_json());
        out.insert(
            "task_projection".into(),
            self.task_projection
                .as_ref()
                .map_or(Value::Null, TaskProjection::to_json),
        );
        out.insert(
            "captured_at".into(),
            Value::String(self.captured_at.clone()),
        );
        out.insert(
            "append_prefix_override".into(),
            self.append_prefix_override
                .as_ref()
                .map_or(Value::Null, AppendPrefixOverride::to_json),
        );
        out.insert(
            "provider_schema_version".into(),
            self.provider_schema_version
                .map_or(Value::Null, Value::from),
        );
        Value::Object(out)
    }

    /// See the enclosing record's documentation.
    pub fn from_json(
        raw: &Map<String, Value>,
        where_: &str,
        manifest_schema_version: i64,
    ) -> Result<SourceCopy, String> {
        let source_path =
            required_string(raw.get("source_path"), &format!("{where_}.source_path"))?;
        let snapshot_path =
            required_string(raw.get("snapshot_path"), &format!("{where_}.snapshot_path"))?;
        safe_relative(&source_path)?;
        safe_relative(&snapshot_path)?;
        if manifest_schema_version == 2 {
            let mut lineage: Vec<&str> = COPY_LEGACY_FIELDS.to_vec();
            lineage.push("lineage_root_session_id");
            let mut canonical: Vec<&str> = lineage.clone();
            canonical.extend([
                "canonical_semantic_sha256",
                "canonical_semantic_complete_bytes",
            ]);
            let mut alias: Vec<&str> = canonical.clone();
            alias.push("semantic_alias_baseline_path");
            let mut override_fields: Vec<&str> = alias.clone();
            override_fields.push("append_prefix_override");
            let mut current: Vec<&str> = override_fields.clone();
            current.push("provider_schema_version");
            if !exact_field_sets(
                raw,
                &[
                    COPY_LEGACY_FIELDS,
                    &lineage,
                    &canonical,
                    &alias,
                    &override_fields,
                    &current,
                ],
            ) {
                require_exact_keys(raw, &current, where_)?;
            }
        }
        let kind = required_string(raw.get("kind"), &format!("{where_}.kind"))?;
        if kind != "session" && kind != "task" {
            return Err(format!(
                "{where_}.kind: unsupported Orc database kind {}",
                py_repr_str(&kind)
            ));
        }
        let sha256_text = required_string(raw.get("sha256"), &format!("{where_}.sha256"))?;
        let append_digest = required_string(
            raw.get("append_prefix_sha256"),
            &format!("{where_}.append_prefix_sha256"),
        )?;
        sha256_string(
            Some(&Value::String(sha256_text.clone())),
            &format!("{where_}.sha256"),
        )?;
        sha256_string(
            Some(&Value::String(append_digest.clone())),
            &format!("{where_}.append_prefix_sha256"),
        )?;
        let auxiliary: AuxiliaryStatus;
        let task_projection: Option<TaskProjection>;
        let semantic_identity_mode: String;
        let semantic_sha256: String;
        let semantic_complete_bytes: i64;
        let canonical_semantic_sha256: Option<String>;
        let canonical_semantic_complete_bytes: Option<i64>;
        let semantic_alias_baseline_path: Option<String>;
        let semantic_baseline_path: Option<String>;
        let source_state: String;
        let task_source_ordinal: Option<i64>;
        let append_prefix_override: Option<AppendPrefixOverride>;
        let provider_schema_version: Option<i64>;
        if manifest_schema_version == 1 {
            if source_path != snapshot_path {
                return Err(format!(
                    "{where_}: schema-v1 source and snapshot paths must match"
                ));
            }
            auxiliary = AuxiliaryStatus {
                policy: AUXILIARY_POLICY_LEGACY.to_string(),
                message_count: nonnegative_integer(
                    raw.get("auxiliary_count"),
                    &format!("{where_}.auxiliary_count"),
                )?,
                message_sha256: sha256_string(
                    raw.get("auxiliary_prefix_sha256"),
                    &format!("{where_}.auxiliary_prefix_sha256"),
                )?,
                stable_spawn_count: 0,
                stable_spawn_sha256: empty_sha256(),
                rewrite_count: 0,
                last_rewrite_at: None,
                degraded: false,
                degradation_reason: None,
            };
            task_projection = None;
            semantic_identity_mode = SEMANTIC_IDENTITY_LEGACY.to_string();
            semantic_sha256 = sha256_text.clone();
            semantic_complete_bytes =
                nonnegative_integer(raw.get("snapshot_size"), &format!("{where_}.snapshot_size"))?;
            canonical_semantic_sha256 = None;
            canonical_semantic_complete_bytes = None;
            semantic_alias_baseline_path = None;
            semantic_baseline_path = Some(snapshot_path.clone());
            source_state = SOURCE_STATE_LIVE.to_string();
            task_source_ordinal = if kind == "task" { Some(0) } else { None };
            append_prefix_override = None;
            provider_schema_version = None;
        } else if manifest_schema_version == 2 {
            let expected_snapshot_path = snapshot_object_relative(&sha256_text)?;
            if snapshot_path != expected_snapshot_path {
                return Err(format!(
                    "{where_}.snapshot_path: expected content-addressed path {}",
                    py_repr_str(&expected_snapshot_path)
                ));
            }
            auxiliary = AuxiliaryStatus::from_json(
                mapping(raw.get("auxiliary"), &format!("{where_}.auxiliary"))?,
                &format!("{where_}.auxiliary"),
            )?;
            task_projection = match raw.get("task_projection") {
                None | Some(Value::Null) => None,
                Some(other) => Some(TaskProjection::from_json(
                    mapping(Some(other), &format!("{where_}.task_projection"))?,
                    &format!("{where_}.task_projection"),
                )?),
            };
            semantic_sha256 = sha256_string(
                raw.get("semantic_sha256"),
                &format!("{where_}.semantic_sha256"),
            )?;
            semantic_identity_mode = required_string(
                raw.get("semantic_identity_mode"),
                &format!("{where_}.semantic_identity_mode"),
            )?;
            if semantic_identity_mode != SEMANTIC_IDENTITY_LEGACY
                && semantic_identity_mode != SEMANTIC_IDENTITY_DETERMINISTIC
            {
                return Err(format!(
                    "{where_}.semantic_identity_mode: unsupported mode {}",
                    py_repr_str(&semantic_identity_mode)
                ));
            }
            semantic_complete_bytes = nonnegative_integer(
                raw.get("semantic_complete_bytes"),
                &format!("{where_}.semantic_complete_bytes"),
            )?;
            canonical_semantic_sha256 = match raw.get("canonical_semantic_sha256") {
                None | Some(Value::Null) => None,
                Some(other) => Some(sha256_string(
                    Some(other),
                    &format!("{where_}.canonical_semantic_sha256"),
                )?),
            };
            canonical_semantic_complete_bytes = match raw.get("canonical_semantic_complete_bytes") {
                None | Some(Value::Null) => None,
                Some(other) => Some(nonnegative_integer(
                    Some(other),
                    &format!("{where_}.canonical_semantic_complete_bytes"),
                )?),
            };
            if canonical_semantic_sha256.is_none() != canonical_semantic_complete_bytes.is_none() {
                return Err(format!(
                    "{where_}: canonical semantic identity must be complete"
                ));
            }
            semantic_alias_baseline_path = optional_string(
                raw.get("semantic_alias_baseline_path"),
                &format!("{where_}.semantic_alias_baseline_path"),
            )?;
            if let Some(held) = &semantic_alias_baseline_path {
                safe_relative(held)?;
            }
            semantic_baseline_path = optional_string(
                raw.get("semantic_baseline_path"),
                &format!("{where_}.semantic_baseline_path"),
            )?;
            if let Some(held) = &semantic_baseline_path {
                safe_relative(held)?;
            }
            source_state =
                required_string(raw.get("source_state"), &format!("{where_}.source_state"))?;
            if source_state != SOURCE_STATE_LIVE && source_state != SOURCE_STATE_DETACHED {
                return Err(format!(
                    "{where_}.source_state: unsupported state {}",
                    py_repr_str(&source_state)
                ));
            }
            task_source_ordinal = match raw.get("task_source_ordinal") {
                None | Some(Value::Null) => None,
                Some(other) => Some(nonnegative_integer(
                    Some(other),
                    &format!("{where_}.task_source_ordinal"),
                )?),
            };
            append_prefix_override = match raw.get("append_prefix_override") {
                None | Some(Value::Null) => None,
                Some(other) => Some(AppendPrefixOverride::from_json(
                    mapping(Some(other), &format!("{where_}.append_prefix_override"))?,
                    &format!("{where_}.append_prefix_override"),
                )?),
            };
            if let Some(held) = &append_prefix_override {
                if held.source_path != source_path {
                    return Err(format!(
                        "{where_}.append_prefix_override.source_path: recorded override belongs to {}",
                        py_repr_str(&held.source_path)
                    ));
                }
            }
            if let Some(held) = &append_prefix_override {
                if held.superseded_snapshot_path == snapshot_path {
                    return Err(format!(
                        "{where_}.append_prefix_override.superseded_snapshot_path: the pre-rewrite snapshot cannot be the current snapshot"
                    ));
                }
            }
            provider_schema_version = match raw.get("provider_schema_version") {
                None | Some(Value::Null) => None,
                Some(other) => Some(positive_integer(
                    Some(other),
                    &format!("{where_}.provider_schema_version"),
                )?),
            };
        } else {
            return Err(format!(
                "{where_}: unsupported Orc manifest schema {manifest_schema_version}"
            ));
        }
        if manifest_schema_version == 2
            && ((kind == "session" && auxiliary.policy != AUXILIARY_POLICY_STABLE_SPAWNS)
                || (kind == "task" && auxiliary.policy != AUXILIARY_POLICY_NOT_APPLICABLE)
                || (kind == "session" && task_projection.is_some())
                || (kind == "task" && task_projection.is_none())
                || (kind == "session" && source_state != SOURCE_STATE_LIVE)
                || (kind == "session" && task_source_ordinal.is_some())
                || (kind == "task" && task_source_ordinal.is_none())
                || (kind == "task"
                    && !matches!(raw.get("lineage_root_session_id"), None | Some(Value::Null)))
                || (semantic_identity_mode == SEMANTIC_IDENTITY_LEGACY
                    && semantic_baseline_path.is_none())
                || (semantic_identity_mode == SEMANTIC_IDENTITY_DETERMINISTIC
                    && semantic_baseline_path.is_some())
                || (kind == "task" && append_prefix_override.is_some()))
        {
            return Err(format!(
                "{where_}: invalid schema-v2 projections for {kind} source"
            ));
        }
        Ok(SourceCopy {
            source_path,
            snapshot_path,
            kind,
            owner_session_id: required_string(
                raw.get("owner_session_id"),
                &format!("{where_}.owner_session_id"),
            )?,
            lineage_root_session_id: optional_string(
                raw.get("lineage_root_session_id"),
                &format!("{where_}.lineage_root_session_id"),
            )?,
            source_size: nonnegative_integer(
                raw.get("source_size"),
                &format!("{where_}.source_size"),
            )?,
            snapshot_size: nonnegative_integer(
                raw.get("snapshot_size"),
                &format!("{where_}.snapshot_size"),
            )?,
            sha256: sha256_text,
            append_count: nonnegative_integer(
                raw.get("append_count"),
                &format!("{where_}.append_count"),
            )?,
            append_max_id: nonnegative_big(
                raw.get("append_max_id"),
                &format!("{where_}.append_max_id"),
            )?,
            append_prefix_sha256: append_digest,
            semantic_identity_mode,
            semantic_sha256,
            semantic_complete_bytes,
            canonical_semantic_sha256,
            canonical_semantic_complete_bytes,
            semantic_alias_baseline_path: if manifest_schema_version == 2 {
                semantic_alias_baseline_path
            } else {
                None
            },
            semantic_baseline_path,
            source_state,
            task_source_ordinal,
            auxiliary,
            task_projection,
            captured_at: required_string(raw.get("captured_at"), &format!("{where_}.captured_at"))?,
            append_prefix_override,
            provider_schema_version,
        })
    }
}

// ---------------------------------------------------------------- SQLite layer

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::compressed::{is_compressed, read_source_bytes, resolve_source, source_exists};
use rusqlite::Connection;
use serde_json::Value as JsonValue;

/// One SQLite value, in SQLite's own type classes.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    /// NULL.
    Null,
    /// INTEGER.
    Int(i64),
    /// REAL.
    Real(f64),
    /// TEXT.
    Text(String),
    /// BLOB.
    Blob(Vec<u8>),
}

impl SqlValue {
    fn from_rusqlite(value: rusqlite::types::Value) -> SqlValue {
        use rusqlite::types::Value as Held;
        match value {
            Held::Null => SqlValue::Null,
            Held::Integer(held) => SqlValue::Int(held),
            Held::Real(held) => SqlValue::Real(held),
            Held::Text(held) => SqlValue::Text(held),
            Held::Blob(held) => SqlValue::Blob(held),
        }
    }
}

/// CPython `float.hex()`.
pub fn py_float_hex(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_string();
    }
    if value.is_infinite() {
        return if value < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    let bits = value.to_bits();
    let sign = if bits >> 63 == 1 { "-" } else { "" };
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    let fraction = bits & ((1u64 << 52) - 1);
    if exponent == 0 && fraction == 0 {
        return format!("{sign}0x0.0p+0");
    }
    if exponent == 0 {
        return format!("{sign}0x0.{fraction:013x}p-1022");
    }
    format!("{sign}0x1.{fraction:013x}p{:+}", exponent - 1023)
}

/// Feed one value to a prefix digest, tagged the way the
/// oracle's digest tags it.
pub fn update_digest(digest: &mut sha2::Sha256, value: &SqlValue) {
    match value {
        SqlValue::Null => digest.update(b"N"),
        SqlValue::Int(held) => {
            let payload = held.to_string();
            digest.update(b"I");
            digest.update((payload.len() as u64).to_be_bytes());
            digest.update(payload.as_bytes());
        }
        SqlValue::Real(held) => {
            let payload = py_float_hex(*held);
            digest.update(b"F");
            digest.update((payload.len() as u64).to_be_bytes());
            digest.update(payload.as_bytes());
        }
        SqlValue::Text(held) => {
            digest.update(b"S");
            digest.update((held.len() as u64).to_be_bytes());
            digest.update(held.as_bytes());
        }
        SqlValue::Blob(held) => {
            digest.update(b"B");
            digest.update((held.len() as u64).to_be_bytes());
            digest.update(held);
        }
    }
}

/// The same tagging for a JSON value (stable-spawn and
/// semantic digests).
pub fn update_digest_json(digest: &mut sha2::Sha256, value: &JsonValue) {
    match value {
        JsonValue::Null => digest.update(b"N"),
        JsonValue::Bool(flag) => digest.update(if *flag { b"I1" } else { b"I0" }),
        JsonValue::Number(number) => {
            if let Some(held) = number
                .as_i64()
                .or_else(|| number.as_u64().and_then(|v| i64::try_from(v).ok()))
            {
                update_digest(digest, &SqlValue::Int(held));
            } else if let Some(held) = number.as_f64() {
                update_digest(digest, &SqlValue::Real(held));
            }
        }
        JsonValue::String(held) => update_digest(digest, &SqlValue::Text(held.clone())),
        _ => {}
    }
}

/// Run a query, digesting every row.
pub fn query_digest(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
) -> Result<(i64, String), String> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("SQLite digest query: {error}"))?;
    let params: Vec<&dyn rusqlite::ToSql> = parameters
        .iter()
        .map(|value| value as &dyn rusqlite::ToSql)
        .collect();
    let mut rows = statement
        .query(params.as_slice())
        .map_err(|error| format!("SQLite digest query: {error}"))?;
    let mut digest = sha2::Sha256::new();
    let mut count = 0i64;
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                digest.update(b"R");
                for value in row_values(row) {
                    update_digest(&mut digest, &value);
                }
                count += 1;
            }
            Ok(None) => break,
            Err(error) => return Err(format!("SQLite digest query: {error}")),
        }
    }
    Ok((count, hex_digest(&digest)))
}

impl rusqlite::ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, Value as Held};
        Ok(match self {
            SqlValue::Null => ToSqlOutput::Owned(Held::Null),
            SqlValue::Int(held) => ToSqlOutput::from(*held),
            SqlValue::Real(held) => ToSqlOutput::from(*held),
            SqlValue::Text(held) => ToSqlOutput::from(held.as_str()),
            SqlValue::Blob(held) => ToSqlOutput::from(held.as_slice()),
        })
    }
}

/// Hex of a finished digest.
pub fn hex_digest(digest: &sha2::Sha256) -> String {
    digest
        .clone()
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Digest of a file's bytes.
pub fn sha256_file(path: &Path) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|error| crate::json::os_error(&error, path))?;
    Ok(sha256_hex(&data))
}

type DecompressedCache = (PathBuf, Vec<(String, i64, i64, PathBuf)>);

static DECOMPRESSED: Mutex<Option<DecompressedCache>> = Mutex::new(None);

fn decompressed_database(physical: &Path) -> Result<PathBuf, String> {
    let metadata =
        std::fs::metadata(physical).map_err(|error| crate::json::os_error(&error, physical))?;
    use std::os::unix::fs::MetadataExt as _;
    let key = (
        physical.display().to_string(),
        metadata.size() as i64,
        metadata.mtime() * 1_000_000_000 + metadata.mtime_nsec(),
    );
    let mut guard = DECOMPRESSED.lock().unwrap_or_else(|held| held.into_inner());
    if guard.is_none() {
        let root =
            std::env::temp_dir().join(format!("wrkviz-orc-decompressed-{}", std::process::id()));
        *guard = Some((root, Vec::new()));
    }
    let (root, cache) = guard.as_mut().expect("decompressed root");
    if let Some(cached) = cache
        .iter()
        .find(|(a, b, c, _)| (a.as_str(), *b, *c) == (key.0.as_str(), key.1, key.2))
        .map(|(_, _, _, cached)| cached.clone())
    {
        if cached.is_file() {
            return Ok(cached);
        }
    }
    let data = read_source_bytes(physical)?;
    std::fs::create_dir_all(root.as_path()).map_err(|error| crate::json::os_error(&error, root))?;
    let name = physical
        .file_name()
        .map(|held| held.to_string_lossy().into_owned())
        .unwrap_or_default();
    let logical = name.strip_suffix(".zst").unwrap_or(&name).to_string();
    let target = root.join(format!(".decompressed-{}-{logical}", cache.len()));
    std::fs::write(&target, &data).map_err(|error| crate::json::os_error(&error, &target))?;
    cache.push((key.0, key.1, key.2, target.clone()));
    Ok(target)
}

/// Remove decompressed database copies (process end).
pub fn cleanup_decompressed() {
    let mut guard = DECOMPRESSED.lock().unwrap_or_else(|held| held.into_inner());
    if let Some((root, _)) = guard.take() {
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The byte size of a live database as SQLite sees it.
pub fn logical_source_size(path: &Path) -> Result<i64, String> {
    if let Some(physical) = resolve_source(path) {
        if physical != path && is_compressed(&physical) {
            let copy = decompressed_database(&physical)?;
            return std::fs::metadata(&copy)
                .map(|meta| {
                    use std::os::unix::fs::MetadataExt as _;
                    meta.size() as i64
                })
                .map_err(|error| crate::json::os_error(&error, &copy));
        }
    }
    std::fs::metadata(path)
        .map(|meta| {
            use std::os::unix::fs::MetadataExt as _;
            meta.size() as i64
        })
        .map_err(|error| crate::json::os_error(&error, path))
}

fn percent_encode_path(path: &Path) -> String {
    let mut out = String::new();
    for byte in path.as_os_str().as_encoded_bytes() {
        let keep = byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'.' | b'-' | b'~');
        if keep {
            out.push(*byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Open an Orc SQLite source read-only.
pub fn read_only(path: &Path) -> Result<Connection, String> {
    let mut effective = path.to_path_buf();
    if let Some(physical) = resolve_source(path) {
        if physical != path && is_compressed(&physical) {
            let is_file = physical.is_file();
            let is_symlink = std::fs::symlink_metadata(&physical)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if !is_file || is_symlink {
                return Err(format!(
                    "Orc SQLite source is missing or not a regular file: {}",
                    path.display()
                ));
            }
            effective = decompressed_database(&physical).map_err(|error| {
                format!(
                    "cannot decompress Orc SQLite source {}: {error}",
                    physical.display()
                )
            })?;
        }
    }
    let is_symlink = std::fs::symlink_metadata(&effective)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink || !effective.is_file() {
        return Err(format!(
            "Orc SQLite source is missing or not a regular file: {}",
            effective.display()
        ));
    }
    let resolved = std::fs::canonicalize(&effective)
        .map_err(|error| crate::json::os_error(&error, &effective))?;
    let parent_name = resolved
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let grandparent_name = resolved
        .parent()
        .and_then(|parent| parent.parent())
        .and_then(|parent| parent.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let file_name = resolved
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let immutable = grandparent_name == SNAPSHOT_OBJECT_ROOT
        && parent_name.len() == 2
        && parent_name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && file_name.len() == 67
        && file_name.ends_with(".db")
        && file_name[..64]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let query = if immutable {
        "?mode=ro&immutable=1"
    } else {
        "?mode=ro"
    };
    let uri = format!("file://{}{query}", percent_encode_path(&resolved));
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI;
    let connection = Connection::open_with_flags(&uri, flags).map_err(|error| {
        format!(
            "cannot open Orc SQLite source read-only at {}: {error}",
            effective.display()
        )
    })?;
    connection
        .execute_batch("PRAGMA query_only = ON")
        .map_err(|error| {
            format!(
                "cannot open Orc SQLite source read-only at {}: {error}",
                effective.display()
            )
        })?;
    Ok(connection)
}

/// Every table name in a database.
pub fn table_names(connection: &Connection, where_: &str) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|error| format!("{where_}: {error}"))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| format!("{where_}: {error}"))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(|error| format!("{where_}: {error}"))?);
    }
    Ok(names)
}

/// Refuse unless every named table exists.
pub fn require_tables(connection: &Connection, names: &[&str], where_: &str) -> Result<(), String> {
    let found = table_names(connection, where_)?;
    let mut missing: Vec<&str> = names
        .iter()
        .filter(|name| !found.iter().any(|held| held == *name))
        .copied()
        .collect();
    missing.sort();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{where_}: missing required tables: {}",
            missing.join(", ")
        ))
    }
}

/// Which table holds the session transcript.
pub fn session_storage_table(connection: &Connection, where_: &str) -> Result<String, String> {
    let tables = table_names(connection, where_)?;
    if tables.iter().any(|held| held == "messages") {
        return Ok("messages".to_string());
    }
    if tables.iter().any(|held| held == "conversation_state") {
        return Ok("content_blocks".to_string());
    }
    Err(format!(
        "{where_}: missing both modern messages and legacy conversation_state"
    ))
}

/// One required row of a query.
pub fn query_one(
    connection: &Connection,
    sql: &str,
    where_: &str,
) -> Result<Vec<SqlValue>, String> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("{where_}: {error}"))?;
    let mut rows = statement
        .query([])
        .map_err(|error| format!("{where_}: {error}"))?;
    match rows.next() {
        Ok(Some(row)) => Ok(row_values(row)),
        Ok(None) => Err(format!("{where_}: expected one row")),
        Err(error) => Err(format!("{where_}: {error}")),
    }
}

/// All values of one result row.
pub fn row_values(row: &rusqlite::Row<'_>) -> Vec<SqlValue> {
    let mut values = Vec::new();
    let mut index = 0;
    while let Ok(value_ref) = row.get_ref(index) {
        values.push(SqlValue::from_rusqlite(rusqlite::types::Value::from(
            value_ref,
        )));
        index += 1;
    }
    values
}

/// Run a query, returning the raw SQLite error text.
pub fn query_all_inner(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
) -> Result<Vec<Vec<SqlValue>>, String> {
    let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
    let params: Vec<&dyn rusqlite::ToSql> = parameters
        .iter()
        .map(|value| value as &dyn rusqlite::ToSql)
        .collect();
    let mut rows = statement
        .query(params.as_slice())
        .map_err(|error| error.to_string())?;
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => out.push(row_values(row)),
            Ok(None) => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(out)
}

/// Run a query to completion, collecting rows.
pub fn query_all(
    connection: &Connection,
    sql: &str,
    parameters: &[SqlValue],
    where_: &str,
) -> Result<Vec<Vec<SqlValue>>, String> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| format!("{where_}: {error}"))?;
    let params: Vec<&dyn rusqlite::ToSql> = parameters
        .iter()
        .map(|value| value as &dyn rusqlite::ToSql)
        .collect();
    let mut rows = statement
        .query(params.as_slice())
        .map_err(|error| format!("{where_}: {error}"))?;
    let mut out = Vec::new();
    loop {
        match rows.next() {
            Ok(Some(row)) => out.push(row_values(row)),
            Ok(None) => break,
            Err(error) => return Err(format!("{where_}: {error}")),
        }
    }
    Ok(out)
}

/// An SQLite integer cell, with the oracle's wording.
pub fn cell_integer(value: &SqlValue, where_: &str) -> Result<i64, String> {
    match value {
        SqlValue::Int(held) => Ok(*held),
        _ => Err(format!("{where_}: expected an integer")),
    }
}

/// An SQLite non-negative integer cell.
pub fn cell_nonnegative(value: &SqlValue, where_: &str) -> Result<i64, String> {
    let held = cell_integer(value, where_)
        .map_err(|_| format!("{where_}: expected a non-negative integer"))?;
    if held < 0 {
        return Err(format!("{where_}: expected a non-negative integer"));
    }
    Ok(held)
}

/// An SQLite required-string cell.
pub fn cell_string(value: &SqlValue, where_: &str) -> Result<String, String> {
    match value {
        SqlValue::Text(held) if !held.is_empty() => Ok(held.clone()),
        _ => Err(format!("{where_}: expected a non-empty string")),
    }
}

/// An SQLite optional-string cell (null and empty read as
/// absent).
pub fn cell_optional_string(value: &SqlValue, where_: &str) -> Result<Option<String>, String> {
    match value {
        SqlValue::Null => Ok(None),
        SqlValue::Text(held) if held.is_empty() => Ok(None),
        SqlValue::Text(held) => Ok(Some(held.clone())),
        _ => Err(format!("{where_}: expected a string or null")),
    }
}

/// An SQLite string-valued cell (any text, including empty).
pub fn cell_text(value: &SqlValue, where_: &str) -> Result<String, String> {
    match value {
        SqlValue::Text(held) => Ok(held.clone()),
        _ => Err(format!("{where_}: expected a string")),
    }
}

/// An ISO timestamp cell in epoch milliseconds (naive
/// reads as UTC).
pub fn iso_ms(value: &SqlValue, where_: &str) -> Result<i64, String> {
    let text = cell_string(value, where_)?;
    let normalized = text
        .strip_suffix('Z')
        .map(|head| format!("{head}+00:00"))
        .unwrap_or_else(|| text.clone());
    use agent_harness::claude_records::{timestamp_ms, TimestampError};
    let candidate = JsonValue::String(normalized.clone());
    match timestamp_ms(&candidate) {
        Ok(Some(ms)) => Ok(ms),
        Ok(None) => Err(format!(
            "{where_}: invalid ISO timestamp {}",
            py_repr_str(&text)
        )),
        Err(TimestampError::NoTimezone) => {
            let with_zone = if normalized.len() == 10 {
                format!("{normalized}T00:00:00+00:00")
            } else {
                format!("{normalized}+00:00")
            };
            match timestamp_ms(&JsonValue::String(with_zone)) {
                Ok(Some(ms)) => Ok(ms),
                _ => Err(format!(
                    "{where_}: invalid ISO timestamp {}",
                    py_repr_str(&text)
                )),
            }
        }
        Err(TimestampError::Invalid(_)) => Err(format!(
            "{where_}: invalid ISO timestamp {}",
            py_repr_str(&text)
        )),
    }
}

/// A snapshot path joined without following symlinks.
pub fn snapshot_path(snapshot_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parts = safe_relative_parts(relative)?;
    let mut current = snapshot_root.to_path_buf();
    for (index, part) in parts.iter().enumerate() {
        if let Ok(metadata) = std::fs::symlink_metadata(&current) {
            let mode = metadata.file_type();
            if mode.is_symlink() || (index > 0 && !mode.is_dir()) || (index == 0 && !mode.is_dir())
            {
                return Err(format!(
                    "snapshot path component is a symlink or non-directory: {}",
                    current.display()
                ));
            }
        }
        current.push(part);
    }
    Ok(current)
}

/// A live source path resolved without following symlinks.
pub fn live_source_path(source_root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parts = safe_relative_parts(relative)?;
    let mut current = source_root.to_path_buf();
    let last = parts.len() - 1;
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            continue;
        };
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "symlink in Orc live source path: {}",
                current.display()
            ));
        }
        if index < last && !metadata.is_dir() {
            return Err(format!(
                "non-directory in Orc live source path: {}",
                current.display()
            ));
        }
    }
    Ok(current)
}

/// fsync one regular file without following symlinks.
pub fn fsync_regular_file(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            format!(
                "cannot open snapshot object for fsync {}: {error}",
                path.display()
            )
        })?;
    let metadata = file
        .metadata()
        .map_err(|error| crate::json::os_error(&error, path))?;
    if !metadata.is_file() {
        return Err(format!(
            "snapshot fsync target is not a regular file: {}",
            path.display()
        ));
    }
    file.sync_all()
        .map_err(|error| crate::json::os_error(&error, path))
}

/// fsync one directory.
pub fn fsync_directory(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| {
            format!(
                "cannot open snapshot directory for fsync {}: {error}",
                path.display()
            )
        })?;
    file.sync_all()
        .map_err(|error| crate::json::os_error(&error, path))
}

/// Create a snapshot parent chain, refusing symlinks.
pub fn ensure_snapshot_parent(path: &Path, snapshot_root: &Path) -> Result<(), String> {
    let root_existed = snapshot_root.exists();
    std::fs::create_dir_all(snapshot_root)
        .map_err(|error| crate::json::os_error(&error, snapshot_root))?;
    let root_symlink = std::fs::symlink_metadata(snapshot_root)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if root_symlink || !snapshot_root.is_dir() {
        return Err(format!(
            "snapshot root is a symlink or non-directory: {}",
            snapshot_root.display()
        ));
    }
    if !root_existed {
        if let Some(parent) = snapshot_root.parent() {
            fsync_directory(parent)?;
        }
    }
    let parent = path.parent().unwrap_or(snapshot_root);
    let relative = parent.strip_prefix(snapshot_root).map_err(|_| {
        format!(
            "snapshot path is outside the snapshot root: {}",
            path.display()
        )
    })?;
    let mut current = snapshot_root.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        if current.exists() {
            let symlink = std::fs::symlink_metadata(&current)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if symlink || !current.is_dir() {
                return Err(format!(
                    "snapshot path component is a symlink or non-directory: {}",
                    current.display()
                ));
            }
        } else {
            std::fs::create_dir(&current)
                .map_err(|error| crate::json::os_error(&error, &current))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(&current, std::fs::Permissions::from_mode(0o700));
            }
            if let Some(parent) = current.parent() {
                fsync_directory(parent)?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- discovery

/// One session database's immutable identity and bounds.
#[derive(Clone, Debug)]
pub struct SessionMeta {
    /// See the enclosing record's documentation.
    pub session_id: String,
    /// See the enclosing record's documentation.
    pub parent_id: Option<String>,
    /// See the enclosing record's documentation.
    pub name: String,
    /// See the enclosing record's documentation.
    pub db_name: Option<String>,
    /// See the enclosing record's documentation.
    pub created_at_ms: i64,
    /// See the enclosing record's documentation.
    pub updated_at_ms: i64,
    /// See the enclosing record's documentation.
    pub source_path: String,
}

/// One discovered source database and its possible owners.
#[derive(Clone, Debug)]
pub struct DiscoveredSource {
    /// See the enclosing record's documentation.
    pub source_path: String,
    /// See the enclosing record's documentation.
    pub kind: String,
    /// See the enclosing record's documentation.
    pub owner_candidates: Vec<String>,
    /// See the enclosing record's documentation.
    pub lineage_root_session_id: Option<String>,
    /// See the enclosing record's documentation.
    pub source_state: String,
}

/// The Orc storage schema version a database declares.
pub fn provider_schema_version(path: &Path, where_: &str) -> Result<Option<i64>, String> {
    let connection = read_only(path)?;
    let version_row: Result<Option<Vec<SqlValue>>, String> = (|| {
        let tables = table_names(&connection, where_)?;
        if !tables.iter().any(|held| held == "schema_version") {
            return Ok(None);
        }
        let mut statement = connection
            .prepare("SELECT version FROM schema_version WHERE id = 1")
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        match rows.next() {
            Ok(Some(row)) => Ok(Some(row_values(row))),
            Ok(None) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    })();
    drop(connection);
    match version_row {
        Ok(Some(row)) => {
            let value = row.first().cloned().unwrap_or(SqlValue::Null);
            match value {
                SqlValue::Int(held) if held > 0 => Ok(Some(held)),
                _ => Err(format!(
                    "{where_}: schema_version.version: expected a positive integer"
                )),
            }
        }
        Ok(None) => {
            // Distinguish absent table from a table with no
            // version row by re-checking, like the oracle's
            // two-step read.
            let connection = read_only(path)?;
            let tables = table_names(&connection, where_)?;
            if tables.iter().any(|held| held == "schema_version") {
                return Err(format!(
                    "{where_}: Orc schema_version table holds no version row"
                ));
            }
            Ok(None)
        }
        Err(error) => Err(format!(
            "{where_}: Orc schema_version table is present but unreadable: {error}"
        )),
    }
}

/// Read one session database's metadata.
pub fn session_meta(path: &Path, source_path: &str) -> Result<SessionMeta, String> {
    let connection = read_only(path)?;
    require_tables(
        &connection,
        &["session_meta", "content_blocks"],
        &path.display().to_string(),
    )?;
    session_storage_table(&connection, &path.display().to_string())?;
    let row = (|| -> Result<Vec<SqlValue>, String> {
        let mut statement = connection
            .prepare(
                "SELECT id, parent_id, name, db_name, created_at, updated_at \
                 FROM session_meta LIMIT 1",
            )
            .map_err(|error| error.to_string())?;
        let mut rows = statement.query([]).map_err(|error| error.to_string())?;
        match rows.next() {
            Ok(Some(row)) => Ok(row_values(row)),
            Ok(None) => Err("expected one row".to_string()),
            Err(error) => Err(error.to_string()),
        }
    })()
    .map_err(|error| {
        if error == "expected one row" {
            format!("{}: expected one row", path.display())
        } else {
            format!(
                "failed to inspect Orc session metadata at {}: {error}",
                path.display()
            )
        }
    })?;
    drop(connection);
    let cell = |index: usize| row.get(index).cloned().unwrap_or(SqlValue::Null);
    let session_id = safe_component(
        &cell_string(&cell(0), &format!("{}: id", path.display()))?,
        &format!("{}: id", path.display()),
    )?;
    let parent_id = cell_optional_string(&cell(1), &format!("{}: parent_id", path.display()))?;
    if let Some(parent) = &parent_id {
        safe_component(parent, &format!("{}: parent_id", path.display()))?;
    }
    let db_name = cell_optional_string(&cell(3), &format!("{}: db_name", path.display()))?;
    if let Some(name) = &db_name {
        safe_component(name, &format!("{}: db_name", path.display()))?;
    }
    Ok(SessionMeta {
        session_id,
        parent_id,
        name: cell_string(&cell(2), &format!("{}: name", path.display()))?,
        db_name,
        created_at_ms: iso_ms(&cell(4), &format!("{}: created_at", path.display()))?,
        updated_at_ms: iso_ms(&cell(5), &format!("{}: updated_at", path.display()))?,
        source_path: source_path.to_string(),
    })
}

/// Names of task databases a session lists as associated.
pub fn associated_db_names(path: &Path) -> Result<Vec<String>, String> {
    let connection = read_only(path)?;
    let tables = table_names(&connection, &path.display().to_string())?;
    if !tables.iter().any(|held| held == "associated_dbs") {
        return Ok(Vec::new());
    }
    let rows = query_all(
        &connection,
        "SELECT db_name FROM associated_dbs ORDER BY db_name",
        &[],
        &path.display().to_string(),
    )
    .map_err(|error| {
        format!(
            "failed to inspect Orc associated task databases at {}: {error}",
            path.display()
        )
    })?;
    drop(connection);
    let mut names = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let value = row.first().cloned().unwrap_or(SqlValue::Null);
        names.push(safe_component(
            &cell_string(
                &value,
                &format!("{}: associated_dbs[{index}].db_name", path.display()),
            )?,
            &format!("{}: associated_dbs[{index}].db_name", path.display()),
        )?);
    }
    Ok(names)
}

/// Task database paths for one session, each flagged
/// (path, required, is_primary).
pub fn session_task_relatives(
    database: &Path,
    meta: &SessionMeta,
) -> Result<Vec<(String, bool, bool)>, String> {
    let primary_name: &str = if meta.parent_id.is_none() {
        meta.db_name.as_deref().unwrap_or(&meta.session_id)
    } else {
        &meta.session_id
    };
    let primary_path = format!(".tg/{primary_name}.db");
    let mut required_by_path: Vec<(String, bool)> = vec![(primary_path.clone(), false)];
    for name in associated_db_names(database)? {
        let relative = format!(".tg/{name}.db");
        if !required_by_path.iter().any(|(held, _)| *held == relative) {
            required_by_path.push((relative, false));
        }
    }
    required_by_path.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(required_by_path
        .into_iter()
        .map(|(path, required)| {
            let is_primary = path == primary_path;
            (path, required, is_primary)
        })
        .collect())
}

/// Root session ids for a lineage (the root plus any
/// continuations), uniqueness-checked.
pub fn lineage_roots(
    root_session_id: &str,
    continuation_session_ids: &[String],
) -> Result<Vec<String>, String> {
    let mut roots = Vec::new();
    roots.push(safe_component(
        root_session_id,
        "Orc root or continuation session id",
    )?);
    for item in continuation_session_ids {
        roots.push(safe_component(item, "Orc root or continuation session id")?);
    }
    let mut unique = roots.clone();
    unique.sort();
    unique.dedup();
    if unique.len() != roots.len() {
        return Err("Orc root and continuation session IDs must be unique".to_string());
    }
    Ok(roots)
}

/// The parsed continuation specs for one ingest.
pub fn continuation_specs(values: &[String]) -> Result<Vec<ContinuationSpec>, String> {
    let mut specs = Vec::new();
    for (index, value) in values.iter().enumerate() {
        specs.push(ContinuationSpec::from_value(
            &JsonValue::String(value.clone()),
            &format!("continuation_specs[{index}]"),
        )?);
    }
    Ok(specs)
}

/// Where a bounded continuation starts, and when.
pub fn resolve_continuation_start(
    path: &Path,
    meta: &SessionMeta,
    spec: &ContinuationSpec,
) -> Result<(Option<(i64, i64)>, i64), String> {
    let Some(start_message_id) = &spec.start_message_id else {
        return Ok((None, meta.created_at_ms));
    };
    let connection = read_only(path)?;
    if session_storage_table(&connection, &path.display().to_string())? != "messages" {
        return Err(format!(
            "bounded Orc continuation {} requires the append-only messages table",
            py_repr_str(&spec.session_id)
        ));
    }
    let rows = query_all(
        &connection,
        "SELECT id, created_at_ms, message_json FROM messages ORDER BY id",
        &[],
        &path.display().to_string(),
    )
    .map_err(|error| {
        format!(
            "failed to resolve Orc continuation start in {}: {error}",
            path.display()
        )
    })?;
    drop(connection);
    let mut matches: Vec<(i64, i64)> = Vec::new();
    for row in &rows {
        let cell = |index: usize| row.get(index).cloned().unwrap_or(SqlValue::Null);
        let source_line = cell_nonnegative(&cell(0), &format!("{}: message id", path.display()))?;
        let timestamp_ms = cell_nonnegative(
            &cell(1),
            &format!("{}: message created_at_ms", path.display()),
        )?;
        let raw_json = cell_string(&cell(2), &format!("{}: message_json", path.display()))?;
        let decoded: JsonValue = serde_json::from_str(&raw_json).map_err(|error| {
            format!(
                "{}: invalid message_json at row {source_line}: {error}",
                path.display()
            )
        })?;
        let message = decoded.as_object().ok_or_else(|| {
            format!(
                "{}: message_json[{source_line}]: expected an object",
                path.display()
            )
        })?;
        if message.get("id").and_then(JsonValue::as_str) == Some(start_message_id.as_str()) {
            matches.push((source_line, timestamp_ms));
        }
    }
    if matches.len() != 1 || matches[0].1 <= 0 {
        return Err(format!(
            "bounded Orc continuation {} start message {} resolved to {} valid rows",
            py_repr_str(&spec.session_id),
            py_repr_str(start_message_id),
            matches.len()
        ));
    }
    Ok((Some(matches[0]), matches[0].1))
}

/// Whether a session shows anything at or after an instant.
pub fn session_has_activity_at_or_after(
    source_root: &Path,
    path: &Path,
    meta: &SessionMeta,
    lineage_root_session_id: &str,
    start_at_ms: i64,
) -> Result<bool, String> {
    let _ = lineage_root_session_id;
    if meta.created_at_ms >= start_at_ms {
        return Ok(true);
    }
    {
        let connection = read_only(path)?;
        let table = session_storage_table(&connection, &path.display().to_string())?;
        let rows = query_all(
            &connection,
            &format!("SELECT MAX(created_at_ms) FROM {table}"),
            &[],
            &path.display().to_string(),
        )?;
        drop(connection);
        if let Some(SqlValue::Int(held)) = rows.first().and_then(|row| row.first()) {
            if *held >= start_at_ms {
                return Ok(true);
            }
        }
    }
    if auxiliary_observation(path, meta)?
        .stable_spawns
        .iter()
        .any(|spawn| spawn.timestamp_ms >= start_at_ms)
    {
        return Ok(true);
    }
    for (relative, _, _) in session_task_relatives(path, meta)? {
        let task_path = live_source_path(source_root, &relative)?;
        if !source_exists(&task_path) {
            continue;
        }
        let connection = read_only(&task_path)?;
        require_tables(
            &connection,
            &["task_notes"],
            &task_path.display().to_string(),
        )?;
        let rows = query_all(
            &connection,
            "SELECT MAX(created_at) FROM task_notes",
            &[],
            &task_path.display().to_string(),
        )?;
        drop(connection);
        if let Some(value) = rows.first().and_then(|row| row.first()) {
            if !matches!(value, SqlValue::Null)
                && iso_ms(
                    value,
                    &format!("{}: latest task note created_at", task_path.display()),
                )? >= start_at_ms
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Sources below one Orc installation root.
pub fn discover_sources(
    source_root: &Path,
    root_session_id: &str,
) -> Result<Vec<DiscoveredSource>, String> {
    let root_id = safe_component(root_session_id, "root session id")?;
    let sessions_root = live_source_path(source_root, ".orc/sessions")?;
    let root_is_symlink = std::fs::symlink_metadata(&sessions_root)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if !sessions_root.is_dir() || root_is_symlink {
        return Err(format!(
            "missing Orc sessions directory: {}",
            sessions_root.display()
        ));
    }
    let index_path = live_source_path(source_root, ".orc/index.db")?;
    let mut selected: Option<Vec<String>> = None;
    let mut index_parents: Option<Vec<(String, Option<String>)>> = None;
    if source_exists(&index_path) {
        let connection = read_only(&index_path)?;
        require_tables(
            &connection,
            &["sessions"],
            &index_path.display().to_string(),
        )?;
        let rows = query_all(
            &connection,
            "SELECT id, parent_id FROM sessions ORDER BY id",
            &[],
            &index_path.display().to_string(),
        )
        .map_err(|error| {
            format!(
                "failed to inspect Orc session index at {}: {error}",
                index_path.display()
            )
        })?;
        drop(connection);
        let mut parents: Vec<(String, Option<String>)> = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
            let session_id = safe_component(
                &cell_string(
                    &cell(0),
                    &format!("{}: sessions[{index}].id", index_path.display()),
                )?,
                &format!("{}: sessions[{index}].id", index_path.display()),
            )?;
            let parent = cell_optional_string(
                &cell(1),
                &format!("{}: sessions[{index}].parent_id", index_path.display()),
            )?;
            parents.push((session_id, parent));
        }
        if !parents.iter().any(|(held, _)| *held == root_id) {
            return Err(format!(
                "root Orc session {} is absent from {}",
                py_repr_str(&root_id),
                index_path.display()
            ));
        }
        let mut chosen: Vec<String> = vec![root_id.clone()];
        let mut changed = true;
        while changed {
            changed = false;
            for (session_id, parent) in &parents {
                if let Some(parent) = parent {
                    if chosen.contains(parent) && !chosen.contains(session_id) {
                        chosen.push(session_id.clone());
                        changed = true;
                    }
                }
            }
        }
        selected = Some(chosen);
        index_parents = Some(parents);
    }
    let mut metas: Vec<SessionMeta> = Vec::new();
    let mut candidate_names: Vec<String> = match &selected {
        Some(chosen) => {
            let mut held = chosen.clone();
            held.sort();
            held
        }
        None => vec![root_id.clone()],
    };
    candidate_names.dedup();
    for session_name in &candidate_names {
        let session_dir = sessions_root.join(session_name);
        let dir_symlink = std::fs::symlink_metadata(&session_dir)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if dir_symlink || !session_dir.is_dir() {
            if selected.is_some() {
                return Err(format!(
                    "selected Orc session directory is missing: {}",
                    session_dir.display()
                ));
            }
            continue;
        }
        let database = live_source_path(
            source_root,
            &format!(".orc/sessions/{session_name}/session.db"),
        )?;
        if !source_exists(&database) {
            if selected.is_some() || *session_name == root_id {
                return Err(format!(
                    "selected Orc session database is missing: {}",
                    database.display()
                ));
            }
            continue;
        }
        let relative = database
            .strip_prefix(source_root)
            .map(|held| held.to_string_lossy().replace('\\', "/"))
            .map_err(|_| {
                format!(
                    "Orc source is outside the source root: {}",
                    database.display()
                )
            })?;
        let meta = match session_meta(&database, &relative) {
            Ok(meta) => meta,
            Err(error) => {
                if selected.is_some() || *session_name == root_id {
                    return Err(error);
                }
                continue;
            }
        };
        if meta.session_id != *session_name {
            return Err(format!(
                "Orc session directory {} contains session {}",
                py_repr_str(session_name),
                py_repr_str(&meta.session_id)
            ));
        }
        if let Some(parents) = &index_parents {
            let indexed = parents
                .iter()
                .find(|(held, _)| held == session_name)
                .and_then(|(_, parent)| parent.clone());
            if meta.parent_id != indexed {
                return Err(format!(
                    "Orc session parent differs between index and session DB for {}",
                    py_repr_str(session_name)
                ));
            }
        }
        metas.push(meta);
    }
    if !metas.iter().any(|meta| meta.session_id == root_id) {
        return Err(format!(
            "root Orc session {} was not found under {}",
            py_repr_str(&root_id),
            sessions_root.display()
        ));
    }
    let selected: Vec<String> = selected.unwrap_or_else(|| vec![root_id.clone()]);
    let mut result: Vec<DiscoveredSource> = Vec::new();
    let mut task_owner_evidence: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    let meta_of = |session_id: &str| -> &SessionMeta {
        metas
            .iter()
            .find(|meta| meta.session_id == session_id)
            .expect("selected session meta")
    };
    for session_id in {
        let mut held = selected.clone();
        held.sort();
        held
    } {
        let meta = meta_of(&session_id);
        result.push(DiscoveredSource {
            source_path: meta.source_path.clone(),
            kind: "session".to_string(),
            owner_candidates: vec![session_id.clone()],
            lineage_root_session_id: None,
            source_state: SOURCE_STATE_LIVE.to_string(),
        });
        let session_database = live_source_path(source_root, &meta.source_path)?;
        for (task_relative, required, is_primary) in
            session_task_relatives(&session_database, meta)?
        {
            let task_path = live_source_path(source_root, &task_relative)?;
            if !source_exists(&task_path) {
                if required {
                    return Err(format!(
                        "session {} references task database {}, but {} is missing",
                        py_repr_str(&session_id),
                        py_repr_str(&task_relative),
                        task_path.display()
                    ));
                }
                continue;
            }
            match task_owner_evidence
                .iter_mut()
                .find(|(held, _)| *held == task_relative)
            {
                Some((_, evidence)) => evidence.push((session_id.clone(), is_primary)),
                None => task_owner_evidence
                    .push((task_relative, vec![(session_id.clone(), is_primary)])),
            }
        }
    }
    fn session_depth(session_id: &str, metas: &[SessionMeta], selected: &[String]) -> i64 {
        let mut depth = 0;
        let mut seen: Vec<String> = vec![session_id.to_string()];
        let mut parent = metas
            .iter()
            .find(|meta| meta.session_id == session_id)
            .and_then(|meta| meta.parent_id.clone());
        while let Some(held) = parent {
            if !selected.contains(&held) || seen.contains(&held) {
                break;
            }
            seen.push(held.clone());
            depth += 1;
            parent = metas
                .iter()
                .find(|meta| meta.session_id == held)
                .and_then(|meta| meta.parent_id.clone());
        }
        depth
    }
    task_owner_evidence.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, mut evidence) in task_owner_evidence {
        evidence.sort_by(|a, b| {
            let meta_a = meta_of(&a.0);
            let meta_b = meta_of(&b.0);
            (
                !a.1,
                meta_a.parent_id.is_some(),
                session_depth(&a.0, &metas, &selected),
                meta_a.created_at_ms,
                &a.0,
            )
                .cmp(&(
                    !b.1,
                    meta_b.parent_id.is_some(),
                    session_depth(&b.0, &metas, &selected),
                    meta_b.created_at_ms,
                    &b.0,
                ))
        });
        result.push(DiscoveredSource {
            source_path: path,
            kind: "task".to_string(),
            owner_candidates: evidence.into_iter().map(|(held, _)| held).collect(),
            lineage_root_session_id: None,
            source_state: SOURCE_STATE_LIVE.to_string(),
        });
    }
    result.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    Ok(result)
}

/// Sources below one installation root and every
/// configured continuation lineage.
pub fn discover_continuation_sources(
    source_root: &Path,
    root_session_id: &str,
    specs_in: &[ContinuationSpec],
    previously_selected_session_ids: &[String],
) -> Result<Vec<DiscoveredSource>, String> {
    let mut all_specs: Vec<ContinuationSpec> = vec![ContinuationSpec {
        session_id: root_session_id.to_string(),
        start_message_id: None,
    }];
    // The root spec needs safe-component validation.
    all_specs[0].session_id = safe_component(root_session_id, "continuation_specs[0].session_id")?;
    all_specs.extend(specs_in.iter().cloned());
    let mut merged: Vec<DiscoveredSource> = Vec::new();
    let mut root_start: Option<i64> = None;
    let mut selected_sessions: Vec<String> = Vec::new();
    for spec in &all_specs {
        let root = &spec.session_id;
        let root_relative = format!(".orc/sessions/{root}/session.db");
        let root_path = live_source_path(source_root, &root_relative)?;
        let root_meta = session_meta(&root_path, &root_relative)?;
        if root_meta.session_id != *root || root_meta.parent_id.is_some() {
            return Err(format!(
                "configured Orc continuation {} is not a parentless root session",
                py_repr_str(root)
            ));
        }
        let (_, effective_start) = resolve_continuation_start(&root_path, &root_meta, spec)?;
        if let Some(previous) = root_start {
            if effective_start <= previous {
                return Err(
                    "Orc root and continuation sessions must be ordered by strictly increasing start time"
                        .to_string(),
                );
            }
        }
        root_start = Some(effective_start);
        let mut lineage_sources = discover_sources(source_root, root)?;
        if spec.start_message_id.is_some() {
            let mut retained_sessions: Vec<String> = vec![root.clone()];
            let mut allowed_task_paths: Vec<String> =
                session_task_relatives(&root_path, &root_meta)?
                    .into_iter()
                    .filter(|(_, _, is_primary)| *is_primary)
                    .map(|(path, _, _)| path)
                    .collect();
            let sessions_only: Vec<DiscoveredSource> = lineage_sources
                .iter()
                .filter(|source| source.kind == "session")
                .cloned()
                .collect();
            for session_source in &sessions_only {
                let session_path = live_source_path(source_root, &session_source.source_path)?;
                let held_meta = session_meta(&session_path, &session_source.source_path)?;
                if held_meta.session_id == *root {
                    continue;
                }
                if !previously_selected_session_ids.contains(&held_meta.session_id)
                    && !session_has_activity_at_or_after(
                        source_root,
                        &session_path,
                        &held_meta,
                        root,
                        effective_start,
                    )?
                {
                    continue;
                }
                retained_sessions.push(held_meta.session_id.clone());
                for (path, _, _) in session_task_relatives(&session_path, &held_meta)? {
                    if !allowed_task_paths.contains(&path) {
                        allowed_task_paths.push(path);
                    }
                }
            }
            lineage_sources.retain(|source| {
                (source.kind == "session"
                    && source
                        .owner_candidates
                        .first()
                        .is_some_and(|held| retained_sessions.contains(held)))
                    || allowed_task_paths.contains(&source.source_path)
            });
        }
        for source in lineage_sources {
            let mut source = source;
            if source.kind == "session" {
                source.lineage_root_session_id = Some(root.clone());
            }
            let prior_index = merged
                .iter()
                .position(|held| held.source_path == source.source_path);
            if source.kind == "session" {
                let session_id = source.owner_candidates.first().cloned().unwrap_or_default();
                if selected_sessions.contains(&session_id) || prior_index.is_some() {
                    return Err(format!(
                        "configured Orc root lineages overlap at session {}",
                        py_repr_str(&session_id)
                    ));
                }
                selected_sessions.push(session_id);
                merged.push(source);
                continue;
            }
            match prior_index {
                None => merged.push(source),
                Some(index) => {
                    if merged[index].kind != "task" {
                        return Err(format!(
                            "Orc source path has conflicting database kinds: {}",
                            source.source_path
                        ));
                    }
                    for owner in &source.owner_candidates {
                        if !merged[index].owner_candidates.contains(owner) {
                            merged[index].owner_candidates.push(owner.clone());
                        }
                    }
                }
            }
        }
    }
    merged.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    Ok(merged)
}

// ---------------------------------------------------------------- conversation evidence

/// The normalized conversation from a session database.
pub fn conversation_messages(connection: &Connection) -> Result<Vec<JsonValue>, String> {
    if session_storage_table(connection, "Orc session")? == "messages" {
        let rows = query_all(
            connection,
            "SELECT id, created_at_ms, message_json FROM messages ORDER BY id",
            &[],
            "messages",
        )?;
        let mut result = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
            let message_id = cell_nonnegative(&cell(0), &format!("messages[{index}].id"))?;
            let created_at_ms =
                cell_nonnegative(&cell(1), &format!("messages[{index}].created_at_ms"))?;
            let raw = cell_string(&cell(2), &format!("messages[{index}].message_json"))?;
            let parsed: JsonValue = serde_json::from_str(&raw).map_err(|error| {
                format!("invalid messages.message_json at row {message_id}: {error}")
            })?;
            let mut message = parsed
                .as_object()
                .cloned()
                .ok_or_else(|| format!("messages[{index}].message_json: expected an object"))?;
            let embedded_at = nonnegative_integer(
                message.get("created_at_ms"),
                &format!("messages[{index}].message_json.created_at_ms"),
            )?;
            if embedded_at != created_at_ms {
                return Err(format!(
                    "messages[{index}] timestamp differs from message_json"
                ));
            }
            message.insert("id".into(), JsonValue::from(message_id));
            message.insert("created_at_ms".into(), JsonValue::from(created_at_ms));
            result.push(JsonValue::Object(message));
        }
        return Ok(result);
    }
    let row = query_one(
        connection,
        "SELECT conversation_json FROM conversation_state WHERE id = 1",
        "conversation_state",
    )?;
    let raw = cell_string(
        row.first().unwrap_or(&SqlValue::Null),
        "conversation_state.conversation_json",
    )?;
    let parsed: JsonValue = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid conversation_state JSON: {error}"))?;
    let root = parsed
        .as_object()
        .ok_or_else(|| "conversation_state: expected an object".to_string())?;
    match root.get("messages") {
        Some(JsonValue::Array(items)) => Ok(items.clone()),
        _ => Err("conversation_state.messages: expected an array".to_string()),
    }
}

/// Canonical digest of a normalized conversation.
pub fn messages_digest(messages: &[JsonValue]) -> Result<String, String> {
    let encoded = crate::json::dump_compact(&JsonValue::Array(messages.to_vec()));
    Ok(sha256_hex(encoded.as_bytes()))
}

/// One spawn an append-only rewrite cannot take away.
#[derive(Clone, Debug, PartialEq)]
pub struct StableSpawn {
    /// See the enclosing record's documentation.
    pub session_id: String,
    /// See the enclosing record's documentation.
    pub parent_session_id: Option<String>,
    /// See the enclosing record's documentation.
    pub message_id: i64,
    /// See the enclosing record's documentation.
    pub block_id: i64,
    /// See the enclosing record's documentation.
    pub timestamp_ms: i64,
    /// See the enclosing record's documentation.
    pub agent_id: String,
}

impl StableSpawn {
    fn key(&self) -> (String, i64) {
        (self.session_id.clone(), self.block_id)
    }
}

/// Every stable spawn in a conversation, sorted by key.
pub fn stable_spawn_records(
    messages: &[JsonValue],
    meta: &SessionMeta,
    where_: &str,
) -> Result<Vec<StableSpawn>, String> {
    let mut records: Vec<StableSpawn> = Vec::new();
    let mut seen: Vec<(String, i64)> = Vec::new();
    for (message_index, raw_message) in messages.iter().enumerate() {
        let message_where = format!("{where_}: messages[{message_index}]");
        let message = raw_message
            .as_object()
            .ok_or_else(|| format!("{message_where}: expected an object"))?;
        let message_id = signed_integer(message.get("id"), &format!("{message_where}.id"))?;
        let timestamp_ms = signed_integer(
            message.get("created_at_ms"),
            &format!("{message_where}.created_at_ms"),
        )?;
        let blocks = match message.get("blocks") {
            Some(JsonValue::Array(items)) => items.clone(),
            _ => return Err(format!("{message_where}.blocks: expected an array")),
        };
        for (block_index, raw_block) in blocks.iter().enumerate() {
            let block_where = format!("{message_where}.blocks[{block_index}]");
            let block = raw_block
                .as_object()
                .ok_or_else(|| format!("{block_where}: expected an object"))?;
            if block.get("type").and_then(JsonValue::as_str) != Some("AgentBlock") {
                continue;
            }
            let block_id = signed_integer(block.get("id"), &format!("{block_where}.id"))?;
            let record = StableSpawn {
                session_id: meta.session_id.clone(),
                parent_session_id: meta.parent_id.clone(),
                message_id,
                block_id,
                timestamp_ms,
                agent_id: required_string(
                    block.get("agent_id"),
                    &format!("{block_where}.agent_id"),
                )?,
            };
            if seen.contains(&record.key()) {
                return Err(format!(
                    "{where_}: duplicate AgentBlock identity ({}, {})",
                    py_repr_str(&record.session_id),
                    record.block_id
                ));
            }
            seen.push(record.key());
            records.push(record);
        }
    }
    records.sort_by_key(|a| a.key());
    Ok(records)
}

/// Digest of stable spawn records.
pub fn stable_spawn_digest(records: &[StableSpawn]) -> String {
    let mut digest = sha2::Sha256::new();
    for record in records {
        digest.update(b"R");
        update_digest_json(&mut digest, &JsonValue::String(record.session_id.clone()));
        update_digest_json(
            &mut digest,
            &record
                .parent_session_id
                .clone()
                .map_or(JsonValue::Null, JsonValue::String),
        );
        update_digest_json(&mut digest, &JsonValue::from(record.message_id));
        update_digest_json(&mut digest, &JsonValue::from(record.block_id));
        update_digest_json(&mut digest, &JsonValue::from(record.timestamp_ms));
        update_digest_json(&mut digest, &JsonValue::String(record.agent_id.clone()));
    }
    hex_digest(&digest)
}

/// A session's conversation and the spawns it proves.
#[derive(Clone, Debug)]
pub struct AuxiliaryObservation {
    /// See the enclosing record's documentation.
    pub messages: Vec<JsonValue>,
    /// See the enclosing record's documentation.
    pub message_sha256: String,
    /// See the enclosing record's documentation.
    pub stable_spawns: Vec<StableSpawn>,
    /// See the enclosing record's documentation.
    pub stable_spawn_sha256: String,
}

/// Observe a session's conversation evidence.
pub fn auxiliary_observation(
    path: &Path,
    meta: &SessionMeta,
) -> Result<AuxiliaryObservation, String> {
    let connection = read_only(path)?;
    let messages = conversation_messages(&connection)?;
    drop(connection);
    let records = stable_spawn_records(&messages, meta, &path.display().to_string())?;
    Ok(AuxiliaryObservation {
        message_sha256: messages_digest(&messages)?,
        stable_spawns: records.clone(),
        stable_spawn_sha256: stable_spawn_digest(&records),
        messages,
    })
}

/// Whether the current conversation extends the recorded
/// one unchanged to its length.
pub fn conversation_is_append_extension(
    previous: &AuxiliaryObservation,
    current: &AuxiliaryObservation,
) -> Result<bool, String> {
    let previous_count = previous.messages.len();
    Ok(current.messages.len() >= previous_count
        && messages_digest(&current.messages[..previous_count])? == previous.message_sha256)
}

/// Refuse when stable spawns disappeared or changed.
pub fn validate_stable_spawn_extension(
    previous: &AuxiliaryObservation,
    current: &AuxiliaryObservation,
    relative: &str,
) -> Result<(), String> {
    let missing: Vec<&StableSpawn> = previous
        .stable_spawns
        .iter()
        .filter(|record| {
            !current
                .stable_spawns
                .iter()
                .any(|held| held.key() == record.key())
        })
        .collect();
    if let Some(first) = missing.first() {
        return Err(format!(
            "Orc stable spawn evidence disappeared for {relative}: ({}, {})",
            py_repr_str(&first.session_id),
            first.block_id
        ));
    }
    let changed: Vec<&StableSpawn> = previous
        .stable_spawns
        .iter()
        .filter(|record| {
            current
                .stable_spawns
                .iter()
                .find(|held| held.key() == record.key())
                .is_some_and(|held| held != *record)
        })
        .collect();
    if let Some(first) = changed.first() {
        return Err(format!(
            "Orc stable spawn evidence was rewritten for {relative}: ({}, {})",
            py_repr_str(&first.session_id),
            first.block_id
        ));
    }
    Ok(())
}

/// Refuse when storage migration changed anything but a
/// spawn's message id.
pub fn validate_stable_spawn_storage_migration(
    previous: &AuxiliaryObservation,
    current: &AuxiliaryObservation,
    relative: &str,
) -> Result<(), String> {
    let missing: Vec<&StableSpawn> = previous
        .stable_spawns
        .iter()
        .filter(|record| {
            !current
                .stable_spawns
                .iter()
                .any(|held| held.key() == record.key())
        })
        .collect();
    if let Some(first) = missing.first() {
        return Err(format!(
            "Orc stable spawn evidence disappeared during storage migration for {relative}: ({}, {})",
            py_repr_str(&first.session_id),
            first.block_id
        ));
    }
    for prior in &previous.stable_spawns {
        let current_record = current
            .stable_spawns
            .iter()
            .find(|held| held.key() == prior.key())
            .expect("checked present");
        if (
            &prior.session_id,
            &prior.parent_session_id,
            prior.block_id,
            prior.timestamp_ms,
            &prior.agent_id,
        ) != (
            &current_record.session_id,
            &current_record.parent_session_id,
            current_record.block_id,
            current_record.timestamp_ms,
            &current_record.agent_id,
        ) {
            return Err(format!(
                "Orc stable spawn evidence was rewritten during storage migration for {relative}: ({}, {})",
                py_repr_str(&prior.session_id),
                prior.block_id
            ));
        }
    }
    Ok(())
}

/// Refuse when immutable session metadata was rewritten.
pub fn validate_session_meta_extension(
    previous: &SessionMeta,
    current: &SessionMeta,
    relative: &str,
) -> Result<(), String> {
    if (
        &current.session_id,
        &current.parent_id,
        current.created_at_ms,
    ) != (
        &previous.session_id,
        &previous.parent_id,
        previous.created_at_ms,
    ) {
        return Err(format!(
            "Orc immutable session metadata was rewritten for {relative}"
        ));
    }
    if current.updated_at_ms < previous.updated_at_ms {
        return Err(format!(
            "Orc session updated_at moved backwards for {relative}: {} to {}",
            previous.updated_at_ms, current.updated_at_ms
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- task projections

/// Frozen per-note enrichment evidence (schema 1).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskNoteEnrichment {
    /// See the enclosing record's documentation.
    pub note_id: i64,
    /// See the enclosing record's documentation.
    pub task_id: String,
    /// See the enclosing record's documentation.
    pub server_author: Option<String>,
    /// See the enclosing record's documentation.
    pub task_owner: Option<String>,
    /// See the enclosing record's documentation.
    pub title: String,
}

/// One frozen task note (schema 2 history).
#[derive(Clone, Debug, PartialEq)]
pub struct FrozenTaskNote {
    /// See the enclosing record's documentation.
    pub note_id: i64,
    /// See the enclosing record's documentation.
    pub task_id: String,
    /// See the enclosing record's documentation.
    pub content: String,
    /// See the enclosing record's documentation.
    pub created_at: String,
    /// See the enclosing record's documentation.
    pub server_author: Option<String>,
    /// See the enclosing record's documentation.
    pub task_owner: Option<String>,
    /// See the enclosing record's documentation.
    pub title: String,
}

impl FrozenTaskNote {
    /// The schema-1 enrichment view of this note.
    pub fn enrichment(&self) -> TaskNoteEnrichment {
        TaskNoteEnrichment {
            note_id: self.note_id,
            task_id: self.task_id.clone(),
            server_author: self.server_author.clone(),
            task_owner: self.task_owner.clone(),
            title: self.title.clone(),
        }
    }
}

/// One currently-visible task note.
#[derive(Clone, Debug)]
pub struct ObservedTaskNote {
    /// See the enclosing record's documentation.
    pub note_id: i64,
    /// See the enclosing record's documentation.
    pub task_id: String,
    /// See the enclosing record's documentation.
    pub content: String,
    /// See the enclosing record's documentation.
    pub created_at: String,
    /// See the enclosing record's documentation.
    pub server_author: Option<String>,
    /// See the enclosing record's documentation.
    pub task_owner: Option<String>,
    /// See the enclosing record's documentation.
    pub title: Option<String>,
}

/// Canonical schema-1 projection text.
pub fn task_projection_text(records: &[TaskNoteEnrichment]) -> String {
    let items: Vec<JsonValue> = records
        .iter()
        .map(|record| {
            let mut item = Map::new();
            item.insert("note_id".into(), JsonValue::from(record.note_id));
            item.insert("task_id".into(), JsonValue::String(record.task_id.clone()));
            item.insert(
                "server_author".into(),
                record
                    .server_author
                    .clone()
                    .map_or(JsonValue::Null, JsonValue::String),
            );
            item.insert(
                "task_owner".into(),
                record
                    .task_owner
                    .clone()
                    .map_or(JsonValue::Null, JsonValue::String),
            );
            item.insert("title".into(), JsonValue::String(record.title.clone()));
            JsonValue::Object(item)
        })
        .collect();
    let mut root = Map::new();
    root.insert("schema_version".into(), JsonValue::from(1));
    root.insert("records".into(), JsonValue::Array(items));
    crate::json::dump_compact(&JsonValue::Object(root)) + "\n"
}

/// Canonical schema-2 frozen history text.
pub fn frozen_task_projection_text(records: &[FrozenTaskNote]) -> String {
    let items: Vec<JsonValue> = records
        .iter()
        .map(|record| {
            let mut item = Map::new();
            item.insert("note_id".into(), JsonValue::from(record.note_id));
            item.insert("task_id".into(), JsonValue::String(record.task_id.clone()));
            item.insert("content".into(), JsonValue::String(record.content.clone()));
            item.insert(
                "created_at".into(),
                JsonValue::String(record.created_at.clone()),
            );
            item.insert(
                "server_author".into(),
                record
                    .server_author
                    .clone()
                    .map_or(JsonValue::Null, JsonValue::String),
            );
            item.insert(
                "task_owner".into(),
                record
                    .task_owner
                    .clone()
                    .map_or(JsonValue::Null, JsonValue::String),
            );
            item.insert("title".into(), JsonValue::String(record.title.clone()));
            JsonValue::Object(item)
        })
        .collect();
    let mut root = Map::new();
    root.insert("schema_version".into(), JsonValue::from(2));
    root.insert("records".into(), JsonValue::Array(items));
    crate::json::dump_compact(&JsonValue::Object(root)) + "\n"
}

/// A task database's enrichment records and their digest.
pub struct TaskEnrichmentObservation {
    /// See the enclosing record's documentation.
    pub records: Vec<TaskNoteEnrichment>,
    /// See the enclosing record's documentation.
    pub sha256: String,
}

/// Observe a task database's enrichment.
pub fn task_enrichment_observation(path: &Path) -> Result<TaskEnrichmentObservation, String> {
    let connection = read_only(path)?;
    let rows = query_all_inner(
        &connection,
        "SELECT n.id, n.task_id, n.author_unixname, t.owner, t.title \
         FROM task_notes n LEFT JOIN tasks t ON t.local_id = n.task_id \
         ORDER BY n.id",
        &[],
    )
    .map_err(|error| format!("failed to inspect Orc tasks at {}: {error}", path.display()))?;
    drop(connection);
    let mut records = Vec::new();
    let mut seen: Vec<i64> = Vec::new();
    for row in &rows {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        let note_id = cell_nonnegative(&cell(0), &format!("{}: note id", path.display()))?;
        let record = TaskNoteEnrichment {
            note_id,
            task_id: cell_string(&cell(1), &format!("{}: note task_id", path.display()))?,
            server_author: cell_optional_string(
                &cell(2),
                &format!("{}: note author", path.display()),
            )?,
            task_owner: cell_optional_string(&cell(3), &format!("{}: task owner", path.display()))?,
            title: cell_string(&cell(4), &format!("{}: task title", path.display()))?,
        };
        if seen.contains(&note_id) {
            return Err(format!(
                "{}: duplicate task note id {note_id}",
                path.display()
            ));
        }
        seen.push(note_id);
        records.push(record);
    }
    let digest = sha256_hex(task_projection_text(&records).as_bytes());
    Ok(TaskEnrichmentObservation {
        records,
        sha256: digest,
    })
}

/// Every currently-visible note in a task database.
pub fn observed_task_notes(path: &Path) -> Result<Vec<ObservedTaskNote>, String> {
    let connection = read_only(path)?;
    let rows = query_all_inner(
        &connection,
        "SELECT n.id, n.task_id, n.content, n.created_at, n.author_unixname, \
         t.owner, t.title FROM task_notes n LEFT JOIN tasks t \
         ON t.local_id = n.task_id ORDER BY n.id",
        &[],
    )
    .map_err(|error| format!("failed to inspect Orc tasks at {}: {error}", path.display()))?;
    drop(connection);
    let mut records = Vec::new();
    for row in &rows {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        records.push(ObservedTaskNote {
            note_id: cell_nonnegative(&cell(0), &format!("{}: note id", path.display()))?,
            task_id: cell_string(&cell(1), &format!("{}: note task_id", path.display()))?,
            content: cell_text(&cell(2), &format!("{}: note content", path.display()))?,
            created_at: cell_string(&cell(3), &format!("{}: note created_at", path.display()))?,
            server_author: cell_optional_string(
                &cell(4),
                &format!("{}: note author", path.display()),
            )?,
            task_owner: cell_optional_string(&cell(5), &format!("{}: task owner", path.display()))?,
            title: cell_optional_string(&cell(6), &format!("{}: task title", path.display()))?,
        });
    }
    if {
        let mut ids: Vec<i64> = records.iter().map(|record| record.note_id).collect();
        ids.sort();
        ids.dedup();
        ids.len()
    } != records.len()
    {
        return Err(format!("{}: duplicate task note id", path.display()));
    }
    Ok(records)
}

/// Orc's durable task-note allocation high-water mark.
pub fn task_note_sequence(path: &Path) -> Result<i64, String> {
    let connection = read_only(path)?;
    let outcome: Result<i64, String> = (|| {
        let tables = table_names(&connection, &path.display().to_string())?;
        if tables.iter().any(|held| held == "sqlite_sequence") {
            let rows = query_all_inner(
                &connection,
                "SELECT seq FROM sqlite_sequence WHERE name = 'task_notes'",
                &[],
            )?;
            if let Some(row) = rows.first() {
                return cell_nonnegative(
                    row.first().unwrap_or(&SqlValue::Null),
                    &format!("{}: task note sequence", path.display()),
                );
            }
        }
        let row = query_one(
            &connection,
            "SELECT COALESCE(MAX(id), 0) FROM task_notes",
            &path.display().to_string(),
        )?;
        cell_nonnegative(
            row.first().unwrap_or(&SqlValue::Null),
            &format!("{}: task note highwater", path.display()),
        )
    })();
    drop(connection);
    outcome.map_err(|error| {
        if error.contains(": task note") || error.contains("expected one row") {
            error
        } else {
            format!(
                "failed to inspect Orc task-note sequence at {}: {error}",
                path.display()
            )
        }
    })
}

fn read_projection_text(
    snapshot_root: &Path,
    status: &TaskProjection,
) -> Result<(PathBuf, String), String> {
    let path = snapshot_path(snapshot_root, &status.path)?;
    let is_symlink = std::fs::symlink_metadata(&path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink || !path.is_file() {
        return Err(format!(
            "task projection is missing or unsafe: {}",
            path.display()
        ));
    }
    let data = std::fs::read(&path).map_err(|error| crate::json::os_error(&error, &path))?;
    let text = String::from_utf8(data).map_err(|error| {
        format!(
            "'utf-8' codec can't decode bytes from {}: {error}",
            path.display()
        )
    })?;
    if sha256_hex(text.as_bytes()) != status.sha256 {
        return Err(format!("task projection hash mismatch: {}", path.display()));
    }
    Ok((path, text))
}

fn parse_projection_records(
    path: &Path,
    text: &str,
    schema: i64,
    frozen: bool,
) -> Result<Vec<JsonValue>, String> {
    let parsed: JsonValue = serde_json::from_str(text).map_err(|error| {
        format!(
            "invalid task projection JSON at {}: {error}",
            path.display()
        )
    })?;
    let root = parsed
        .as_object()
        .ok_or_else(|| format!("{}: expected an object", path.display()))?;
    require_exact_keys(
        root,
        &["schema_version", "records"],
        &path.display().to_string(),
    )?;
    if root.get("schema_version").and_then(JsonValue::as_i64) != Some(schema) {
        return Err(if frozen {
            format!(
                "unsupported frozen task projection schema at {}",
                path.display()
            )
        } else {
            format!("unsupported task projection schema at {}", path.display())
        });
    }
    match root.get("records") {
        Some(JsonValue::Array(items)) => Ok(items.clone()),
        _ => Err(format!("{}.records: expected an array", path.display())),
    }
}

/// Load and authenticate a schema-1 task projection.
pub fn load_legacy_task_projection(
    snapshot_root: &Path,
    status: &TaskProjection,
) -> Result<Vec<TaskNoteEnrichment>, String> {
    let (path, raw_text) = read_projection_text(snapshot_root, status)?;
    let items = parse_projection_records(&path, &raw_text, 1, false)?;
    let mut records = Vec::new();
    let mut prior_id = -1i64;
    for (index, raw_record) in items.iter().enumerate() {
        let held = format!("{}.records[{index}]", path.display());
        let obj = mapping(Some(raw_record), &held)?;
        require_exact_keys(
            obj,
            &["note_id", "task_id", "server_author", "task_owner", "title"],
            &held,
        )?;
        let record = TaskNoteEnrichment {
            note_id: nonnegative_integer(obj.get("note_id"), &format!("{held}.note_id"))?,
            task_id: required_string(obj.get("task_id"), &format!("{held}.task_id"))?,
            server_author: optional_string(
                obj.get("server_author"),
                &format!("{held}.server_author"),
            )?,
            task_owner: optional_string(obj.get("task_owner"), &format!("{held}.task_owner"))?,
            title: required_string(obj.get("title"), &format!("{held}.title"))?,
        };
        if record.note_id <= prior_id {
            return Err(format!(
                "task projection note IDs are not strictly ordered: {}",
                path.display()
            ));
        }
        prior_id = record.note_id;
        records.push(record);
    }
    if records.len() as i64 != status.note_count {
        return Err(format!(
            "task projection count mismatch: {}",
            path.display()
        ));
    }
    if task_projection_text(&records) != raw_text {
        return Err(format!(
            "task projection is not canonical: {}",
            path.display()
        ));
    }
    Ok(records)
}

/// Load and authenticate a schema-2 frozen task history.
pub fn load_frozen_task_projection(
    snapshot_root: &Path,
    status: &TaskProjection,
) -> Result<Vec<FrozenTaskNote>, String> {
    let (path, raw_text) = read_projection_text(snapshot_root, status)?;
    let items = parse_projection_records(&path, &raw_text, 2, true)?;
    let mut records = Vec::new();
    let mut prior_id = -1i64;
    for (index, raw_record) in items.iter().enumerate() {
        let held = format!("{}.records[{index}]", path.display());
        let obj = mapping(Some(raw_record), &held)?;
        require_exact_keys(
            obj,
            &[
                "note_id",
                "task_id",
                "content",
                "created_at",
                "server_author",
                "task_owner",
                "title",
            ],
            &held,
        )?;
        let record = FrozenTaskNote {
            note_id: nonnegative_integer(obj.get("note_id"), &format!("{held}.note_id"))?,
            task_id: required_string(obj.get("task_id"), &format!("{held}.task_id"))?,
            content: string_value(obj.get("content"), &format!("{held}.content"))?,
            created_at: required_string(obj.get("created_at"), &format!("{held}.created_at"))?,
            server_author: optional_string(
                obj.get("server_author"),
                &format!("{held}.server_author"),
            )?,
            task_owner: optional_string(obj.get("task_owner"), &format!("{held}.task_owner"))?,
            title: required_string(obj.get("title"), &format!("{held}.title"))?,
        };
        if record.note_id <= prior_id {
            return Err(format!(
                "task projection note IDs are not strictly ordered: {}",
                path.display()
            ));
        }
        prior_id = record.note_id;
        records.push(record);
    }
    if records.len() as i64 != status.note_count
        || frozen_task_projection_text(&records) != raw_text
    {
        return Err(format!(
            "frozen task projection mismatch: {}",
            path.display()
        ));
    }
    Ok(records)
}

/// Freeze one currently-visible note.
pub fn freeze_current_task_note(
    record: &ObservedTaskNote,
    relative: &str,
) -> Result<FrozenTaskNote, String> {
    let Some(title) = &record.title else {
        return Err(format!(
            "new task note {} lacks its task row in {relative}",
            record.note_id
        ));
    };
    Ok(FrozenTaskNote {
        note_id: record.note_id,
        task_id: record.task_id.clone(),
        content: record.content.clone(),
        created_at: record.created_at.clone(),
        server_author: record.server_author.clone(),
        task_owner: record.task_owner.clone(),
        title: title.clone(),
    })
}

/// Load v3 history or upgrade one older projection.
pub fn bootstrap_frozen_task_projection(
    snapshot_root: &Path,
    source: &SourceCopy,
    source_path: &Path,
) -> Result<Vec<FrozenTaskNote>, String> {
    if let Some(projection) = &source.task_projection {
        if projection.policy == TASK_HISTORY_POLICY {
            return load_frozen_task_projection(snapshot_root, projection);
        }
    }
    let observed = observed_task_notes(source_path)?;
    let Some(projection) = &source.task_projection else {
        let mut frozen = Vec::new();
        for record in &observed {
            frozen.push(freeze_current_task_note(record, &source.source_path)?);
        }
        return Ok(frozen);
    };
    let enrichments = load_legacy_task_projection(snapshot_root, projection)?;
    if observed.len() != enrichments.len() {
        return Err(format!(
            "legacy task projection count does not match its snapshot for {}",
            source.source_path
        ));
    }
    let mut frozen = Vec::new();
    for record in &observed {
        let enrichment = enrichments
            .iter()
            .find(|held| held.note_id == record.note_id);
        match enrichment {
            Some(enrichment) if enrichment.task_id == record.task_id => {
                frozen.push(FrozenTaskNote {
                    note_id: record.note_id,
                    task_id: record.task_id.clone(),
                    content: record.content.clone(),
                    created_at: record.created_at.clone(),
                    server_author: enrichment.server_author.clone(),
                    task_owner: enrichment.task_owner.clone(),
                    title: enrichment.title.clone(),
                });
            }
            _ => {
                return Err(format!(
                    "legacy task projection does not match note {} in {}",
                    record.note_id, source.source_path
                ));
            }
        }
    }
    Ok(frozen)
}

/// Digest of frozen records' enrichment view.
pub fn task_enrichment_sha256(records: &[FrozenTaskNote]) -> String {
    let enrichments: Vec<TaskNoteEnrichment> =
        records.iter().map(FrozenTaskNote::enrichment).collect();
    sha256_hex(task_projection_text(&enrichments).as_bytes())
}

/// Write a frozen history under `.staging` and return
/// (temporary, target, digest, relative path).
pub fn stage_task_projection(
    records: &[FrozenTaskNote],
    snapshot_root: &Path,
) -> Result<(PathBuf, PathBuf, String, String), String> {
    let text = frozen_task_projection_text(records);
    let digest = sha256_hex(text.as_bytes());
    let relative = task_projection_relative(&digest)?;
    let target = snapshot_path(snapshot_root, &relative)?;
    let token = format!(
        "{:016x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|held| held.as_nanos() as u64)
            .unwrap_or(0)
    );
    let temporary = snapshot_root.join(".staging").join(format!(
        "task-projection-{}-{token}.json",
        std::process::id()
    ));
    ensure_snapshot_parent(&temporary, snapshot_root)?;
    std::fs::write(&temporary, text.as_bytes())
        .map_err(|error| crate::json::os_error(&error, &temporary))?;
    Ok((temporary, target, digest, relative))
}

// ---------------------------------------------------------------- logical state

/// Columns the content-blocks append digest covers, in
/// digest order.
pub const CONTENT_BLOCK_PREFIX_COLUMNS: [&str; 18] = [
    "rowid",
    "id",
    "message_id",
    "session_id",
    "block_index",
    "created_at_ms",
    "turn_index",
    "role",
    "block_type",
    "content",
    "searchable_text",
    "code_input",
    "code_output",
    "code_exit_code",
    "model",
    "user_source",
    "token_count",
    "extra",
];

/// Columns the messages append digest covers, in order.
pub const MESSAGE_PREFIX_COLUMNS: [&str; 6] = [
    "id",
    "session_id",
    "role",
    "created_at_ms",
    "message_json",
    "search_text",
];

/// See the enclosing record's documentation.
pub const SESSION_STATE_SHIFT: u32 = 64;
/// See the enclosing record's documentation.
pub const SESSION_STATE_MASK: u128 = (1 << SESSION_STATE_SHIFT) - 1;
/// The packed session-state tag value (bit 128).
pub fn session_state_tag() -> BigUint {
    BigUint([0, 0, 1, 0])
}

/// One source's counted, watermarked, digested prefix.
#[derive(Clone, Debug, PartialEq)]
pub struct LogicalState {
    /// See the enclosing record's documentation.
    pub append_count: i64,
    /// See the enclosing record's documentation.
    pub append_max_id: BigUint,
    /// See the enclosing record's documentation.
    pub append_prefix_sha256: String,
}

/// Row counts and watermarks for a session's tables.
#[derive(Clone, Debug)]
pub struct SessionGeometry {
    /// See the enclosing record's documentation.
    pub storage_table: String,
    /// See the enclosing record's documentation.
    pub content_count: i64,
    /// See the enclosing record's documentation.
    pub content_max_id: i64,
    /// See the enclosing record's documentation.
    pub message_count: i64,
    /// See the enclosing record's documentation.
    pub message_max_id: i64,
}

/// One guarded table, its digest columns, and its
/// watermark.
#[derive(Clone, Debug)]
pub struct PrefixScope {
    /// See the enclosing record's documentation.
    pub table: String,
    /// See the enclosing record's documentation.
    pub columns: Vec<&'static str>,
    /// See the enclosing record's documentation.
    pub limit: u128,
}

impl PrefixScope {
    /// The one statement both the digest and the diff
    /// read rows through.
    pub fn query(&self) -> String {
        let key = self.columns[0];
        format!(
            "SELECT {} FROM {} WHERE {key} <= ? ORDER BY {key}",
            self.columns.join(", "),
            self.table
        )
    }
}

/// Measure a session's tables, honouring legacy modes.
pub fn session_geometry(
    connection: &Connection,
    path: &Path,
    session_state_mode: Option<&str>,
) -> Result<SessionGeometry, String> {
    let mut storage_table = session_storage_table(connection, &path.display().to_string())?;
    match session_state_mode {
        Some("messages-only") => {
            if storage_table != "messages" {
                return Err(format!(
                    "{}: messages-only legacy state lacks messages table",
                    path.display()
                ));
            }
        }
        Some("content-only") => storage_table = "content_blocks".to_string(),
        Some(other) => {
            return Err(format!(
                "unsupported legacy Orc session state mode {}",
                py_repr_str(other)
            ))
        }
        None => {}
    }
    let mut content_count = 0i64;
    let mut content_max_id = 0i64;
    if session_state_mode != Some("messages-only") {
        let row = query_one(
            connection,
            "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM content_blocks",
            &path.display().to_string(),
        )?;
        content_count = cell_nonnegative(&row[0], &format!("{}: content count", path.display()))?;
        content_max_id =
            cell_nonnegative(&row[1], &format!("{}: max content rowid", path.display()))?;
    }
    let mut message_count = 0i64;
    let mut message_max_id = 0i64;
    if storage_table == "messages" {
        let row = query_one(
            connection,
            "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM messages",
            &path.display().to_string(),
        )?;
        message_count = cell_nonnegative(&row[0], &format!("{}: message count", path.display()))?;
        message_max_id = cell_nonnegative(&row[1], &format!("{}: max message id", path.display()))?;
    }
    Ok(SessionGeometry {
        storage_table,
        content_count,
        content_max_id,
        message_count,
        message_max_id,
    })
}

/// What the append-prefix digest covers, and whether it
/// is a legacy prefix.
pub fn session_prefix_scopes(
    geometry: &SessionGeometry,
    session_state_mode: Option<&str>,
    prefix_max_id: Option<BigUint>,
) -> (Vec<PrefixScope>, bool) {
    if session_state_mode == Some("messages-only") {
        let limit = prefix_max_id
            .and_then(|held| held.as_u64())
            .map(u128::from)
            .unwrap_or(geometry.message_max_id as u128);
        return (
            vec![PrefixScope {
                table: "messages".to_string(),
                columns: MESSAGE_PREFIX_COLUMNS.to_vec(),
                limit,
            }],
            false,
        );
    }
    if geometry.storage_table == "messages" {
        let (content_limit, message_limit, legacy_prefix) = match prefix_max_id {
            None => (
                geometry.content_max_id as u128,
                geometry.message_max_id as u128,
                false,
            ),
            Some(packed_max) => {
                if packed_max.has_session_tag() {
                    let (content, message) = packed_max.unpacked();
                    (u128::from(content), u128::from(message), false)
                } else {
                    (
                        packed_max.as_u64().map(u128::from).unwrap_or(u128::MAX),
                        0,
                        true,
                    )
                }
            }
        };
        let content_scope = PrefixScope {
            table: "content_blocks".to_string(),
            columns: CONTENT_BLOCK_PREFIX_COLUMNS.to_vec(),
            limit: content_limit,
        };
        if legacy_prefix {
            return (vec![content_scope], true);
        }
        return (
            vec![
                content_scope,
                PrefixScope {
                    table: "messages".to_string(),
                    columns: MESSAGE_PREFIX_COLUMNS.to_vec(),
                    limit: message_limit,
                },
            ],
            false,
        );
    }
    let limit = prefix_max_id
        .and_then(|held| held.as_u64())
        .map(u128::from)
        .unwrap_or(geometry.content_max_id as u128);
    (
        vec![PrefixScope {
            table: "content_blocks".to_string(),
            columns: CONTENT_BLOCK_PREFIX_COLUMNS.to_vec(),
            limit,
        }],
        false,
    )
}

/// Measure one source's append state.
pub fn logical_state(
    path: &Path,
    kind: &str,
    prefix_max_id: Option<BigUint>,
    legacy_task_fields: bool,
    session_state_mode: Option<&str>,
) -> Result<LogicalState, String> {
    let connection = read_only(path)?;
    let outcome = logical_state_inner(
        &connection,
        path,
        kind,
        prefix_max_id,
        legacy_task_fields,
        session_state_mode,
    );
    drop(connection);
    outcome.map_err(|error| {
        if error.starts_with("SQLite quick_check failed")
            || error.contains(": missing required tables:")
            || error.contains(": missing both modern messages")
            || error.contains("session row identity exceeds 64 bits")
            || error.contains("messages-only legacy state lacks")
            || error.starts_with("unsupported legacy Orc session state mode")
            || error.starts_with("unsupported Orc source kind")
        {
            error
        } else {
            format!(
                "failed to inspect Orc SQLite source {}: {error}",
                path.display()
            )
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn logical_state_inner(
    connection: &Connection,
    path: &Path,
    kind: &str,
    prefix_max_id: Option<BigUint>,
    legacy_task_fields: bool,
    session_state_mode: Option<&str>,
) -> Result<LogicalState, String> {
    let check = query_one(
        connection,
        "PRAGMA quick_check",
        &path.display().to_string(),
    )?;
    let first = check.first().cloned().unwrap_or(SqlValue::Null);
    if first != SqlValue::Text("ok".to_string()) {
        let rendered = match &first {
            SqlValue::Text(text) => text.clone(),
            other => format!("{other:?}"),
        };
        return Err(format!(
            "SQLite quick_check failed for {}: ('{rendered}',)",
            path.display()
        ));
    }
    if kind == "session" {
        require_tables(
            connection,
            &["session_meta", "content_blocks"],
            &path.display().to_string(),
        )?;
        let geometry = session_geometry(connection, path, session_state_mode)?;
        let (scopes, legacy_prefix) =
            session_prefix_scopes(&geometry, session_state_mode, prefix_max_id);
        if session_state_mode == Some("messages-only") {
            let scope = &scopes[0];
            let (prefix_count, prefix_digest) = query_digest(
                connection,
                &scope.query(),
                &[SqlValue::Int(scope.limit as i64)],
            )?;
            return Ok(LogicalState {
                append_count: if prefix_max_id.is_none() {
                    geometry.message_count
                } else {
                    prefix_count
                },
                append_max_id: BigUint::from_u64(geometry.message_max_id as u64),
                append_prefix_sha256: prefix_digest,
            });
        }
        if geometry.storage_table == "messages" {
            if geometry.content_max_id as u128 > SESSION_STATE_MASK
                || geometry.message_max_id as u128 > SESSION_STATE_MASK
            {
                return Err(format!(
                    "{}: session row identity exceeds 64 bits",
                    path.display()
                ));
            }
            let packed_max_id = BigUint::packed(
                geometry.content_max_id as u64,
                geometry.message_max_id as u64,
            );
            let content_scope = &scopes[0];
            let (content_prefix_count, content_prefix_digest) = query_digest(
                connection,
                &content_scope.query(),
                &[SqlValue::Int(content_scope.limit as i64)],
            )?;
            if legacy_prefix {
                return Ok(LogicalState {
                    append_count: content_prefix_count,
                    append_max_id: packed_max_id,
                    append_prefix_sha256: content_prefix_digest,
                });
            }
            let message_scope = &scopes[1];
            let (message_prefix_count, message_prefix_digest) = query_digest(
                connection,
                &message_scope.query(),
                &[SqlValue::Int(message_scope.limit as i64)],
            )?;
            let mut combined = sha2::Sha256::new();
            update_digest(&mut combined, &SqlValue::Text("content_blocks".to_string()));
            update_digest(&mut combined, &SqlValue::Int(content_prefix_count));
            update_digest(&mut combined, &SqlValue::Int(content_scope.limit as i64));
            update_digest(&mut combined, &SqlValue::Text(content_prefix_digest));
            update_digest(&mut combined, &SqlValue::Text("messages".to_string()));
            update_digest(&mut combined, &SqlValue::Int(message_prefix_count));
            update_digest(&mut combined, &SqlValue::Int(message_scope.limit as i64));
            update_digest(&mut combined, &SqlValue::Text(message_prefix_digest));
            return Ok(LogicalState {
                append_count: if prefix_max_id.is_none() {
                    geometry.content_count + geometry.message_count
                } else {
                    content_prefix_count + message_prefix_count
                },
                append_max_id: packed_max_id,
                append_prefix_sha256: hex_digest(&combined),
            });
        }
        let scope = &scopes[0];
        let (prefix_count, prefix_digest) = query_digest(
            connection,
            &scope.query(),
            &[SqlValue::Int(scope.limit as i64)],
        )?;
        return Ok(LogicalState {
            append_count: if prefix_max_id.is_none() {
                geometry.content_count
            } else {
                prefix_count
            },
            append_max_id: BigUint::from_u64(geometry.content_max_id as u64),
            append_prefix_sha256: prefix_digest,
        });
    }
    if kind == "task" {
        require_tables(
            connection,
            &["tasks", "task_notes"],
            &path.display().to_string(),
        )?;
        let row = query_one(
            connection,
            "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM task_notes",
            &path.display().to_string(),
        )?;
        let append_count = cell_nonnegative(&row[0], &format!("{}: note count", path.display()))?;
        let append_max_id = cell_nonnegative(&row[1], &format!("{}: max note id", path.display()))?;
        let limit = prefix_max_id
            .and_then(|held| held.as_u64())
            .map(u128::from)
            .unwrap_or_else(|| append_max_id as u128);
        let selected_fields = if legacy_task_fields {
            "id, task_id, content, created_at, server_comment_id, author_unixname"
        } else {
            "id, task_id, content, created_at"
        };
        let (prefix_count, prefix_digest) = query_digest(
            connection,
            &format!("SELECT {selected_fields} FROM task_notes WHERE id <= ? ORDER BY id"),
            &[SqlValue::Int(limit as i64)],
        )?;
        return Ok(LogicalState {
            append_count: if prefix_max_id.is_none() {
                append_count
            } else {
                prefix_count
            },
            append_max_id: BigUint::from_u64(append_max_id as u64),
            append_prefix_sha256: prefix_digest,
        });
    }
    Err(format!("unsupported Orc source kind {}", py_repr_str(kind)))
}

// ---------------------------------------------------------------- semantic identity

/// Deterministic cache identity for one normalized state.
#[derive(Clone, Debug, PartialEq)]
pub struct SemanticIdentity {
    /// See the enclosing record's documentation.
    pub sha256: String,
    /// See the enclosing record's documentation.
    pub complete_bytes: i64,
}

/// Hash one canonical semantic payload.
pub fn semantic_identity(payload: &Map<String, JsonValue>) -> SemanticIdentity {
    let encoded = crate::json::dump_compact(&JsonValue::Object(payload.clone()));
    SemanticIdentity {
        sha256: sha256_hex(encoded.as_bytes()),
        complete_bytes: encoded.len() as i64,
    }
}

fn identity_payload(entries: Vec<(&str, JsonValue)>) -> Map<String, JsonValue> {
    entries
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

/// A session's semantic identity at one schema version.
pub fn session_semantic_identity(
    source_path: &str,
    owner_session_id: &str,
    state: &LogicalState,
    meta: &SessionMeta,
    auxiliary: &AuxiliaryObservation,
    schema_version: i64,
) -> Result<SemanticIdentity, String> {
    if !(2..=4).contains(&schema_version) {
        return Err(format!(
            "unsupported Orc session semantic schema {schema_version}"
        ));
    }
    Ok(semantic_identity(&identity_payload(vec![
        ("schema_version", JsonValue::from(schema_version)),
        ("kind", JsonValue::String("session".to_string())),
        ("source_path", JsonValue::String(source_path.to_string())),
        (
            "owner_session_id",
            JsonValue::String(owner_session_id.to_string()),
        ),
        ("append_count", JsonValue::from(state.append_count)),
        (
            "append_max_id",
            JsonNumber::from_decimal(&state.append_max_id),
        ),
        (
            "append_prefix_sha256",
            JsonValue::String(state.append_prefix_sha256.clone()),
        ),
        (
            "stable_spawn_sha256",
            JsonValue::String(auxiliary.stable_spawn_sha256.clone()),
        ),
        ("session_id", JsonValue::String(meta.session_id.clone())),
        (
            "parent_id",
            meta.parent_id
                .clone()
                .map_or(JsonValue::Null, JsonValue::String),
        ),
        ("name", JsonValue::String(meta.name.clone())),
        ("created_at_ms", JsonValue::from(meta.created_at_ms)),
    ])))
}

/// A task source's semantic identity at one schema
/// version.
pub fn task_semantic_identity(
    source_path: &str,
    owner_session_id: &str,
    task_source_ordinal: i64,
    state: &LogicalState,
    projection_sha256: &str,
    schema_version: i64,
) -> Result<SemanticIdentity, String> {
    if !(2..=3).contains(&schema_version) {
        return Err(format!(
            "unsupported Orc task semantic schema {schema_version}"
        ));
    }
    Ok(semantic_identity(&identity_payload(vec![
        ("schema_version", JsonValue::from(schema_version)),
        ("kind", JsonValue::String("task".to_string())),
        ("source_path", JsonValue::String(source_path.to_string())),
        (
            "owner_session_id",
            JsonValue::String(owner_session_id.to_string()),
        ),
        ("task_source_ordinal", JsonValue::from(task_source_ordinal)),
        ("append_count", JsonValue::from(state.append_count)),
        (
            "append_max_id",
            JsonNumber::from_decimal(&state.append_max_id),
        ),
        (
            "append_prefix_sha256",
            JsonValue::String(state.append_prefix_sha256.clone()),
        ),
        (
            "task_projection_sha256",
            JsonValue::String(projection_sha256.to_string()),
        ),
    ])))
}

/// The logical state frozen task history authenticates.
pub fn frozen_task_logical_state(records: &[FrozenTaskNote]) -> LogicalState {
    let mut digest = sha2::Sha256::new();
    for record in records {
        digest.update(b"R");
        update_digest(&mut digest, &SqlValue::Int(record.note_id));
        update_digest(&mut digest, &SqlValue::Text(record.task_id.clone()));
        update_digest(&mut digest, &SqlValue::Text(record.content.clone()));
        update_digest(&mut digest, &SqlValue::Text(record.created_at.clone()));
    }
    LogicalState {
        append_count: records.len() as i64,
        append_max_id: BigUint::from_u64(
            records
                .iter()
                .map(|record| record.note_id as u64)
                .max()
                .unwrap_or(0),
        ),
        append_prefix_sha256: hex_digest(&digest),
    }
}

/// (rewrite digest, missing count, missing-ids digest)
/// for frozen history against current notes.
pub fn task_rewrite_observation(
    frozen: &[FrozenTaskNote],
    current: &[ObservedTaskNote],
) -> (String, i64, String) {
    let missing_ids: Vec<i64> = frozen
        .iter()
        .filter(|record| !current.iter().any(|held| held.note_id == record.note_id))
        .map(|record| record.note_id)
        .collect();
    let mut facts: Vec<JsonValue> = Vec::new();
    for record in frozen {
        let observed = current.iter().find(|held| held.note_id == record.note_id);
        match observed {
            None => {
                let mut fact = Map::new();
                fact.insert("note_id".into(), JsonValue::from(record.note_id));
                fact.insert("state".into(), JsonValue::String("missing".to_string()));
                facts.push(JsonValue::Object(fact));
            }
            Some(observed) => {
                if (
                    &observed.server_author,
                    &observed.task_owner,
                    &observed.title,
                ) != (
                    &record.server_author,
                    &record.task_owner,
                    &Some(record.title.clone()),
                ) {
                    let mut fact = Map::new();
                    fact.insert("note_id".into(), JsonValue::from(record.note_id));
                    fact.insert(
                        "state".into(),
                        JsonValue::String("enrichment-changed".to_string()),
                    );
                    fact.insert(
                        "server_author".into(),
                        observed
                            .server_author
                            .clone()
                            .map_or(JsonValue::Null, JsonValue::String),
                    );
                    fact.insert(
                        "task_owner".into(),
                        observed
                            .task_owner
                            .clone()
                            .map_or(JsonValue::Null, JsonValue::String),
                    );
                    fact.insert(
                        "title".into(),
                        observed
                            .title
                            .clone()
                            .map_or(JsonValue::Null, JsonValue::String),
                    );
                    facts.push(JsonValue::Object(fact));
                }
            }
        }
    }
    let facts_empty = facts.is_empty();
    let payload = crate::json::dump_compact(&JsonValue::Array(facts));
    let rewrite_sha = if facts_empty {
        empty_task_rewrite_sha256()
    } else {
        sha256_hex(payload.as_bytes())
    };
    let missing_payload = crate::json::dump_compact(&JsonValue::Array(
        missing_ids.iter().copied().map(JsonValue::from).collect(),
    ));
    (
        rewrite_sha,
        missing_ids.len() as i64,
        sha256_hex(missing_payload.as_bytes()),
    )
}

/// (gap count, gap digest) below the allocation mark.
pub fn task_unobserved_id_gaps(frozen: &[FrozenTaskNote], sequence: i64) -> (i64, String) {
    let mut gaps: Vec<JsonValue> = Vec::new();
    let mut count = 0i64;
    for note_id in 1..=sequence {
        if !frozen.iter().any(|record| record.note_id == note_id) {
            gaps.push(JsonValue::from(note_id));
            count += 1;
        }
    }
    let payload = crate::json::dump_compact(&JsonValue::Array(gaps));
    (count, sha256_hex(payload.as_bytes()))
}

/// Refuse when measured state disagrees with a manifest.
pub fn validate_manifest_logical_state(
    source: &SourceCopy,
    state: &LogicalState,
    where_: &str,
) -> Result<(), String> {
    if state.append_count != source.append_count
        || state.append_max_id != source.append_max_id
        || state.append_prefix_sha256 != source.append_prefix_sha256
    {
        return Err(format!(
            "Orc logical state does not match its manifest: {where_}"
        ));
    }
    Ok(())
}

/// Refuse when a deterministic identity disagrees.
pub fn validate_recorded_semantic_identity(
    source: &SourceCopy,
    identity: &SemanticIdentity,
    where_: &str,
) -> Result<(), String> {
    if source.semantic_identity_mode != SEMANTIC_IDENTITY_DETERMINISTIC {
        return Ok(());
    }
    if source.semantic_sha256 != identity.sha256
        || source.semantic_complete_bytes != identity.complete_bytes
    {
        return Err(format!(
            "Orc deterministic semantic identity does not match artifacts: {where_}"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------- content records

/// See the enclosing record's documentation.
pub const CLASSIFICATION_VERSION: &str = "authorship-v2";
/// See the enclosing record's documentation.
pub const ORC_PERIODIC_REMINDER: &str = "This is your periodic reminder to make sure your running state is aligned with your overarching goals";

/// Decode one optional JSON object stored in a text
/// column.
pub fn optional_json_mapping(
    value: &SqlValue,
    where_: &str,
) -> Result<Map<String, JsonValue>, String> {
    let Some(text) = cell_optional_string(value, where_)? else {
        return Ok(Map::new());
    };
    let decoded: JsonValue =
        serde_json::from_str(&text).map_err(|error| format!("{where_}: invalid JSON ({error})"))?;
    match decoded.as_object() {
        Some(held) => Ok(held.clone()),
        None => Err(format!("{where_}: expected an object")),
    }
}

/// Decode structured modern provenance; named system
/// sources read as empty.
pub fn modern_source_mapping(
    value: Option<&JsonValue>,
    where_: &str,
) -> Result<Map<String, JsonValue>, String> {
    match value {
        None | Some(JsonValue::Null) => Ok(Map::new()),
        Some(JsonValue::String(text)) if text.is_empty() => Ok(Map::new()),
        Some(JsonValue::String(text)) => {
            if !text.starts_with('{') {
                return Ok(Map::new());
            }
            let decoded: JsonValue = serde_json::from_str(text)
                .map_err(|error| format!("{where_}: invalid JSON ({error})"))?;
            match decoded.as_object() {
                Some(held) => Ok(held.clone()),
                None => Err(format!("{where_}: expected an object")),
            }
        }
        Some(_) => Err(format!("{where_}: expected a string or null")),
    }
}

/// A nested object, or empty.
pub fn nested_mapping(raw: &Map<String, JsonValue>, key: &str) -> Map<String, JsonValue> {
    raw.get(key)
        .and_then(JsonValue::as_object)
        .cloned()
        .unwrap_or_default()
}

fn first_string(values: &[Option<&JsonValue>]) -> Option<String> {
    for value in values {
        if let Some(JsonValue::String(text)) = value {
            if !text.is_empty() {
                return Some(text.clone());
            }
        }
    }
    None
}

/// Whether the text is one of the two scheduled families.
pub fn is_scheduled_orc_input(text: &str) -> bool {
    if text.starts_with(ORC_PERIODIC_REMINDER) {
        return true;
    }
    let Some(rest) =
        text.strip_prefix("We're working in eastern time and the current date/time is `")
    else {
        return false;
    };
    // \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} (EDT|EST)`, use that to orient
    let bytes = rest.as_bytes();
    let digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let shape = bytes.len() >= 27
        && digit(0)
        && digit(1)
        && digit(2)
        && digit(3)
        && bytes[4] == b'-'
        && digit(5)
        && digit(6)
        && bytes[7] == b'-'
        && digit(8)
        && digit(9)
        && bytes[10] == b' '
        && digit(11)
        && digit(12)
        && bytes[13] == b':'
        && digit(14)
        && digit(15)
        && bytes[16] == b':'
        && digit(17)
        && digit(18)
        && bytes[19] == b' ';
    if !shape {
        return false;
    }
    let tail = &rest[20..];
    (tail.starts_with("EDT") || tail.starts_with("EST"))
        && tail[3..].starts_with("`, use that to orient ")
}

/// (kind, author, ingress, author kind, native id) for
/// one user text block.
#[allow(clippy::too_many_arguments)]
pub fn orc_input_provenance(
    user_source: &Map<String, JsonValue>,
    extra: &Map<String, JsonValue>,
    text: &str,
    message_id: &str,
    owner_gchat_senders: &[String],
) -> Result<(String, Option<String>, String, String, String), String> {
    let orc = nested_mapping(user_source, "Orc");
    if user_source.contains_key("Orc") {
        let author = first_string(&[orc.get("sender_session"), extra.get("sender_session")]);
        return Ok((
            "inter_agent_message".to_string(),
            author,
            "orc".to_string(),
            "agent".to_string(),
            message_id.to_string(),
        ));
    }
    let gchat = nested_mapping(user_source, "GChat");
    if user_source.contains_key("GChat") {
        let mut explicit_owner: Option<bool> = match gchat.get("is_owner") {
            Some(JsonValue::Bool(flag)) => Some(*flag),
            _ => match extra.get("is_owner") {
                Some(JsonValue::Bool(flag)) => Some(*flag),
                _ => None,
            },
        };
        let author = first_string(&[
            gchat.get("sender_unixname"),
            extra.get("sender_unixname"),
            gchat.get("sender_display_name"),
            extra.get("sender_display_name"),
            gchat.get("sender_name"),
            extra.get("sender_name"),
        ]);
        if explicit_owner.is_none() {
            if let Some(author) = &author {
                if owner_gchat_senders.contains(author) {
                    explicit_owner = Some(true);
                } else if !owner_gchat_senders.is_empty() {
                    explicit_owner = Some(false);
                }
            }
        }
        let native_id = first_string(&[
            gchat.get("message_name"),
            extra.get("message_name"),
            Some(&JsonValue::String(message_id.to_string())),
        ]);
        let Some(native_id) = native_id else {
            return Err("GChat input lacks a native message identity".to_string());
        };
        let (event_kind, author_kind) = if explicit_owner == Some(false) {
            ("external_message", "other_human")
        } else if explicit_owner == Some(true) {
            ("user_prompt", "owner_human")
        } else {
            ("user_prompt", "unknown")
        };
        return Ok((
            event_kind.to_string(),
            author,
            "gchat".to_string(),
            author_kind.to_string(),
            native_id,
        ));
    }
    let submitted = nested_mapping(user_source, "Submitted");
    let submitted_source = submitted.get("source");
    let is_tui = submitted_source.and_then(JsonValue::as_str) == Some("Tui");
    let is_web = submitted_source
        .and_then(JsonValue::as_object)
        .is_some_and(|held| held.contains_key("Web"));
    if (is_tui || is_web) && is_scheduled_orc_input(text) {
        return Ok((
            "system_input".to_string(),
            Some("system".to_string()),
            "scheduled".to_string(),
            "system".to_string(),
            message_id.to_string(),
        ));
    }
    if is_tui {
        return Ok((
            "user_prompt".to_string(),
            None,
            "tui".to_string(),
            "unknown".to_string(),
            message_id.to_string(),
        ));
    }
    if is_web {
        return Ok((
            "user_prompt".to_string(),
            None,
            "submitted_web".to_string(),
            "external_or_unknown".to_string(),
            message_id.to_string(),
        ));
    }
    Ok((
        "user_prompt".to_string(),
        None,
        "orc_unknown".to_string(),
        "unknown".to_string(),
        message_id.to_string(),
    ))
}

fn session_prefix8(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

/// Count `orc.<name>(` calls in code text.
pub fn orc_tool_counts(code: &str) -> Vec<(String, i64)> {
    let chars: Vec<char> = code.chars().collect();
    let mut counts: Vec<(String, i64)> = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        // \borc\.
        let boundary =
            index == 0 || !(chars[index - 1].is_ascii_alphanumeric() || chars[index - 1] == '_');
        if boundary && chars[index..].starts_with(&['o', 'r', 'c', '.']) {
            let mut cursor = index + 4;
            if cursor < chars.len() && (chars[cursor].is_ascii_alphabetic() || chars[cursor] == '_')
            {
                let start = cursor;
                while cursor < chars.len()
                    && (chars[cursor].is_ascii_alphanumeric() || chars[cursor] == '_')
                {
                    cursor += 1;
                }
                let mut after = cursor;
                while after < chars.len() && chars[after].is_whitespace() {
                    after += 1;
                }
                if after < chars.len() && chars[after] == '(' {
                    let name: String = chars[start..cursor].iter().collect();
                    match counts.iter_mut().find(|(held, _)| *held == name) {
                        Some((_, count)) => *count += 1,
                        None => counts.push((name, 1)),
                    }
                    index = after + 1;
                    continue;
                }
            }
        }
        index += 1;
    }
    counts.sort();
    counts
}

/// One spawn an event stream can attribute work to.
#[derive(Clone, Debug)]
pub struct Spawn {
    /// See the enclosing record's documentation.
    pub thread_id: String,
    /// See the enclosing record's documentation.
    pub parent_thread_id: String,
    /// See the enclosing record's documentation.
    pub official_name: String,
    /// See the enclosing record's documentation.
    pub timestamp_ms: i64,
    /// See the enclosing record's documentation.
    pub source_line: i64,
    /// See the enclosing record's documentation.
    pub source_path: String,
}

/// Spawns proven by a session's stable AgentBlocks.
pub fn conversation_spawns(
    path: &Path,
    meta: &SessionMeta,
    start_at_ms: Option<i64>,
    start_source_line: Option<i64>,
) -> Result<Vec<Spawn>, String> {
    let observation = auxiliary_observation(path, meta)?;
    let mut result: Vec<Spawn> = observation
        .stable_spawns
        .iter()
        .filter(|record| match start_source_line {
            Some(line) => record.message_id >= line,
            None => start_at_ms.is_none_or(|start| record.timestamp_ms >= start),
        })
        .map(|record| Spawn {
            thread_id: format!(
                "orc-agent-{}-{}",
                session_prefix8(&meta.session_id),
                record.block_id
            ),
            parent_thread_id: meta.session_id.clone(),
            official_name: record.agent_id.clone(),
            timestamp_ms: record.timestamp_ms,
            source_line: record.block_id,
            source_path: meta.source_path.clone(),
        })
        .collect();
    result.sort_by_key(|held| (held.timestamp_ms, held.source_line));
    Ok(result)
}

/// Where a v2 block was first recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct ModernBlockOrigin {
    /// See the enclosing record's documentation.
    pub source_line: i64,
    /// See the enclosing record's documentation.
    pub native_message_id: String,
}

/// Origins of v2 blocks in the modern messages table.
pub fn modern_block_origins(path: &Path) -> Result<Vec<(String, ModernBlockOrigin)>, String> {
    let connection = read_only(path)?;
    if session_storage_table(&connection, &path.display().to_string())? != "messages" {
        return Ok(Vec::new());
    }
    let rows = query_all(
        &connection,
        "SELECT id, message_json FROM messages ORDER BY id",
        &[],
        &path.display().to_string(),
    )?;
    drop(connection);
    let mut result: Vec<(String, ModernBlockOrigin)> = Vec::new();
    for row in &rows {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        let source_line = cell_nonnegative(&cell(0), &format!("{}: message id", path.display()))?;
        let raw_json = cell_string(&cell(1), &format!("{}: message_json", path.display()))?;
        let decoded: JsonValue = serde_json::from_str(&raw_json).map_err(|error| {
            format!(
                "{}: invalid message_json at row {source_line}: {error}",
                path.display()
            )
        })?;
        let message = decoded.as_object().ok_or_else(|| {
            format!(
                "{}: message_json[{source_line}]: expected an object",
                path.display()
            )
        })?;
        let native_message_id = required_string(
            message.get("id"),
            &format!("{}: message_json[{source_line}].id", path.display()),
        )?;
        let blocks = match message.get("blocks") {
            Some(JsonValue::Array(items)) => items.clone(),
            _ => {
                return Err(format!(
                    "{}: message_json[{source_line}].blocks: expected an array",
                    path.display()
                ))
            }
        };
        for raw_block in &blocks {
            let block = raw_block
                .as_object()
                .ok_or_else(|| format!("{}: message block: expected an object", path.display()))?;
            let Some(block_id) = block.get("id").and_then(|held| match held {
                JsonValue::Number(number) if !number.is_f64() => number.as_i64(),
                _ => None,
            }) else {
                continue;
            };
            let identity = format!("v2-block-{block_id}");
            let origin = ModernBlockOrigin {
                source_line,
                native_message_id: native_message_id.clone(),
            };
            match result.iter_mut().find(|(held, _)| *held == identity) {
                Some((_, prior)) => {
                    if *prior != origin {
                        return Err(format!(
                            "{}: modern block identity {} is duplicated",
                            path.display(),
                            py_repr_str(&identity)
                        ));
                    }
                }
                None => result.push((identity, origin)),
            }
        }
    }
    Ok(result)
}

type ContentRecords = (Vec<Event>, Vec<ToolCall>, Vec<Turn>);

type TurnBounds = (i64, i64, Option<String>);

/// Normalize the legacy content_blocks stream.
#[allow(clippy::too_many_arguments)]
pub fn legacy_content_records(
    path: &Path,
    meta: &SessionMeta,
    id_namespace: &str,
    start_at_ms: Option<i64>,
    start_source_line: Option<i64>,
    modern_origins: &[(String, ModernBlockOrigin)],
) -> Result<ContentRecords, String> {
    let rows = {
        let connection = read_only(path)?;
        let rows = query_all(
            &connection,
            "SELECT rowid, id, message_id, created_at_ms, turn_index, role, block_type, \
             content, code_input, code_output, code_exit_code, user_source, extra \
             FROM content_blocks ORDER BY created_at_ms, rowid",
            &[],
            &path.display().to_string(),
        )?;
        drop(connection);
        rows
    };
    let origin_of = |block_id: &str| -> Option<&ModernBlockOrigin> {
        modern_origins
            .iter()
            .find(|(held, _)| held == block_id)
            .map(|(_, origin)| origin)
    };
    let row_is_included = |row: &[SqlValue]| -> Result<bool, String> {
        let timestamp_ms = cell_integer(&row[3], &format!("{}: created_at_ms", path.display()))?;
        if let Some(line) = start_source_line {
            let block_id = cell_string(&row[1], &format!("{}: block id", path.display()))?;
            return Ok(origin_of(&block_id).is_some_and(|origin| origin.source_line >= line));
        }
        Ok(start_at_ms.is_none_or(|start| timestamp_ms >= start))
    };
    let mut owner_gchat_senders: Vec<String> = Vec::new();
    for row in &rows {
        if !row_is_included(row)? {
            continue;
        }
        let role = cell_string(&row[5], &format!("{}: role", path.display()))?;
        if role != "user" {
            continue;
        }
        let user_source = optional_json_mapping(
            &row[11],
            &format!("{}: content block user_source", path.display()),
        )?;
        if !user_source.contains_key("GChat") {
            continue;
        }
        let gchat = nested_mapping(&user_source, "GChat");
        let extra = optional_json_mapping(
            &row[12],
            &format!("{}: content block extra", path.display()),
        )?;
        let explicit_owner = match gchat.get("is_owner") {
            Some(JsonValue::Bool(flag)) => Some(*flag),
            _ => match extra.get("is_owner") {
                Some(JsonValue::Bool(flag)) => Some(*flag),
                _ => None,
            },
        };
        if explicit_owner != Some(true) {
            continue;
        }
        for value in [
            gchat.get("sender_unixname"),
            extra.get("sender_unixname"),
            gchat.get("sender_display_name"),
            extra.get("sender_display_name"),
            gchat.get("sender_name"),
            extra.get("sender_name"),
        ] {
            if let Some(JsonValue::String(text)) = value {
                if !text.is_empty() && !owner_gchat_senders.contains(text) {
                    owner_gchat_senders.push(text.clone());
                }
            }
        }
    }
    let mut events: Vec<Event> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut turn_bounds: Vec<(i64, TurnBounds)> = Vec::new();
    for row in &rows {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        let rowid = cell_integer(&cell(0), &format!("{}: rowid", path.display()))?;
        let block_id = cell_string(&cell(1), &format!("{}: block id", path.display()))?;
        let message_id = cell_string(&cell(2), &format!("{}: message id", path.display()))?;
        let timestamp_ms = cell_integer(&cell(3), &format!("{}: created_at_ms", path.display()))?;
        if !row_is_included(row)? {
            continue;
        }
        let modern_origin = origin_of(&block_id);
        let turn_index = cell_integer(&cell(4), &format!("{}: turn_index", path.display()))?;
        let role = cell_string(&cell(5), &format!("{}: role", path.display()))?;
        let block_type = cell_string(&cell(6), &format!("{}: block_type", path.display()))?;
        let content = cell_optional_string(&cell(7), &format!("{}: content", path.display()))?;
        let turn_id = format!(
            "{id_namespace}orc-turn-{}-{turn_index}",
            session_prefix8(&meta.session_id)
        );
        match turn_bounds.iter_mut().find(|(held, _)| *held == turn_index) {
            None => turn_bounds.push((
                turn_index,
                (
                    timestamp_ms,
                    timestamp_ms + 1,
                    if matches!(role.as_str(), "assistant" | "notification") {
                        content.clone()
                    } else {
                        None
                    },
                ),
            )),
            Some((_, bounds)) => {
                let last_message = if matches!(role.as_str(), "assistant" | "notification") {
                    content
                        .clone()
                        .filter(|held| !held.is_empty())
                        .or_else(|| bounds.2.clone())
                } else {
                    bounds.2.clone()
                };
                *bounds = (
                    bounds.0.min(timestamp_ms),
                    bounds.1.max(timestamp_ms + 1),
                    last_message,
                );
            }
        }
        let mut event_kind: Option<String> = None;
        let mut event_role: Option<String> = None;
        let mut event_author: Option<String> = None;
        let mut event_recipient: Option<String> = None;
        let mut ingress_kind: Option<String> = None;
        let mut author_kind: Option<String> = None;
        let mut source_native_id = message_id.clone();
        if block_type == "text" && role == "user" {
            let user_source = optional_json_mapping(
                &cell(11),
                &format!("{}: content block {block_id} user_source", path.display()),
            )?;
            let extra = optional_json_mapping(
                &cell(12),
                &format!("{}: content block {block_id} extra", path.display()),
            )?;
            let (kind, author, ingress, akind, native) = orc_input_provenance(
                &user_source,
                &extra,
                content.as_deref().unwrap_or(""),
                &message_id,
                &owner_gchat_senders,
            )?;
            event_kind = Some(kind.clone());
            event_author = author;
            ingress_kind = Some(ingress);
            author_kind = Some(akind);
            source_native_id = native;
            if kind == "inter_agent_message" {
                event_role = None;
                event_recipient = Some(meta.session_id.clone());
            } else if kind == "system_input" {
                event_role = Some("system".to_string());
            } else {
                event_role = Some("user".to_string());
            }
        } else if block_type == "text" && matches!(role.as_str(), "assistant" | "notification") {
            event_kind = Some("assistant_message".to_string());
            event_role = Some("assistant".to_string());
            event_author = Some(meta.session_id.clone());
            ingress_kind = Some("orc".to_string());
            author_kind = Some("agent".to_string());
        }
        if let (Some(kind), Some(content_text)) = (&event_kind, &content) {
            if !content_text.is_empty() {
                events.push(Event {
                    event_id: format!("{id_namespace}orc-block-{block_id}"),
                    thread_id: meta.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    timestamp_ms,
                    kind: kind.clone(),
                    role: event_role.clone(),
                    phase: None,
                    text: Some(content_text.clone()),
                    content_availability: "plaintext".to_string(),
                    encrypted_content: None,
                    author: event_author.clone(),
                    recipient: event_recipient.clone(),
                    source_line: modern_origin.map_or(rowid, |origin| origin.source_line),
                    ingress_kind: ingress_kind.clone(),
                    author_kind: author_kind.clone(),
                    source_native_id: Some(
                        modern_origin.map_or(source_native_id.clone(), |origin| {
                            origin.native_message_id.clone()
                        }),
                    ),
                    classification_version: Some(CLASSIFICATION_VERSION.to_string()),
                });
            }
        }
        if block_type == "code_execution" {
            let code_input =
                cell_optional_string(&cell(8), &format!("{}: code_input", path.display()))?;
            let code_output =
                cell_optional_string(&cell(9), &format!("{}: code_output", path.display()))?;
            let status = match &cell(10) {
                SqlValue::Int(code) if *code != 0 => "failed",
                _ => "completed",
            };
            tools.push(ToolCall {
                call_id: format!("{id_namespace}orc-code-{block_id}"),
                item_id: Some(format!("{id_namespace}{block_id}")),
                thread_id: meta.session_id.clone(),
                turn_id: Some(turn_id),
                name: "code_execution".to_string(),
                namespace: Some("orc".to_string()),
                started_at_ms: timestamp_ms,
                ended_at_ms: Some(timestamp_ms + 1),
                status: status.to_string(),
                input_text: code_input.clone(),
                output_text: code_output,
                nested_tools: orc_tool_counts(code_input.as_deref().unwrap_or("")),
                source_line: modern_origin.map_or(rowid, |origin| origin.source_line),
                input_payload: None,
                output_payload: None,
            });
        }
    }
    turn_bounds.sort_by_key(|(index, _)| *index);
    let turns = turn_bounds
        .into_iter()
        .map(|(turn_index, bounds)| Turn {
            turn_id: format!(
                "{id_namespace}orc-turn-{}-{turn_index}",
                session_prefix8(&meta.session_id)
            ),
            thread_id: meta.session_id.clone(),
            started_at_ms: bounds.0,
            ended_at_ms: Some(bounds.1),
            status: "completed".to_string(),
            first_token_ms: None,
            error: None,
            last_agent_message: bounds.2,
        })
        .collect();
    Ok((events, tools, turns))
}

/// Normalize the append-only schema-v5 messages table.
pub fn modern_content_records(
    path: &Path,
    meta: &SessionMeta,
    id_namespace: &str,
    start_at_ms: Option<i64>,
    start_source_line: Option<i64>,
) -> Result<ContentRecords, String> {
    let (rows, content_rows) = {
        let connection = read_only(path)?;
        let rows = query_all(
            &connection,
            "SELECT id, session_id, role, created_at_ms, message_json FROM messages ORDER BY id",
            &[],
            &path.display().to_string(),
        )?;
        let content_rows = query_all(
            &connection,
            "SELECT id, turn_index FROM content_blocks ORDER BY rowid",
            &[],
            &path.display().to_string(),
        )?;
        drop(connection);
        (rows, content_rows)
    };
    let mut existing_block_turns: Vec<(i64, i64)> = Vec::new();
    for row in &content_rows {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        let block_identity =
            cell_string(&cell(0), &format!("{}: content block id", path.display()))?;
        let Some(inner) = block_identity.strip_prefix("v2-block-") else {
            continue;
        };
        if inner.is_empty() || !inner.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(block_number) = inner.parse::<i64>() else {
            continue;
        };
        existing_block_turns.push((
            block_number,
            cell_integer(
                &cell(1),
                &format!("{}: content block turn_index", path.display()),
            )?,
        ));
    }
    let turn_of_block = |block_id: i64| -> Option<i64> {
        existing_block_turns
            .iter()
            .find(|(held, _)| *held == block_id)
            .map(|(_, held)| *held)
    };
    let message_is_included = |message_id: i64, timestamp_ms: i64| -> bool {
        match start_source_line {
            Some(line) => message_id >= line,
            None => start_at_ms.is_none_or(|start| timestamp_ms >= start),
        }
    };
    let mut parsed: Vec<(i64, i64, String, Map<String, JsonValue>)> = Vec::new();
    let mut owner_gchat_senders: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let cell = |i: usize| row.get(i).cloned().unwrap_or(SqlValue::Null);
        let where_ = format!("{}: messages[{index}]", path.display());
        let _ = &where_;
        let message_id = cell_nonnegative(&cell(0), &format!("{}: message id", path.display()))?;
        let session_id = cell_string(&cell(1), &format!("{}: message session_id", path.display()))?;
        if session_id != meta.session_id {
            return Err(format!(
                "{}: message {message_id} belongs to {}, not {}",
                path.display(),
                py_repr_str(&session_id),
                py_repr_str(&meta.session_id)
            ));
        }
        let stored_role = cell_string(&cell(2), &format!("{}: message role", path.display()))?;
        let timestamp_ms = cell_nonnegative(
            &cell(3),
            &format!("{}: message created_at_ms", path.display()),
        )?;
        let raw_json = cell_string(&cell(4), &format!("{}: message_json", path.display()))?;
        let decoded: JsonValue = serde_json::from_str(&raw_json).map_err(|error| {
            format!(
                "{}: invalid message_json for row {message_id}: {error}",
                path.display()
            )
        })?;
        let message = decoded.as_object().cloned().ok_or_else(|| {
            format!(
                "{}: message_json[{message_id}]: expected an object",
                path.display()
            )
        })?;
        let role = required_string(
            message.get("role"),
            &format!("{}: message_json[{message_id}].role", path.display()),
        )?;
        if role.to_lowercase() != stored_role.to_lowercase() {
            return Err(format!(
                "{}: role differs for message row {message_id}",
                path.display()
            ));
        }
        let embedded_at = nonnegative_integer(
            message.get("created_at_ms"),
            &format!(
                "{}: message_json[{message_id}].created_at_ms",
                path.display()
            ),
        )?;
        if embedded_at != timestamp_ms {
            return Err(format!(
                "{}: timestamp differs for message row {message_id}",
                path.display()
            ));
        }
        let source = modern_source_mapping(
            message.get("source"),
            &format!("{}: message_json[{message_id}].source", path.display()),
        )?;
        if message_is_included(message_id, timestamp_ms)
            && role.to_lowercase() == "user"
            && source.contains_key("GChat")
        {
            let gchat = nested_mapping(&source, "GChat");
            if gchat.get("is_owner") == Some(&JsonValue::Bool(true)) {
                for value in [
                    gchat.get("sender_unixname"),
                    gchat.get("sender_display_name"),
                    gchat.get("sender_name"),
                ] {
                    if let Some(JsonValue::String(text)) = value {
                        if !text.is_empty() && !owner_gchat_senders.contains(text) {
                            owner_gchat_senders.push(text.clone());
                        }
                    }
                }
            }
        }
        parsed.push((message_id, timestamp_ms, role, message));
    }
    let mut events: Vec<Event> = Vec::new();
    let mut tools: Vec<ToolCall> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();
    let mut current_turn_index = 0i64;
    for (message_id, timestamp_ms, role, message) in &parsed {
        let timestamp_ms = *timestamp_ms;
        if timestamp_ms == 0 {
            continue;
        }
        let blocks: Vec<JsonValue> = match message.get("blocks") {
            Some(JsonValue::Array(items)) => items.clone(),
            _ => {
                return Err(format!(
                    "{}: message_json[{message_id}].blocks: expected an array",
                    path.display()
                ))
            }
        };
        let mut known_turns: Vec<i64> = Vec::new();
        for raw_block in &blocks {
            let block = raw_block
                .as_object()
                .ok_or_else(|| format!("{}: message block: expected an object", path.display()))?;
            if let Some(JsonValue::Number(number)) = block.get("id") {
                if !number.is_f64() {
                    if let Some(id) = number.as_i64() {
                        if let Some(turn) = turn_of_block(id) {
                            if !known_turns.contains(&turn) {
                                known_turns.push(turn);
                            }
                        }
                    }
                }
            }
        }
        if known_turns.len() > 1 {
            return Err(format!(
                "{}: message row {message_id} spans multiple legacy turns",
                path.display()
            ));
        }
        let turn_index = if let Some(known) = known_turns.first() {
            current_turn_index = *known;
            *known
        } else {
            if role.to_lowercase() == "user" || current_turn_index == 0 {
                current_turn_index += 1;
            }
            current_turn_index
        };
        if !message_is_included(*message_id, timestamp_ms) {
            continue;
        }
        let native_message_id = required_string(
            message.get("id"),
            &format!("{}: message_json[{message_id}].id", path.display()),
        )?;
        let turn_id = format!(
            "{id_namespace}orc-turn-{}-{turn_index}",
            session_prefix8(&meta.session_id)
        );
        let mut last_agent_message: Option<String> = None;
        let mut error_text: Option<String> = None;
        let mut emitted = false;
        let source = modern_source_mapping(
            message.get("source"),
            &format!("{}: message_json[{message_id}].source", path.display()),
        )?;
        for (block_index, raw_block) in blocks.iter().enumerate() {
            let block = raw_block.as_object().ok_or_else(|| {
                format!(
                    "{}: message_json[{message_id}].blocks[{block_index}]: expected an object",
                    path.display()
                )
            })?;
            let block_type = required_string(
                block.get("type"),
                &format!(
                    "{}: message_json[{message_id}].blocks[{block_index}].type",
                    path.display()
                ),
            )?;
            let Some(block_id) = (match block.get("id") {
                Some(JsonValue::Number(number)) if !number.is_f64() => number.as_i64(),
                _ => None,
            }) else {
                continue;
            };
            if turn_of_block(block_id).is_some() {
                continue;
            }
            if block_type == "ErrorBlock" {
                error_text = optional_string(
                    block.get("message"),
                    &format!(
                        "{}: message_json[{message_id}].blocks[{block_index}].message",
                        path.display()
                    ),
                )?;
                emitted = true;
                continue;
            }
            if block_type == "CodeExecutionBlock" {
                let code_input = optional_string(
                    block.get("code"),
                    &format!(
                        "{}: message_json[{message_id}].blocks[{block_index}].code",
                        path.display()
                    ),
                )?;
                let code_output = optional_string(
                    block.get("output"),
                    &format!(
                        "{}: message_json[{message_id}].blocks[{block_index}].output",
                        path.display()
                    ),
                )?;
                let is_error = match block.get("is_error") {
                    Some(JsonValue::Bool(flag)) => *flag,
                    _ => {
                        return Err(format!(
                            "{}: CodeExecutionBlock {block_id} lacks is_error",
                            path.display()
                        ))
                    }
                };
                let identity = format!("v2-block-{block_id}");
                tools.push(ToolCall {
                    call_id: format!("{id_namespace}orc-code-{identity}"),
                    item_id: Some(format!("{id_namespace}{identity}")),
                    thread_id: meta.session_id.clone(),
                    turn_id: Some(turn_id.clone()),
                    name: "code_execution".to_string(),
                    namespace: Some("orc".to_string()),
                    started_at_ms: timestamp_ms,
                    ended_at_ms: Some(timestamp_ms + 1),
                    status: if is_error { "failed" } else { "completed" }.to_string(),
                    input_text: code_input.clone(),
                    output_text: code_output,
                    nested_tools: orc_tool_counts(code_input.as_deref().unwrap_or("")),
                    source_line: *message_id,
                    input_payload: None,
                    output_payload: None,
                });
                emitted = true;
                continue;
            }
            let text_key = if matches!(block_type.as_str(), "text" | "NotificationBlock") {
                Some("text")
            } else {
                None
            };
            let Some(text_key) = text_key else {
                continue;
            };
            let Some(content) = optional_string(
                block.get(text_key),
                &format!(
                    "{}: message_json[{message_id}].blocks[{block_index}].text",
                    path.display()
                ),
            )?
            else {
                continue;
            };
            let role_key = role.to_lowercase();
            let (
                event_kind,
                event_role,
                event_author,
                event_recipient,
                ingress_kind,
                author_kind,
                source_native_id,
            );
            if role_key == "user" {
                let (kind, author, ingress, akind, native) = orc_input_provenance(
                    &source,
                    &Map::new(),
                    &content,
                    &native_message_id,
                    &owner_gchat_senders,
                )?;
                event_kind = kind.clone();
                event_author = author;
                ingress_kind = ingress;
                author_kind = akind;
                source_native_id = native;
                if kind == "inter_agent_message" {
                    event_role = None;
                    event_recipient = Some(meta.session_id.clone());
                } else if kind == "system_input" {
                    event_role = Some("system".to_string());
                    event_recipient = None;
                } else {
                    event_role = Some("user".to_string());
                    event_recipient = None;
                }
            } else if role_key == "system" {
                event_kind = "system_input".to_string();
                event_role = Some("system".to_string());
                event_author = Some("system".to_string());
                event_recipient = None;
                ingress_kind = "orc".to_string();
                author_kind = "system".to_string();
                source_native_id = native_message_id.clone();
            } else {
                event_kind = "assistant_message".to_string();
                event_role = Some("assistant".to_string());
                event_author = Some(meta.session_id.clone());
                event_recipient = None;
                ingress_kind = "orc".to_string();
                author_kind = "agent".to_string();
                source_native_id = native_message_id.clone();
                last_agent_message = Some(content.clone());
            }
            events.push(Event {
                event_id: format!("{id_namespace}orc-block-v2-block-{block_id}"),
                thread_id: meta.session_id.clone(),
                turn_id: Some(turn_id.clone()),
                timestamp_ms,
                kind: event_kind,
                role: event_role,
                phase: None,
                text: Some(content),
                content_availability: "plaintext".to_string(),
                encrypted_content: None,
                author: event_author,
                recipient: event_recipient,
                source_line: *message_id,
                ingress_kind: Some(ingress_kind),
                author_kind: Some(author_kind),
                source_native_id: Some(source_native_id),
                classification_version: Some(CLASSIFICATION_VERSION.to_string()),
            });
            emitted = true;
        }
        if emitted {
            turns.push(Turn {
                turn_id,
                thread_id: meta.session_id.clone(),
                started_at_ms: timestamp_ms,
                ended_at_ms: Some(timestamp_ms + 1),
                status: if error_text.is_some() {
                    "failed"
                } else {
                    "completed"
                }
                .to_string(),
                first_token_ms: None,
                error: error_text,
                last_agent_message,
            });
        }
    }
    Ok((events, tools, turns))
}

/// Both storage generations of one session, merged.
pub fn content_records(
    path: &Path,
    meta: &SessionMeta,
    id_namespace: &str,
    start_at_ms: Option<i64>,
    start_source_line: Option<i64>,
) -> Result<ContentRecords, String> {
    let modern = {
        let connection = read_only(path)?;
        let held = session_storage_table(&connection, &path.display().to_string())? == "messages";
        drop(connection);
        held
    };
    let origins = if modern {
        modern_block_origins(path)?
    } else {
        Vec::new()
    };
    let legacy = legacy_content_records(
        path,
        meta,
        id_namespace,
        start_at_ms,
        start_source_line,
        &origins,
    )?;
    if !modern {
        return Ok(legacy);
    }
    let additions =
        modern_content_records(path, meta, id_namespace, start_at_ms, start_source_line)?;
    let (legacy_events, legacy_tools, legacy_turns) = legacy;
    let (mut events, mut tools, addition_turns) = additions;
    let mut all_events = legacy_events;
    all_events.append(&mut events);
    let mut all_tools = legacy_tools;
    all_tools.append(&mut tools);
    let mut turns_by_id: Vec<Turn> = legacy_turns;
    for turn in addition_turns {
        match turns_by_id
            .iter_mut()
            .find(|held| held.turn_id == turn.turn_id)
        {
            None => turns_by_id.push(turn),
            Some(prior) => {
                let status = if prior.status == "failed" || turn.status == "failed" {
                    "failed".to_string()
                } else {
                    prior.status.clone()
                };
                *prior = Turn {
                    turn_id: prior.turn_id.clone(),
                    thread_id: prior.thread_id.clone(),
                    started_at_ms: prior.started_at_ms.min(turn.started_at_ms),
                    ended_at_ms: Some(
                        prior
                            .ended_at_ms
                            .unwrap_or(prior.started_at_ms)
                            .max(turn.ended_at_ms.unwrap_or(turn.started_at_ms)),
                    ),
                    status,
                    first_token_ms: prior.first_token_ms,
                    error: prior.error.clone().or(turn.error.clone()),
                    last_agent_message: turn
                        .last_agent_message
                        .clone()
                        .or(prior.last_agent_message.clone()),
                };
            }
        }
    }
    turns_by_id.sort_by(|a, b| (a.started_at_ms, &a.turn_id).cmp(&(b.started_at_ms, &b.turn_id)));
    Ok((all_events, all_tools, turns_by_id))
}

/// Hash normalized transcript semantics independently
/// of storage layout.
pub fn session_canonical_state(path: &Path, meta: &SessionMeta) -> Result<LogicalState, String> {
    let (mut events, mut tools, mut turns) = content_records(path, meta, "", None, None)?;
    events.sort_by(|a, b| (a.timestamp_ms, &a.event_id).cmp(&(b.timestamp_ms, &b.event_id)));
    tools.sort_by(|a, b| (a.started_at_ms, &a.call_id).cmp(&(b.started_at_ms, &b.call_id)));
    turns.sort_by(|a, b| (a.started_at_ms, &a.turn_id).cmp(&(b.started_at_ms, &b.turn_id)));
    let mut root = Map::new();
    root.insert(
        "events".into(),
        JsonValue::Array(events.iter().map(crate::team_data::event_to_json).collect()),
    );
    root.insert(
        "tool_calls".into(),
        JsonValue::Array(tools.iter().map(crate::team_data::tool_to_json).collect()),
    );
    root.insert(
        "turns".into(),
        JsonValue::Array(turns.iter().map(crate::team_data::turn_to_json).collect()),
    );
    let payload = crate::json::dump_compact(&JsonValue::Object(root));
    Ok(LogicalState {
        append_count: (events.len() + tools.len() + turns.len()) as i64,
        append_max_id: BigUint::zero(),
        append_prefix_sha256: sha256_hex(payload.as_bytes()),
    })
}

// ---------------------------------------------------------------- semantic validation

/// Refuse when a legacy-raw baseline no longer proves
/// the recorded identity.
pub fn validate_legacy_semantic_identity(
    snapshot_root: &Path,
    source: &SourceCopy,
    current_identity: &SemanticIdentity,
    projection_sha256: Option<&str>,
) -> Result<(), String> {
    if source.semantic_identity_mode != SEMANTIC_IDENTITY_LEGACY {
        return Ok(());
    }
    let Some(baseline_relative) = &source.semantic_baseline_path else {
        return Err(format!(
            "legacy Orc semantic identity lacks a baseline for {}",
            source.source_path
        ));
    };
    let baseline_path = snapshot_path(snapshot_root, baseline_relative)?;
    let is_symlink = std::fs::symlink_metadata(&baseline_path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink || !baseline_path.is_file() {
        return Err(format!(
            "legacy Orc semantic baseline is missing or unsafe: {}",
            baseline_path.display()
        ));
    }
    let size = std::fs::metadata(&baseline_path)
        .map(|meta| meta.len() as i64)
        .map_err(|error| crate::json::os_error(&error, &baseline_path))?;
    if sha256_file(&baseline_path)? != source.semantic_sha256
        || size != source.semantic_complete_bytes
    {
        return Err(format!(
            "legacy Orc semantic baseline does not match its cache identity: {}",
            baseline_path.display()
        ));
    }
    let baseline_state = logical_state(&baseline_path, &source.kind, None, false, None)?;
    let baseline_identity = if source.kind == "session" {
        let baseline_meta = session_meta(&baseline_path, &source.source_path)?;
        let baseline_auxiliary = auxiliary_observation(&baseline_path, &baseline_meta)?;
        session_semantic_identity(
            &source.source_path,
            &source.owner_session_id,
            &session_canonical_state(&baseline_path, &baseline_meta)?,
            &baseline_meta,
            &baseline_auxiliary,
            4,
        )?
    } else {
        let Some(projection_sha256) = projection_sha256 else {
            return Err(format!(
                "legacy task semantic identity lacks a projection for {}",
                source.source_path
            ));
        };
        task_semantic_identity(
            &source.source_path,
            &source.owner_session_id,
            source.task_source_ordinal.unwrap_or(0),
            &baseline_state,
            projection_sha256,
            2,
        )?
    };
    if baseline_identity != *current_identity {
        return Err(format!(
            "legacy Orc semantic state changed without an identity transition: {}",
            source.source_path
        ));
    }
    Ok(())
}

fn managed_object_digest(relative: &str) -> Option<String> {
    let rest = relative.strip_prefix(&format!("{SNAPSHOT_OBJECT_ROOT}/"))?;
    let (shard, name) = rest.split_once('/')?;
    let digest = name.strip_suffix(".db")?;
    if shard.len() == 2
        && digest.len() == 64
        && shard
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && &digest[..2] == shard
    {
        Some(digest.to_string())
    } else {
        None
    }
}

/// Refuse when a recorded alias does not authenticate.
pub fn validate_semantic_alias(
    snapshot_root: &Path,
    source: &SourceCopy,
    canonical: &SemanticIdentity,
) -> Result<(), String> {
    let recorded = (
        source.semantic_sha256.clone(),
        source.semantic_complete_bytes,
    );
    let expected = (canonical.sha256.clone(), canonical.complete_bytes);
    if source.kind == "task" {
        if recorded != expected || source.semantic_alias_baseline_path.is_some() {
            return Err(format!(
                "task semantic alias differs from canonical history: {}",
                source.source_path
            ));
        }
        return Ok(());
    }
    if recorded == expected {
        if source.semantic_alias_baseline_path.is_some() {
            return Err(format!(
                "canonical Orc semantic identity has an unnecessary alias baseline: {}",
                source.source_path
            ));
        }
        return Ok(());
    }
    let relative = source
        .semantic_alias_baseline_path
        .clone()
        .unwrap_or_else(|| source.snapshot_path.clone());
    let Some(digest) = managed_object_digest(&relative) else {
        return Err(format!(
            "Orc semantic alias baseline is not a managed object: {relative}"
        ));
    };
    let path = snapshot_path(snapshot_root, &relative)?;
    let is_symlink = std::fs::symlink_metadata(&path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink || !path.is_file() || sha256_file(&path)? != digest {
        return Err(format!(
            "Orc semantic alias baseline is missing or unsafe: {}",
            path.display()
        ));
    }
    let meta = session_meta(&path, &source.source_path)?;
    let auxiliary = auxiliary_observation(&path, &meta)?;
    let baseline_canonical = session_semantic_identity(
        &source.source_path,
        &source.owner_session_id,
        &session_canonical_state(&path, &meta)?,
        &meta,
        &auxiliary,
        4,
    )?;
    if baseline_canonical != *canonical {
        return Err(format!(
            "Orc semantic alias baseline has different canonical semantics: {}",
            source.source_path
        ));
    }
    let mut states = vec![logical_state(&path, "session", None, false, None)?];
    if states[0].append_max_id.has_session_tag() {
        for mode in ["content-only", "messages-only"] {
            states.push(logical_state(&path, "session", None, false, Some(mode))?);
        }
    }
    let mut identities: Vec<(String, i64)> = Vec::new();
    for state in &states {
        let identity = session_semantic_identity(
            &source.source_path,
            &source.owner_session_id,
            state,
            &meta,
            &auxiliary,
            if state.append_max_id.has_session_tag() {
                3
            } else {
                2
            },
        )?;
        let held = (identity.sha256, identity.complete_bytes);
        if !identities.contains(&held) {
            identities.push(held);
        }
    }
    if !identities.contains(&recorded) {
        return Err(format!(
            "Orc semantic alias does not match its authenticated baseline: {}",
            source.source_path
        ));
    }
    Ok(())
}

/// Authenticate one snapshotted source against its
/// manifest record; returns its canonical identity.
pub fn validate_source_semantic_identity(
    snapshot_root: &Path,
    source: &SourceCopy,
) -> Result<SemanticIdentity, String> {
    let path = snapshot_path(snapshot_root, &source.snapshot_path)?;
    let is_symlink = std::fs::symlink_metadata(&path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false);
    if is_symlink || !path.is_file() || sha256_file(&path)? != source.sha256 {
        return Err(format!(
            "Orc snapshot does not match its manifest: {}",
            path.display()
        ));
    }
    let state = logical_state(&path, &source.kind, None, false, None)?;
    validate_manifest_logical_state(source, &state, &path.display().to_string())?;
    let mut projection_sha256: Option<String> = None;
    let identity: SemanticIdentity;
    if source.kind == "session" {
        let meta = session_meta(&path, &source.source_path)?;
        let auxiliary = auxiliary_observation(&path, &meta)?;
        if source.auxiliary.message_count != auxiliary.messages.len() as i64
            || source.auxiliary.message_sha256 != auxiliary.message_sha256
            || source.auxiliary.stable_spawn_count != auxiliary.stable_spawns.len() as i64
            || source.auxiliary.stable_spawn_sha256 != auxiliary.stable_spawn_sha256
        {
            return Err(format!(
                "Orc auxiliary evidence does not match its snapshot: {}",
                path.display()
            ));
        }
        let legacy_identity = session_semantic_identity(
            &source.source_path,
            &source.owner_session_id,
            &state,
            &meta,
            &auxiliary,
            if state.append_max_id.has_session_tag() {
                3
            } else {
                2
            },
        )?;
        let mut accepted: Vec<(String, i64)> = vec![(
            legacy_identity.sha256.clone(),
            legacy_identity.complete_bytes,
        )];
        if state.append_max_id.has_session_tag() {
            for mode in ["content-only", "messages-only"] {
                let storage_state = logical_state(&path, "session", None, false, Some(mode))?;
                let storage_identity = session_semantic_identity(
                    &source.source_path,
                    &source.owner_session_id,
                    &storage_state,
                    &meta,
                    &auxiliary,
                    2,
                )?;
                accepted.push((storage_identity.sha256, storage_identity.complete_bytes));
            }
        }
        identity = session_semantic_identity(
            &source.source_path,
            &source.owner_session_id,
            &session_canonical_state(&path, &meta)?,
            &meta,
            &auxiliary,
            4,
        )?;
        if source.canonical_semantic_sha256.is_none()
            && source.semantic_identity_mode == SEMANTIC_IDENTITY_DETERMINISTIC
            && !accepted.contains(&(
                source.semantic_sha256.clone(),
                source.semantic_complete_bytes,
            ))
            && (
                source.semantic_sha256.clone(),
                source.semantic_complete_bytes,
            ) != (identity.sha256.clone(), identity.complete_bytes)
        {
            return Err(format!(
                "Orc deterministic semantic identity does not match artifacts: {}",
                source.source_path
            ));
        }
    } else {
        let Some(projection) = &source.task_projection else {
            return Err(format!(
                "task source lacks frozen enrichment: {}",
                source.source_path
            ));
        };
        if projection.policy == TASK_HISTORY_POLICY {
            let frozen = load_frozen_task_projection(snapshot_root, projection)?;
            let current_records = observed_task_notes(&path)?;
            let current_sequence = task_note_sequence(&path)?;
            if let Some(recorded_sequence) = projection.observed_note_sequence {
                if current_sequence != recorded_sequence {
                    return Err(format!(
                        "task note allocation sequence does not match its snapshot: {}",
                        path.display()
                    ));
                }
            }
            let unexpected: Vec<i64> = current_records
                .iter()
                .filter(|record| !frozen.iter().any(|held| held.note_id == record.note_id))
                .map(|record| record.note_id)
                .collect();
            if let Some(first) = unexpected.iter().min() {
                return Err(format!(
                    "task snapshot contains an unfrozen note for {}: {first}",
                    source.source_path
                ));
            }
            for observed in &current_records {
                let prior = frozen
                    .iter()
                    .find(|held| held.note_id == observed.note_id)
                    .expect("checked present");
                if (
                    observed.task_id.clone(),
                    observed.content.clone(),
                    observed.created_at.clone(),
                ) != (
                    prior.task_id.clone(),
                    prior.content.clone(),
                    prior.created_at.clone(),
                ) {
                    return Err(format!(
                        "task note immutable core was rewritten for note {} in {}",
                        observed.note_id, source.source_path
                    ));
                }
            }
            let enrichment_sha256 = task_enrichment_sha256(&frozen);
            if enrichment_sha256 != projection.observed_enrichment_sha256 {
                return Err(format!(
                    "frozen task enrichment does not match its manifest: {}",
                    path.display()
                ));
            }
            let (rewrite_sha256, missing_count, missing_sha256) =
                task_rewrite_observation(&frozen, &current_records);
            let (gap_count, gap_sha256) = task_unobserved_id_gaps(&frozen, current_sequence);
            if rewrite_sha256 != projection.observed_note_rewrite_sha256
                || missing_count != projection.missing_note_count
                || missing_sha256 != projection.missing_note_ids_sha256
                || (projection.unobserved_note_id_gap_count.is_some()
                    && (gap_count != projection.unobserved_note_id_gap_count.unwrap_or(0)
                        || gap_sha256
                            != projection
                                .unobserved_note_id_gap_sha256
                                .clone()
                                .unwrap_or_default()))
            {
                return Err(format!(
                    "task rewrite observation does not match its snapshot: {}",
                    path.display()
                ));
            }
            projection_sha256 = Some(enrichment_sha256.clone());
            identity = task_semantic_identity(
                &source.source_path,
                &source.owner_session_id,
                source.task_source_ordinal.unwrap_or(0),
                &frozen_task_logical_state(&frozen),
                &enrichment_sha256,
                2,
            )?;
        } else {
            load_legacy_task_projection(snapshot_root, projection)?;
            let observation = task_enrichment_observation(&path)?;
            if observation.sha256 != projection.observed_enrichment_sha256 {
                return Err(format!(
                    "task enrichment observation does not match its snapshot: {}",
                    path.display()
                ));
            }
            projection_sha256 = Some(projection.sha256.clone());
            identity = task_semantic_identity(
                &source.source_path,
                &source.owner_session_id,
                source.task_source_ordinal.unwrap_or(0),
                &state,
                &projection.sha256,
                2,
            )?;
        }
    }
    if let Some(canonical_sha) = &source.canonical_semantic_sha256 {
        if *canonical_sha != identity.sha256
            || source.canonical_semantic_complete_bytes != Some(identity.complete_bytes)
        {
            return Err(format!(
                "Orc canonical semantic identity does not match artifacts: {}",
                source.source_path
            ));
        }
    }
    if source.canonical_semantic_sha256.is_some()
        && source.semantic_identity_mode == SEMANTIC_IDENTITY_DETERMINISTIC
    {
        validate_semantic_alias(snapshot_root, source, &identity)?;
    }
    if source.kind != "session" && source.canonical_semantic_sha256.is_none() {
        validate_recorded_semantic_identity(source, &identity, &source.source_path)?;
    }
    validate_legacy_semantic_identity(
        snapshot_root,
        source,
        &identity,
        projection_sha256.as_deref(),
    )?;
    Ok(identity)
}

// ---------------------------------------------------------------- snapshot planning

/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_ROWS: usize = 20;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_KEYS: usize = 20;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_MESSAGE_CHARS: usize = 2000;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_CONTEXT_CHARS: usize = 40;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_EXCERPT_CHARS: usize = 160;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_JSON_CHARS: usize = 65_536;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_JSON_PATHS: usize = 12;
/// See the enclosing record's documentation.
pub const PREFIX_DIFF_MAX_JSON_DEPTH: usize = 16;

/// Discovery output: what to snapshot, against what prior.
pub struct DiscoveryPlan {
    /// See the enclosing record's documentation.
    pub discovered: Vec<DiscoveredSource>,
    /// See the enclosing record's documentation.
    pub previous_by_path: Vec<(String, SourceCopy)>,
    /// See the enclosing record's documentation.
    pub selected_session_ids: Vec<String>,
    /// See the enclosing record's documentation.
    pub task_ordinals: Vec<(String, i64)>,
    /// See the enclosing record's documentation.
    pub session_task_paths: Vec<(String, Vec<String>)>,
}

impl DiscoveryPlan {
    /// See the enclosing record's documentation.
    pub fn previous(&self, path: &str) -> Option<&SourceCopy> {
        self.previous_by_path
            .iter()
            .find(|(held, _)| held == path)
            .map(|(_, held)| held)
    }

    /// See the enclosing record's documentation.
    pub fn ordinal(&self, path: &str) -> Option<i64> {
        self.task_ordinals
            .iter()
            .find(|(held, _)| held == path)
            .map(|(_, held)| *held)
    }

    /// See the enclosing record's documentation.
    pub fn task_paths(&self, session_path: &str) -> Option<&Vec<String>> {
        self.session_task_paths
            .iter()
            .find(|(held, _)| held == session_path)
            .map(|(_, held)| held)
    }
}

fn path_set(paths: &[String]) -> Vec<String> {
    let mut held = paths.to_vec();
    held.sort();
    held.dedup();
    held
}

/// Plan one snapshot run against the previous manifest.
pub fn plan_snapshot_sources(
    source_root: &Path,
    root_session_id: &str,
    snapshot_root: &Path,
    previous_sources: &[SourceCopy],
    continuation_specs_in: &[ContinuationSpec],
) -> Result<DiscoveryPlan, String> {
    lineage_roots(
        root_session_id,
        &continuation_specs_in
            .iter()
            .map(|spec| spec.session_id.clone())
            .collect::<Vec<_>>(),
    )?;
    let mut live_discovered = if continuation_specs_in.is_empty() {
        discover_sources(source_root, root_session_id)?
    } else {
        discover_continuation_sources(
            source_root,
            root_session_id,
            continuation_specs_in,
            &previous_sources
                .iter()
                .filter(|source| source.kind == "session")
                .map(|source| source.owner_session_id.clone())
                .collect::<Vec<_>>(),
        )?
    };
    {
        let mut paths: Vec<&str> = live_discovered
            .iter()
            .map(|source| source.source_path.as_str())
            .collect();
        paths.sort();
        paths.dedup();
        if paths.len() != live_discovered.len() {
            return Err("Orc source discovery produced duplicate source records".to_string());
        }
    }
    let mut previous_by_path: Vec<(String, SourceCopy)> = Vec::new();
    for source in previous_sources {
        if previous_by_path
            .iter()
            .any(|(held, _)| *held == source.source_path)
        {
            return Err(format!(
                "duplicate previous Orc source record: {}",
                source.source_path
            ));
        }
        previous_by_path.push((source.source_path.clone(), source.clone()));
    }
    let previous_of = |path: &str| -> Option<SourceCopy> {
        previous_by_path
            .iter()
            .find(|(held, _)| held == path)
            .map(|(_, held)| held.clone())
    };
    let selected_session_ids: Vec<String> = path_set(
        &live_discovered
            .iter()
            .filter(|source| source.kind == "session")
            .map(|source| source.owner_candidates[0].clone())
            .collect::<Vec<_>>(),
    );
    let selected_task_paths: Vec<String> = path_set(
        &live_discovered
            .iter()
            .filter(|source| source.kind == "task")
            .map(|source| source.source_path.clone())
            .collect::<Vec<_>>(),
    );
    let restricted_task_sources = continuation_specs_in
        .iter()
        .any(|spec| spec.start_message_id.is_some());
    let mut newly_detached_paths: Vec<String> = Vec::new();
    let mut session_task_paths: Vec<(String, Vec<String>)> = Vec::new();
    let live_sessions: Vec<DiscoveredSource> = live_discovered
        .iter()
        .filter(|source| source.kind == "session")
        .cloned()
        .collect();
    for discovered_source in &live_sessions {
        let live_path = live_source_path(source_root, &discovered_source.source_path)?;
        let live_meta = session_meta(&live_path, &discovered_source.source_path)?;
        let mut live_task_paths: Vec<String> = session_task_relatives(&live_path, &live_meta)?
            .into_iter()
            .map(|(path, _, _)| path)
            .filter(|path| !restricted_task_sources || selected_task_paths.contains(path))
            .collect();
        live_task_paths.sort();
        live_task_paths.dedup();
        session_task_paths.push((
            discovered_source.source_path.clone(),
            live_task_paths.clone(),
        ));
        let Some(previous) = previous_of(&discovered_source.source_path) else {
            continue;
        };
        let previous_path = snapshot_path(snapshot_root, &previous.snapshot_path)?;
        let is_symlink = std::fs::symlink_metadata(&previous_path)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !previous_path.is_file() || sha256_file(&previous_path)? != previous.sha256
        {
            return Err(format!(
                "previous Orc snapshot does not match its manifest: {}",
                previous_path.display()
            ));
        }
        let prior_meta = session_meta(&previous_path, &discovered_source.source_path)?;
        let prior_task_paths: Vec<String> = session_task_relatives(&previous_path, &prior_meta)?
            .into_iter()
            .map(|(path, _, _)| path)
            .filter(|path| !restricted_task_sources || selected_task_paths.contains(path))
            .collect();
        for path in &prior_task_paths {
            if !live_task_paths.contains(path) && !newly_detached_paths.contains(path) {
                newly_detached_paths.push(path.clone());
            }
        }
    }
    let planned_live_task_paths: Vec<String> = {
        let mut held: Vec<String> = session_task_paths
            .iter()
            .flat_map(|(_, paths)| paths.iter().cloned())
            .filter(|path| {
                live_source_path(source_root, path).is_ok_and(|held| source_exists(&held))
            })
            .collect();
        held.sort();
        held.dedup();
        held
    };
    let discovered_live_task_paths: Vec<String> = {
        let mut held: Vec<String> = live_discovered
            .iter()
            .filter(|item| item.kind == "task")
            .map(|item| item.source_path.clone())
            .collect();
        held.sort();
        held.dedup();
        held
    };
    if path_set(&planned_live_task_paths) != path_set(&discovered_live_task_paths) {
        return Err(
            "Orc task database references changed during source discovery; retry ingest"
                .to_string(),
        );
    }
    let discovered_paths = path_set(
        &live_discovered
            .iter()
            .map(|source| source.source_path.clone())
            .collect::<Vec<_>>(),
    );
    let mut frozen_paths: Vec<String> = previous_sources
        .iter()
        .filter(|source| {
            source.kind == "task"
                && !discovered_paths.contains(&source.source_path)
                && (source.source_state == SOURCE_STATE_DETACHED
                    || newly_detached_paths.contains(&source.source_path)
                    || live_source_path(source_root, &source.source_path)
                        .is_ok_and(|held| !source_exists(&held)))
        })
        .map(|source| source.source_path.clone())
        .collect();
    frozen_paths.sort();
    frozen_paths.dedup();
    let mut disappeared: Vec<String> = previous_by_path
        .iter()
        .map(|(path, _)| path.clone())
        .filter(|path| !discovered_paths.contains(path) && !frozen_paths.contains(path))
        .collect();
    disappeared.sort();
    if !disappeared.is_empty() {
        return Err(format!(
            "previously observed Orc source disappeared: {}",
            disappeared.join(", ")
        ));
    }
    for path in &frozen_paths {
        let previous = previous_of(path).expect("frozen previous");
        live_discovered.push(DiscoveredSource {
            source_path: path.clone(),
            kind: "task".to_string(),
            owner_candidates: vec![previous.owner_session_id.clone()],
            lineage_root_session_id: None,
            source_state: SOURCE_STATE_DETACHED.to_string(),
        });
    }
    live_discovered.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    let mut used_task_ordinals: Vec<(String, Vec<i64>)> = Vec::new();
    for prior_source in previous_sources {
        if prior_source.kind == "task" {
            if let Some(ordinal) = prior_source.task_source_ordinal {
                match used_task_ordinals
                    .iter_mut()
                    .find(|(held, _)| *held == prior_source.owner_session_id)
                {
                    Some((_, held)) => {
                        if !held.contains(&ordinal) {
                            held.push(ordinal);
                        }
                    }
                    None => used_task_ordinals
                        .push((prior_source.owner_session_id.clone(), vec![ordinal])),
                }
            }
        }
    }
    let mut task_ordinals: Vec<(String, i64)> = Vec::new();
    for discovered_item in &live_discovered {
        if discovered_item.kind != "task" {
            continue;
        }
        let previous = previous_of(&discovered_item.source_path);
        let owner = previous
            .as_ref()
            .map(|held| held.owner_session_id.clone())
            .unwrap_or_else(|| discovered_item.owner_candidates[0].clone());
        let ordinal = match previous.as_ref().and_then(|held| held.task_source_ordinal) {
            Some(ordinal) => ordinal,
            None => {
                let used: Vec<i64> = used_task_ordinals
                    .iter()
                    .find(|(held, _)| *held == owner)
                    .map(|(_, held)| held.clone())
                    .unwrap_or_default();
                let mut ordinal = 0i64;
                while used.contains(&ordinal) {
                    ordinal += 1;
                }
                ordinal
            }
        };
        task_ordinals.push((discovered_item.source_path.clone(), ordinal));
        match used_task_ordinals
            .iter_mut()
            .find(|(held, _)| *held == owner)
        {
            Some((_, held)) => {
                if !held.contains(&ordinal) {
                    held.push(ordinal);
                }
            }
            None => used_task_ordinals.push((owner, vec![ordinal])),
        }
    }
    Ok(DiscoveryPlan {
        discovered: live_discovered,
        previous_by_path,
        selected_session_ids,
        task_ordinals,
        session_task_paths,
    })
}

// ---------------------------------------------------------------- prefix diff

fn prefix_value(value: &SqlValue) -> (String, String) {
    match value {
        SqlValue::Null => ("null".to_string(), String::new()),
        SqlValue::Int(held) => ("int".to_string(), held.to_string()),
        SqlValue::Real(held) => ("real".to_string(), py_float_hex(*held)),
        SqlValue::Text(held) => ("text".to_string(), held.clone()),
        SqlValue::Blob(held) => (
            "blob".to_string(),
            held.iter().map(|byte| format!("{byte:02x}")).collect(),
        ),
    }
}

fn prefix_values_equal(previous: &SqlValue, observed: &SqlValue) -> bool {
    // Type-strict, like the digest tags: 1 != 1.0.
    std::mem::discriminant(previous) == std::mem::discriminant(observed) && previous == observed
}

fn prefix_rows_equal(previous: &[SqlValue], observed: &[SqlValue]) -> bool {
    previous.len() == observed.len()
        && previous
            .iter()
            .zip(observed.iter())
            .all(|(a, b)| prefix_values_equal(a, b))
}

fn prefix_window(body: &str, start: i64, end: i64) -> (String, bool) {
    let char_count = body.chars().count() as i64;
    let start = start.clamp(0, char_count);
    let end = end.clamp(start, char_count);
    let end = if end - start > PREFIX_DIFF_EXCERPT_CHARS as i64 {
        start + PREFIX_DIFF_EXCERPT_CHARS as i64
    } else {
        end
    };
    let text: String = body
        .chars()
        .skip(start as usize)
        .take((end - start) as usize)
        .collect();
    let mut out = text;
    if start > 0 {
        out = format!("...{out}");
    }
    if end < char_count {
        out = format!("{out}...");
    }
    (out, start > 0 || end < char_count)
}

fn prefix_excerpts(previous_body: &str, observed_body: &str) -> (String, String, bool) {
    let previous_chars: Vec<char> = previous_body.chars().collect();
    let observed_chars: Vec<char> = observed_body.chars().collect();
    let shared = previous_chars.len().min(observed_chars.len());
    let mut head = 0usize;
    while head < shared && previous_chars[head] == observed_chars[head] {
        head += 1;
    }
    let mut tail = 0usize;
    while tail < shared - head
        && previous_chars[previous_chars.len() - 1 - tail]
            == observed_chars[observed_chars.len() - 1 - tail]
    {
        tail += 1;
    }
    let start = head as i64 - PREFIX_DIFF_CONTEXT_CHARS as i64;
    let (previous_excerpt, previous_bounded) = prefix_window(
        previous_body,
        start,
        previous_chars.len() as i64 - tail as i64 + PREFIX_DIFF_CONTEXT_CHARS as i64,
    );
    let (observed_excerpt, observed_bounded) = prefix_window(
        observed_body,
        start,
        observed_chars.len() as i64 - tail as i64 + PREFIX_DIFF_CONTEXT_CHARS as i64,
    );
    (
        previous_excerpt,
        observed_excerpt,
        previous_bounded || observed_bounded,
    )
}

fn collect_json_changes(
    previous: &JsonValue,
    observed: &JsonValue,
    path: &str,
    depth: usize,
    found: &mut Vec<String>,
) -> bool {
    if found.len() >= PREFIX_DIFF_MAX_JSON_PATHS {
        return true;
    }
    if prefix_values_equal_json(previous, observed) {
        return false;
    }
    if depth >= PREFIX_DIFF_MAX_JSON_DEPTH {
        found.push(if path.is_empty() { "." } else { path }.to_string());
        return true;
    }
    if let (Some(previous_object), Some(observed_object)) =
        (previous.as_object(), observed.as_object())
    {
        let mut keys: Vec<&String> = previous_object
            .keys()
            .chain(observed_object.keys())
            .collect();
        keys.sort();
        keys.dedup();
        let mut bounded = false;
        for key in keys {
            let child = if path.is_empty() {
                key.clone()
            } else {
                format!("{path}.{key}")
            };
            match (previous_object.get(key), observed_object.get(key)) {
                (Some(left), Some(right)) => {
                    bounded =
                        collect_json_changes(left, right, &child, depth + 1, found) || bounded;
                }
                _ => {
                    if found.len() >= PREFIX_DIFF_MAX_JSON_PATHS {
                        return true;
                    }
                    found.push(child);
                }
            }
        }
        return bounded;
    }
    if let (Some(previous_array), Some(observed_array)) = (previous.as_array(), observed.as_array())
    {
        if previous_array.len() == observed_array.len() {
            let mut bounded = false;
            for (index, (left, right)) in
                previous_array.iter().zip(observed_array.iter()).enumerate()
            {
                bounded = collect_json_changes(
                    left,
                    right,
                    &format!("{path}[{index}]"),
                    depth + 1,
                    found,
                ) || bounded;
            }
            return bounded;
        }
    }
    found.push(if path.is_empty() { "." } else { path }.to_string());
    false
}

fn prefix_values_equal_json(previous: &JsonValue, observed: &JsonValue) -> bool {
    // The diff compares the way the digest does: int/float
    // and bool/int are different kinds.
    match (previous, observed) {
        (JsonValue::Number(a), JsonValue::Number(b)) => {
            (a.is_i64() || a.is_u64()) == (b.is_i64() || b.is_u64())
                && crate::json::dump_compact(&JsonValue::Number(a.clone()))
                    == crate::json::dump_compact(&JsonValue::Number(b.clone()))
        }
        _ => previous == observed,
    }
}

fn json_change_paths(previous_body: &str, observed_body: &str) -> (Vec<String>, bool) {
    if previous_body.len() > PREFIX_DIFF_MAX_JSON_CHARS
        || observed_body.len() > PREFIX_DIFF_MAX_JSON_CHARS
    {
        return (Vec::new(), false);
    }
    let (Ok(previous_value), Ok(observed_value)) = (
        serde_json::from_str::<JsonValue>(previous_body),
        serde_json::from_str::<JsonValue>(observed_body),
    ) else {
        return (Vec::new(), false);
    };
    if !previous_value.is_object() && !previous_value.is_array() {
        return (Vec::new(), false);
    }
    if !observed_value.is_object() && !observed_value.is_array() {
        return (Vec::new(), false);
    }
    let mut found = Vec::new();
    let bounded = collect_json_changes(&previous_value, &observed_value, "", 0, &mut found);
    (found, bounded)
}

fn prefix_display(kind: &str, excerpt: &str) -> String {
    if kind == "null" {
        "null".to_string()
    } else {
        format!("{kind}:{excerpt}")
    }
}

fn prefix_column_change(
    column: &str,
    previous_value: &SqlValue,
    observed_value: &SqlValue,
) -> PrefixColumnChange {
    let (previous_kind, previous_body) = prefix_value(previous_value);
    let (observed_kind, observed_body) = prefix_value(observed_value);
    let mut json_paths = Vec::new();
    let mut json_paths_bounded = false;
    let (previous_excerpt, observed_excerpt, bounded);
    if previous_kind == observed_kind {
        let (a, b, held) = prefix_excerpts(&previous_body, &observed_body);
        previous_excerpt = a;
        observed_excerpt = b;
        bounded = held;
        if previous_kind == "text" {
            (json_paths, json_paths_bounded) = json_change_paths(&previous_body, &observed_body);
        }
    } else {
        let (pe, pb) = prefix_window(&previous_body, 0, previous_body.chars().count() as i64);
        let (oe, ob) = prefix_window(&observed_body, 0, observed_body.chars().count() as i64);
        previous_excerpt = pe;
        observed_excerpt = oe;
        bounded = pb || ob;
    }
    PrefixColumnChange {
        column: column.to_string(),
        previous: prefix_display(&previous_kind, &previous_excerpt),
        observed: prefix_display(&observed_kind, &observed_excerpt),
        bounded,
        json_paths,
        json_paths_bounded,
    }
}

/// Bounded evidence about how a live prefix differs.
pub struct PrefixDiff {
    /// See the enclosing record's documentation.
    pub changed_rows: Vec<PrefixRowChange>,
    /// See the enclosing record's documentation.
    pub changed_row_count: i64,
    /// See the enclosing record's documentation.
    pub missing_rows: Vec<String>,
    /// See the enclosing record's documentation.
    pub missing_row_count: i64,
    /// See the enclosing record's documentation.
    pub added_rows: Vec<String>,
    /// See the enclosing record's documentation.
    pub added_row_count: i64,
}

impl PrefixDiff {
    /// See the enclosing record's documentation.
    pub fn changed_rows_bounded(&self) -> bool {
        (self.changed_rows.len() as i64) < self.changed_row_count
    }
}

fn prefix_rows_stream(
    connection: &Connection,
    scope: &PrefixScope,
) -> Result<Vec<Vec<SqlValue>>, String> {
    query_all(
        connection,
        &scope.query(),
        &[SqlValue::Int(scope.limit as i64)],
        "prefix diff",
    )
}

/// Walk both prefixes in key order and diff them,
/// boundedly.
pub fn append_prefix_diff(
    previous_path: &Path,
    current_path: &Path,
    scopes: &[PrefixScope],
) -> Result<PrefixDiff, String> {
    let previous_connection = read_only(previous_path)?;
    let current_connection = read_only(current_path).map_err(|error| {
        format!(
            "failed to compare Orc append prefixes for {}: {error}",
            current_path.display()
        )
    })?;
    let mut changed: Vec<PrefixRowChange> = Vec::new();
    let mut changed_count = 0i64;
    let mut missing: Vec<String> = Vec::new();
    let mut missing_count = 0i64;
    let mut added: Vec<String> = Vec::new();
    let mut added_count = 0i64;
    for scope in scopes {
        let previous_rows = prefix_rows_stream(&previous_connection, scope).map_err(|error| {
            format!(
                "failed to compare Orc append prefixes for {}: {error}",
                current_path.display()
            )
        })?;
        let current_rows = prefix_rows_stream(&current_connection, scope).map_err(|error| {
            format!(
                "failed to compare Orc append prefixes for {}: {error}",
                current_path.display()
            )
        })?;
        let mut previous_iter = previous_rows.iter().peekable();
        let mut current_iter = current_rows.iter().peekable();
        let key_of =
            |row: &[SqlValue]| -> Result<i64, String> { cell_integer(&row[0], "prefix diff: key") };
        loop {
            let previous_row = previous_iter.peek();
            let current_row = current_iter.peek();
            if previous_row.is_none() && current_row.is_none() {
                break;
            }
            let previous_key = previous_row.map(|row| key_of(row)).transpose()?;
            let current_key = current_row.map(|row| key_of(row)).transpose()?;
            if current_key.is_none() || (previous_key.is_some() && previous_key < current_key) {
                missing_count += 1;
                if missing.len() < PREFIX_DIFF_MAX_KEYS {
                    missing.push(format!("{} row {}", scope.table, previous_key.unwrap_or(0)));
                }
                previous_iter.next();
                continue;
            }
            if previous_key.is_none() || current_key < previous_key {
                added_count += 1;
                if added.len() < PREFIX_DIFF_MAX_KEYS {
                    added.push(format!("{} row {}", scope.table, current_key.unwrap_or(0)));
                }
                current_iter.next();
                continue;
            }
            let (previous_row, current_row) = (
                previous_iter.next().expect("row present"),
                current_iter.next().expect("row present"),
            );
            if !prefix_rows_equal(previous_row, current_row) {
                changed_count += 1;
                if changed.len() < PREFIX_DIFF_MAX_ROWS {
                    let mut columns = Vec::new();
                    for (index, name) in scope.columns.iter().enumerate() {
                        if !prefix_values_equal(&previous_row[index], &current_row[index]) {
                            columns.push(prefix_column_change(
                                name,
                                &previous_row[index],
                                &current_row[index],
                            ));
                        }
                    }
                    changed.push(PrefixRowChange {
                        table: scope.table.clone(),
                        row_id: previous_key.unwrap_or(0),
                        columns,
                    });
                }
            }
        }
    }
    Ok(PrefixDiff {
        changed_rows: changed,
        changed_row_count: changed_count,
        missing_rows: missing,
        missing_row_count: missing_count,
        added_rows: added,
        added_row_count: added_count,
    })
}

/// The scopes the guard digested for a path at a
/// watermark.
pub fn prefix_scopes_for(
    path: &Path,
    session_state_mode: Option<&str>,
    prefix_max_id: BigUint,
) -> Result<Vec<PrefixScope>, String> {
    let connection = read_only(path)?;
    let geometry = session_geometry(&connection, path, session_state_mode).map_err(|error| {
        format!(
            "failed to inspect Orc SQLite source {}: {error}",
            path.display()
        )
    })?;
    drop(connection);
    let (scopes, _) = session_prefix_scopes(&geometry, session_state_mode, Some(prefix_max_id));
    Ok(scopes)
}

/// Refuse a prefix rewrite, or record its acceptance.
#[allow(clippy::too_many_arguments)]
pub fn resolve_prefix_rewrite(
    relative: &str,
    kind: &str,
    session_id: &str,
    previous: &SourceCopy,
    previous_state: &LogicalState,
    observed: &LogicalState,
    diff: &PrefixDiff,
    accept_prefix_rewrite: &[String],
    captured_at: &str,
) -> Result<AppendPrefixOverride, String> {
    let prefix_error = format!("Orc {kind} existing append prefix was rewritten for {relative}");
    if diff.missing_row_count > 0 || diff.added_row_count > 0 {
        return Err(format!(
            "{prefix_error}: the recorded prefix lost {} row(s) ({}) and gained {} row(s) ({}) below its watermark; accepting a prefix rewrite covers changed column values only, never rows appearing or disappearing",
            diff.missing_row_count,
            if diff.missing_rows.is_empty() { "none".to_string() } else { diff.missing_rows.join(", ") },
            diff.added_row_count,
            if diff.added_rows.is_empty() { "none".to_string() } else { diff.added_rows.join(", ") }
        ));
    }
    if diff.changed_row_count == 0 {
        return Err(format!(
            "{prefix_error}: the digest moved but no row differs, so the archive cannot describe what changed"
        ));
    }
    if !accept_prefix_rewrite.iter().any(|held| held == session_id) {
        let mut described: Vec<String> = Vec::new();
        let mut budget = PREFIX_DIFF_MAX_MESSAGE_CHARS as i64;
        for row in &diff.changed_rows {
            let line = row.describe();
            if budget - (line.chars().count() as i64) < 0 && !described.is_empty() {
                break;
            }
            budget -= line.chars().count() as i64;
            described.push(line);
        }
        let remaining = diff.changed_row_count - described.len() as i64;
        let mut evidence = described.join("; ");
        if remaining > 0 {
            evidence += &format!("; ... {remaining} further changed row(s) not shown");
        }
        return Err(format!(
            "{prefix_error}: {} row(s) changed at or below the append watermark [{evidence}]; re-run with --accept-orc-prefix-rewrite {session_id} to record this rewrite and re-baseline the digest -- that authorizes this one session and no other, so another rewritten session in this lineage refuses again with its own evidence -- accepting supersedes the pre-rewrite snapshot {} (kept for comparison until the next accepted override on this source, then reclaimed) and records only a bounded summary -- at most {PREFIX_DIFF_MAX_ROWS} of those {} changed row(s), at most {PREFIX_DIFF_EXCERPT_CHARS} characters per column value",
            diff.changed_row_count,
            previous.snapshot_path,
            diff.changed_row_count
        ));
    }
    Ok(AppendPrefixOverride {
        policy: PREFIX_OVERRIDE_POLICY.to_string(),
        source_path: relative.to_string(),
        accepted_at: captured_at.to_string(),
        override_count: previous
            .append_prefix_override
            .as_ref()
            .map_or(1, |prior| prior.override_count + 1),
        previous_append_prefix_sha256: previous_state.append_prefix_sha256.clone(),
        observed_append_prefix_sha256: observed.append_prefix_sha256.clone(),
        superseded_snapshot_path: previous.snapshot_path.clone(),
        superseded_sha256: previous.sha256.clone(),
        changed_row_count: diff.changed_row_count,
        changed_rows: diff.changed_rows.clone(),
        changed_rows_bounded: diff.changed_rows_bounded(),
        degraded: true,
        degradation_reason: PREFIX_OVERRIDE_DEGRADATION_REASON.to_string(),
    })
}

// ---------------------------------------------------------------- prepare & publish

/// One backed-up candidate ready to publish.
pub struct PreparedCandidate {
    /// See the enclosing record's documentation.
    pub source: SourceCopy,
    /// See the enclosing record's documentation.
    pub temporary: Option<PathBuf>,
    /// See the enclosing record's documentation.
    pub target_relative: String,
    /// See the enclosing record's documentation.
    pub canonical_identity: SemanticIdentity,
}

/// Create a SQLite snapshot of one source.
pub fn backup_database(
    source: &Path,
    destination: &Path,
    snapshot_root: &Path,
) -> Result<(), String> {
    ensure_snapshot_parent(destination, snapshot_root)?;
    if destination.exists() {
        let is_symlink = std::fs::symlink_metadata(destination)
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_symlink || !destination.is_file() {
            return Err(format!(
                "snapshot target is a symlink or non-file: {}",
                destination.display()
            ));
        }
    }
    let source_connection = read_only(source)?;
    let outcome = (|| -> Result<(), String> {
        source_connection
            .backup(rusqlite::MAIN_DB, destination, None)
            .map_err(|error| {
                format!(
                    "failed to back up Orc SQLite source {}: {error}",
                    source.display()
                )
            })?;
        let destination_connection =
            Connection::open(destination).map_err(|error| error.to_string())?;
        let mode: String = destination_connection
            .query_row("PRAGMA journal_mode = DELETE", [], |row| row.get(0))
            .map_err(|error| error.to_string())?;
        if mode.to_lowercase() != "delete" {
            return Err(format!(
                "failed to normalize Orc snapshot journal mode for {}",
                source.display()
            ));
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        return Err(format!(
            "failed to back up Orc SQLite source {}: {error}",
            source.display()
        ));
    }
    Ok(())
}

/// Atomically publish one immutable object, or reuse an
/// identical orphan. Returns whether a new object was
/// created.
pub fn publish_snapshot_candidate(
    temporary: &Path,
    target: &Path,
    expected_sha256: &str,
    snapshot_root: &Path,
) -> Result<bool, String> {
    ensure_snapshot_parent(target, snapshot_root)?;
    fsync_regular_file(temporary)?;
    match std::fs::symlink_metadata(target) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(format!(
                    "snapshot object target is a symlink or non-file: {}",
                    target.display()
                ));
            }
            if sha256_file(target)? != expected_sha256 {
                return Err(format!(
                    "preexisting snapshot object has the wrong hash: {}",
                    target.display()
                ));
            }
            fsync_regular_file(target)?;
            fsync_directory(target.parent().unwrap_or(target))?;
            std::fs::remove_file(temporary)
                .map_err(|error| crate::json::os_error(&error, temporary))?;
            return Ok(false);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(crate::json::os_error(&error, target)),
    }
    match std::fs::hard_link(temporary, target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let is_symlink = std::fs::symlink_metadata(target)
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink || !target.is_file() {
                return Err(format!(
                    "snapshot object target is a symlink or non-file: {}",
                    target.display()
                ));
            }
            if sha256_file(target)? != expected_sha256 {
                return Err(format!(
                    "raced snapshot object has the wrong hash: {}",
                    target.display()
                ));
            }
            fsync_regular_file(target)?;
            fsync_directory(target.parent().unwrap_or(target))?;
            std::fs::remove_file(temporary)
                .map_err(|error| crate::json::os_error(&error, temporary))?;
            return Ok(false);
        }
        Err(error) => return Err(crate::json::os_error(&error, target)),
    }
    if sha256_file(target)? != expected_sha256 {
        return Err(format!(
            "published snapshot object failed hash verification: {}",
            target.display()
        ));
    }
    fsync_directory(target.parent().unwrap_or(target))?;
    std::fs::remove_file(temporary).map_err(|error| crate::json::os_error(&error, temporary))?;
    Ok(true)
}

fn managed_name_parts(name: &str, extension: &str) -> Option<String> {
    let digest = name.strip_suffix(extension)?;
    if digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Some(digest.to_string())
    } else {
        None
    }
}

fn prune_managed_objects(
    snapshot_root: &Path,
    root_name: &str,
    extension: &str,
    retained: &[String],
) -> Result<i64, String> {
    let object_root = snapshot_root.join(root_name);
    let metadata = match std::fs::symlink_metadata(&object_root) {
        Ok(held) => held,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(crate::json::os_error(&error, &object_root)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "Orc snapshot object root is unsafe: {}",
            object_root.display()
        ));
    }
    let mut removed = 0i64;
    let mut prefixes: Vec<PathBuf> = Vec::new();
    let entries = std::fs::read_dir(&object_root)
        .map_err(|error| crate::json::os_error(&error, &object_root))?;
    for entry in entries.flatten() {
        prefixes.push(entry.path());
    }
    prefixes.sort_by_key(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()));
    for prefix in &prefixes {
        let prefix_meta = std::fs::symlink_metadata(prefix)
            .map_err(|error| crate::json::os_error(&error, prefix))?;
        if prefix_meta.file_type().is_symlink() {
            return Err(format!(
                "symlink in Orc snapshot object store: {}",
                prefix.display()
            ));
        }
        let prefix_name = prefix
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let prefix_ok = prefix_name.len() == 2
            && prefix_name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !prefix_meta.is_dir() || !prefix_ok {
            continue;
        }
        let mut prefix_changed = false;
        let mut candidates: Vec<PathBuf> = Vec::new();
        let entries =
            std::fs::read_dir(prefix).map_err(|error| crate::json::os_error(&error, prefix))?;
        for entry in entries.flatten() {
            candidates.push(entry.path());
        }
        candidates.sort_by_key(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()));
        for candidate in &candidates {
            let candidate_meta = std::fs::symlink_metadata(candidate)
                .map_err(|error| crate::json::os_error(&error, candidate))?;
            if candidate_meta.file_type().is_symlink() {
                return Err(format!(
                    "symlink in Orc snapshot object store: {}",
                    candidate.display()
                ));
            }
            let name = candidate
                .file_name()
                .map(|held| held.to_string_lossy().into_owned())
                .unwrap_or_default();
            if extension == ".db" {
                for suffix in ["-wal", "-shm"] {
                    if let Some(base) = name.strip_suffix(suffix) {
                        if base.len() == 67
                            && base.ends_with(".db")
                            && managed_name_parts(&base[..67], ".db")
                                .is_some_and(|digest| digest[..2] == *prefix_name)
                            && candidate_meta.is_file()
                        {
                            std::fs::remove_file(candidate)
                                .map_err(|error| crate::json::os_error(&error, candidate))?;
                            removed += 1;
                            prefix_changed = true;
                        }
                    }
                }
            }
            let Some(digest) = managed_name_parts(&name, extension) else {
                continue;
            };
            if !candidate_meta.is_file() || digest[..2] != *prefix_name {
                continue;
            }
            let relative = candidate
                .strip_prefix(snapshot_root)
                .map(|held| held.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if retained.contains(&relative) {
                continue;
            }
            std::fs::remove_file(candidate)
                .map_err(|error| crate::json::os_error(&error, candidate))?;
            removed += 1;
            prefix_changed = true;
        }
        if prefix_changed {
            fsync_directory(prefix)?;
        }
    }
    Ok(removed)
}

/// Remove unreferenced managed objects after a durable
/// manifest commit; returns the count removed.
pub fn prune_orc_snapshot_objects(
    snapshot_root: &Path,
    retained_sources: &[SourceCopy],
) -> Result<i64, String> {
    let mut retained_databases: Vec<String> = Vec::new();
    let mut retained_projections: Vec<String> = Vec::new();
    for source in retained_sources {
        let expected = snapshot_object_relative(&source.sha256)?;
        if source.snapshot_path != expected {
            return Err(format!(
                "refusing Orc object GC for unmanaged snapshot path {}",
                py_repr_str(&source.snapshot_path)
            ));
        }
        retained_databases.push(source.snapshot_path.clone());
        if let Some(held) = &source.append_prefix_override {
            retained_databases.push(held.superseded_snapshot_path.clone());
        }
        let mut retained_items: Vec<(String, String)> =
            vec![(source.snapshot_path.clone(), source.sha256.clone())];
        if source.semantic_identity_mode == SEMANTIC_IDENTITY_LEGACY {
            let Some(baseline) = &source.semantic_baseline_path else {
                return Err(format!(
                    "legacy Orc source lacks semantic baseline: {}",
                    source.source_path
                ));
            };
            retained_items.push((baseline.clone(), source.semantic_sha256.clone()));
            if baseline.starts_with(&format!("{SNAPSHOT_OBJECT_ROOT}/")) {
                retained_databases.push(baseline.clone());
            }
        }
        if let Some(alias) = &source.semantic_alias_baseline_path {
            let is_managed = managed_object_digest(alias);
            let Some(digest) = is_managed else {
                return Err(format!("invalid Orc semantic alias baseline: {alias}"));
            };
            retained_databases.push(alias.clone());
            retained_items.push((alias.clone(), digest));
        }
        if let Some(projection) = &source.task_projection {
            retained_projections.push(projection.path.clone());
            retained_items.push((projection.path.clone(), projection.sha256.clone()));
        }
        for (relative, expected_sha256) in &retained_items {
            let path = snapshot_path(snapshot_root, relative)?;
            let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
                format!(
                    "retained Orc snapshot artifact is missing: {}",
                    path.display()
                )
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(format!(
                    "retained Orc snapshot artifact is unsafe: {}",
                    path.display()
                ));
            }
            if sha256_file(&path)? != *expected_sha256 {
                return Err(format!(
                    "retained Orc snapshot artifact has wrong hash: {}",
                    path.display()
                ));
            }
            if relative == source.semantic_baseline_path.as_deref().unwrap_or("")
                && metadata.len() as i64 != source.semantic_complete_bytes
            {
                return Err(format!(
                    "retained Orc semantic baseline has wrong size: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(prune_managed_objects(
        snapshot_root,
        SNAPSHOT_OBJECT_ROOT,
        ".db",
        &retained_databases,
    )? + prune_managed_objects(
        snapshot_root,
        TASK_PROJECTION_ROOT,
        ".json",
        &retained_projections,
    )?)
}

/// Remove managed candidates left by a terminated
/// ingest transaction; returns the count removed.
pub fn prune_orc_staging(snapshot_root: &Path) -> Result<i64, String> {
    let staging = snapshot_path(snapshot_root, ".staging")?;
    let metadata = match std::fs::symlink_metadata(&staging) {
        Ok(held) => held,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(crate::json::os_error(&error, &staging)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!("Orc staging root is unsafe: {}", staging.display()));
    }
    let managed = |name: &str| -> bool {
        fn staged_ok(name: &str, prefix: &str, extension: &str) -> bool {
            let Some(rest) = name.strip_prefix(prefix) else {
                return false;
            };
            let Some(core) = rest.strip_suffix(extension) else {
                return false;
            };
            // orc-<pid>-<16 hex> (suffixes wal/shm stripped by caller)
            let Some((pid, token)) = core.split_once('-') else {
                return false;
            };
            !pid.is_empty()
                && pid.bytes().all(|b| b.is_ascii_digit())
                && token.len() == 16
                && token
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        if staged_ok(name, "orc-", ".db") || staged_ok(name, "task-projection-", ".json") {
            return true;
        }
        for suffix in ["-wal", "-shm"] {
            if let Some(base) = name.strip_suffix(suffix) {
                let base = format!("{base}.db");
                if staged_ok(&base, "orc-", ".db") {
                    return true;
                }
            }
        }
        false
    };
    let mut removed = 0i64;
    let mut candidates: Vec<PathBuf> = Vec::new();
    let entries =
        std::fs::read_dir(&staging).map_err(|error| crate::json::os_error(&error, &staging))?;
    for entry in entries.flatten() {
        candidates.push(entry.path());
    }
    candidates.sort_by_key(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()));
    for candidate in &candidates {
        let candidate_meta = std::fs::symlink_metadata(candidate)
            .map_err(|error| crate::json::os_error(&error, candidate))?;
        if candidate_meta.file_type().is_symlink() || !candidate_meta.is_file() {
            return Err(format!(
                "unsafe entry in Orc staging directory: {}",
                candidate.display()
            ));
        }
        let name = candidate
            .file_name()
            .map(|held| held.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !managed(&name) {
            continue;
        }
        std::fs::remove_file(candidate)
            .map_err(|error| crate::json::os_error(&error, candidate))?;
        removed += 1;
    }
    if removed > 0 {
        fsync_directory(&staging)?;
    }
    Ok(removed)
}
