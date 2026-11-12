//! The single source of truth for the wrkslots state and handoff formats.
//!
//! These types define the `ACTIVE.<machine>.json` envelope and records,
//! the `ARCHIVED.<machine>.json` envelope and records, and the handoff
//! sidecar (current schema 2 and legacy schema 1). The JSON Schemas
//! under `validation/schemas/` are generated from these types and
//! checked in; the hand validators in `active_state`, `archive_state`,
//! `handoff_state` and `schema` enforce the same shapes plus the
//! cross-field rules a schema cannot express. Field sets are exact:
//! unknown fields are refused, matching both editions today.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

fn schema_two(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "integer", "const": 2}))
        .expect("schema-two schema is valid")
}

fn schema_one(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    serde_json::from_value(serde_json::json!({"type": "integer", "const": 1}))
        .expect("schema-one schema is valid")
}

/// A process identity recorded in a state record.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    /// Process identifier.
    #[schemars(range(min = 1))]
    pub pid: u64,
    /// Process start time in clock ticks.
    #[schemars(range(min = 1))]
    pub start_ticks: u64,
    /// Boot identifier.
    pub boot_id: String,
    /// Host identifier.
    pub host_id: String,
    /// The process's cgroup path.
    pub cgroup_path: String,
}

/// One checkout of a slot.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    /// Checkout name.
    pub name: String,
    /// Path relative to the slot directory.
    pub path: String,
    /// Repository the checkout came from.
    pub repository: String,
    /// Branch checked out.
    pub branch: String,
    /// Start-point digest.
    #[schemars(length(equal = 40))]
    pub start_point: String,
    /// Remote name.
    pub remote: String,
    /// Digest of the remote URL.
    #[schemars(length(equal = 64))]
    pub remote_url_sha256: String,
    /// Ref the work is expected to land on.
    pub landed_ref: String,
    /// Head digest.
    #[schemars(length(equal = 40))]
    pub head: String,
    /// Remote refs containing the head.
    pub containing_remote_refs: Vec<String>,
    /// Version-control system; currently always `git`.
    pub vcs: Vcs,
}

/// The handoff summary embedded in an active record.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffSummary {
    /// When the handoff was recorded.
    pub recorded_at: String,
    /// Validation evidence.
    pub validation: Vec<String>,
    /// Known limitations.
    pub limitations: Vec<String>,
    /// Exact continuation instructions.
    pub continuation: String,
    /// Commits finish discarded under --discard-unaccounted (#1364).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discarded_unaccounted: Vec<String>,
}

/// Where an imported record came from.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportSource {
    /// Source format; currently `worktree-state-v3`.
    pub format: ImportFormat,
    /// Source path relative to the registry.
    pub path: String,
    /// Digest of the source file.
    #[schemars(length(equal = 64))]
    pub file_sha256: String,
    /// Digest of the source row.
    #[schemars(length(equal = 64))]
    pub row_sha256: String,
    /// Status of the source row.
    pub status: ImportStatus,
    /// The source row itself; its own format is historical.
    pub row: Map<String, Value>,
}

/// The task scope identity recorded on an active record.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskScope {
    /// The systemd unit of the task leader.
    pub unit: String,
    /// Invocation identifier of the unit.
    pub invocation_id: String,
    /// Cgroup path of the task.
    pub cgroup_path: String,
    /// Boot identifier.
    pub boot_id: String,
    /// Leader process identifier.
    #[schemars(range(min = 1))]
    pub leader_pid: u64,
    /// Leader start ticks.
    #[schemars(range(min = 1))]
    pub leader_start_ticks: u64,
    /// How the scope was verified.
    pub verification: Verification,
}

/// Version-control system of a checkout.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Vcs {
    /// Git.
    Git,
    /// Eden.
    Eden,
}

/// Import source format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub enum ImportFormat {
    /// The v3 worktree-state format.
    #[serde(rename = "worktree-state-v3")]
    WorktreeStateV3,
}

/// Status of an imported source row.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportStatus {
    /// Live.
    Active,
    /// Quarantined by its lease.
    LeaseQuarantined,
    /// Owner lease revoked.
    OwnerLeaseRevoked,
    /// Release requested.
    ReleaseRequested,
}

/// Retirement mode of an archived record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveMode {
    /// The slot was removed.
    Remove,
}

/// Who retired an archived record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveActor {
    /// The coordinator.
    Coordinator,
}

/// Physical storage state of an archived record.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhysicalStorage {
    /// Storage was removed.
    Removed,
}

/// Task-scope verification method.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub enum Verification {
    /// The systemd runtime invocation symlink proof.
    #[serde(rename = "systemd-runtime-invocation-symlink-v1")]
    SystemdRuntimeInvocationSymlinkV1,
}

/// Slot kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotType {
    /// A working agent slot.
    Agent,
    /// A validation slot.
    Validate,
}

/// Checkout layout of a slot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Checkouts nested under one slot directory.
    Nested,
    /// One checkout at the slot directory.
    Flat,
}

