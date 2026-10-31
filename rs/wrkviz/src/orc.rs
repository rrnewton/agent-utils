//! Import Orc SQLite session lineages:
//! `py/wrkviz/orc.py` at parity. This module lands in
//! stages: manifest record types first, then discovery
//! and snapshotting, then team assembly.

use serde_json::{Map, Value};
use sha2::Digest as _;

use crate::json::py_repr_str;

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
pub fn nonnegative_u128(value: Option<&Value>, where_: &str) -> Result<u128, String> {
    // Append watermarks pack two 64-bit ids and a tag bit.
    match value {
        Some(Value::Number(number)) => {
            let text = number.to_string();
            if text.starts_with('-') {
                return Err(format!("{where_}: expected a non-negative integer"));
            }
            text.parse::<u128>()
                .map_err(|_| format!("{where_}: expected a non-negative integer"))
        }
        _ => Err(format!("{where_}: expected a non-negative integer")),
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
    pub append_max_id: u128,
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
        out.insert("append_max_id".into(), self.append_max_id.into());
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
            append_max_id: nonnegative_u128(
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
