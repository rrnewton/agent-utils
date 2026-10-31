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
