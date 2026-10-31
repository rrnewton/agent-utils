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
        if let Some(text) = raw.as_str() {
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
    pub append_max_id: i64,
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
            append_max_id: nonnegative_integer(
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