/// One active slot record in `ACTIVE.<machine>.json`.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveRecord {
    /// Slot name.
    pub slot: String,
    /// Owning agent name.
    pub agent: String,
    /// Task the slot serves.
    pub task: String,
    /// Purpose of the slot.
    pub purpose: String,
    /// Machine the slot lives on.
    pub machine: String,
    /// Slot generation.
    #[schemars(range(min = 1))]
    pub generation: u64,
    /// When the slot was created.
    pub created_at: String,
    /// Last heartbeat time.
    pub heartbeat_at: String,
    /// Heartbeat time-to-live in seconds.
    #[schemars(range(min = 1))]
    pub heartbeat_ttl_seconds: u64,
    /// Bound owner process, or null.
    pub owner: Option<ProcessIdentity>,
    /// Coordinator lease process; always present on a valid record
    /// (both editions refuse a record without one).
    pub coordinator_lease: ProcessIdentity,
    /// Recovery note, or null.
    pub coordinator_recovery_note: Option<String>,
    /// Latest handoff summary, or null.
    pub handoff: Option<HandoffSummary>,
    /// The slot's checkouts.
    pub checkouts: Vec<Checkout>,
    /// Slot kind; absent means `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_type: Option<SlotType>,
    /// Checkout layout, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    /// Import provenance, when imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<ImportSource>,
    /// Task scope identity, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_scope: Option<TaskScope>,
}

/// The `ACTIVE.<machine>.json` envelope, schema 2.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveState {
    /// Format schema; always 2.
    #[schemars(schema_with = "schema_two")]
    pub schema: u64,
    /// Machine this shard belongs to.
    pub machine: String,
    /// Monotonic revision of the shard.
    pub revision: u64,
    /// Active slot records.
    pub slots: Vec<ActiveRecord>,
}

/// One archived slot record in `ARCHIVED.<machine>.json`.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveRecord {
    /// Archive identifier binding machine, slot, generation and finish.
    pub archive_id: String,
    /// Slot name.
    pub slot: String,
    /// Owning agent name.
    pub agent: String,
    /// Task the slot served.
    pub task: String,
    /// Purpose of the slot.
    pub purpose: String,
    /// Machine the slot lived on.
    pub machine: String,
    /// Slot generation.
    #[schemars(range(min = 1))]
    pub generation: u64,
    /// When the slot was created.
    pub created_at: String,
    /// When the slot was retired.
    pub finished_at: String,
    /// Retirement mode; currently `remove`.
    pub mode: ArchiveMode,
    /// Who retired it; currently `coordinator`.
    pub actor: ArchiveActor,
    /// Physical storage state; currently `removed`.
    pub physical_storage: PhysicalStorage,
    /// Validation evidence.
    pub validation: Vec<String>,
    /// Known limitations.
    pub limitations: Vec<String>,
    /// Continuation instructions.
    pub continuation: String,
    /// The slot's checkouts.
    pub checkouts: Vec<Checkout>,
    /// Slot kind; absent means `agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slot_type: Option<SlotType>,
    /// Salvaged items, when any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salvage: Option<Vec<Map<String, Value>>>,
    /// Checkout layout, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<Layout>,
    /// Import provenance, when imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<ImportSource>,
}

/// The `ARCHIVED.<machine>.json` envelope, schema 2.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedState {
    /// Format schema; always 2.
    #[schemars(schema_with = "schema_two")]
    pub schema: u64,
    /// Machine this shard belongs to.
    pub machine: String,
    /// Monotonic revision of the shard.
    pub revision: u64,
    /// Archived slot records.
    pub records: Vec<ArchiveRecord>,
}

/// Identity of the handoff source file. Writers emit exactly these
/// keys; loaders accept (and ignore) extra keys, so the schema does.
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct SourceIdentity {
    /// `[device, inode]` of the source file.
    pub identity: [u64; 2],
    /// Size of the source file in bytes.
    pub size: u64,
    /// SHA-256 of the source file, lowercase hex.
    #[schemars(length(equal = 64))]
    pub sha256: String,
}

/// Handoff source kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffSource {
    /// Written by `write-handoff`.
    WriteHandoff,
    /// Provenance-upgraded from a legacy slot file.
    LegacySlotFile,
}

/// A generation-bound handoff sidecar, schema 2 (current).
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffSidecar {
    /// Format schema; always 2.
    #[schemars(schema_with = "schema_two")]
    pub schema: u64,
    /// Machine the slot lives on.
    pub machine: String,
    /// Slot name.
    pub slot: String,
    /// Slot generation the handoff belongs to.
    #[schemars(range(min = 1))]
    pub generation: u64,
    /// When the handoff was recorded.
    pub recorded_at: String,
    /// How the handoff originated.
    pub source: HandoffSource,
    /// Normalized absolute path of the source file.
    pub source_path: String,
    /// Identity of the source file.
    pub source_identity: SourceIdentity,
    /// Handoff contents as UTF-8 text.
    pub contents_utf8: String,
    /// SHA-256 of the contents, lowercase hex.
    #[schemars(length(equal = 64))]
    pub sha256: String,
}

/// A generation-bound handoff sidecar, schema 1 (legacy, read-only).
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffSidecarLegacy {
    /// Format schema; always 1.
    #[schemars(schema_with = "schema_one")]
    pub schema: u64,
    /// Machine the slot lives on.
    pub machine: String,
    /// Slot name.
    pub slot: String,
    /// Slot generation the handoff belongs to.
    #[schemars(range(min = 1))]
    pub generation: u64,
    /// When the handoff was recorded.
    pub recorded_at: String,
    /// How the handoff originated.
    pub source: HandoffSource,
    /// Identity of the source file, when recorded.
    pub source_identity: Option<SourceIdentity>,
    /// Handoff contents as UTF-8 text.
    pub contents_utf8: String,
    /// SHA-256 of the contents, lowercase hex.
    #[schemars(length(equal = 64))]
    pub sha256: String,
}

/// Mark these fields required even though they are nullable.
fn require(schema: &mut Value, names: &[&str]) {
    let required = schema
        .as_object_mut()
        .expect("schema is an object")
        .entry("required")
        .or_insert_with(|| Value::Array(Vec::new()));
    let list = required.as_array_mut().expect("required is an array");
    for name in names {
        if !list.iter().any(|item| item == name) {
            list.push(Value::String((*name).to_owned()));
        }
    }
    list.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
}

/// Remove the `null` alternative a serde `Option` generates for a
/// field both editions accept only when absent-or-valued, never null.
fn denull(defs: &mut Value, record: &str, names: &[&str]) {
    let Some(properties) = defs[record]["properties"].as_object_mut() else {
        return;
    };
    for name in names {
        let Some(field) = properties.get_mut(*name) else {
            continue;
        };
        if let Some(any_of) = field.get("anyOf").and_then(Value::as_array).cloned() {
            let non_null: Vec<Value> = any_of
                .into_iter()
                .filter(|branch| branch.get("type") != Some(&Value::String("null".to_owned())))
                .collect();
            if non_null.len() == 1 {
                let only = non_null.into_iter().next().expect("one branch");
                if let Some(obj) = field.as_object_mut() {
                    obj.remove("anyOf");
                    if let Value::Object(branch) = only {
                        for (key, value) in branch {
                            obj.insert(key, value);
                        }
                    }
                }
            }
        }
        if let Some(types) = field.get("type").and_then(Value::as_array).cloned() {
            let kept: Vec<Value> = types
                .into_iter()
                .filter(|item| item != &Value::String("null".to_owned()))
                .collect();
            if kept.len() == 1 {
                field["type"] = kept.into_iter().next().expect("one type");
            }
        }
    }
}

/// The generated schema of each format, keyed by file stem.
#[must_use]
pub fn generated_schemas() -> Vec<(&'static str, Value)> {
    let mut active = serde_json::to_value(schemars::schema_for!(ActiveState))
        .expect("active state schema serializes");
    if let Some(defs) = active.get_mut("$defs") {
        let mut patch = defs.clone();
        denull(
            &mut patch,
            "ActiveRecord",
            &["slot_type", "layout", "task_scope"],
        );
        if let Some(record) = patch.get_mut("ActiveRecord") {
            require(record, &["owner", "coordinator_recovery_note", "handoff"]);
        }
        active["$defs"] = patch;
    }
    let mut archived = serde_json::to_value(schemars::schema_for!(ArchivedState))
        .expect("archived state schema serializes");
    if let Some(defs) = archived.get_mut("$defs") {
        let mut patch = defs.clone();
        denull(
            &mut patch,
            "ArchiveRecord",
            &["slot_type", "layout", "salvage"],
        );
        archived["$defs"] = patch;
    }
    let mut legacy = serde_json::to_value(schemars::schema_for!(HandoffSidecarLegacy))
        .expect("legacy handoff sidecar schema serializes");
    require(&mut legacy, &["source_identity"]);
    vec![
        ("wrkslots-active-state", active),
        ("wrkslots-archived-state", archived),
        (
            "wrkslots-handoff-sidecar",
            serde_json::to_value(schemars::schema_for!(HandoffSidecar))
                .expect("handoff sidecar schema serializes"),
        ),
        ("wrkslots-handoff-sidecar-v1", legacy),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_record_refuses_unknown_fields() {
        let value = serde_json::json!({
            "slot": "s", "agent": "a", "task": "t", "purpose": "p",
            "machine": "m", "generation": 1, "created_at": "2026-01-01T00:00:00Z",
            "heartbeat_at": "2026-01-01T00:00:00Z", "heartbeat_ttl_seconds": 60,
            "owner": null,
            "coordinator_lease": {"pid": 1, "start_ticks": 2, "boot_id": "b", "host_id": "m", "cgroup_path": "/x"},
            "coordinator_recovery_note": null, "handoff": null,
            "checkouts": [], "bogus": 1
        });
        assert!(serde_json::from_value::<ActiveRecord>(value).is_err());
    }

    #[test]
    fn every_format_generates_a_schema() {
        assert_eq!(generated_schemas().len(), 4);
    }
}
