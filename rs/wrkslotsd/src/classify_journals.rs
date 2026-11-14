//! Reader: classify outstanding create journals (`classify-create-journals`).
//!
//! Reproduces the Python edition's read-only classification: every
//! outstanding journal must be a create journal whose target is
//! unrelated to the requested slot, with process liveness, planned /
//! created checkout agreement, and worktree dirtiness inspected.
//! No locks, authorization, or writes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::active_state::{as_int, as_list, as_mapping, as_str, load_cfg, slot_directory, Cfg};
use crate::batch_text::sort_keys;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::git_storage_state::git_run;
use crate::global_state::load_event_chain;
use crate::journal_state::validate_journal_shape;
use crate::status_state::process_state;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
struct Row {
    machine: String,
    slot: String,
    agent: String,
    slot_type: String,
    slot_path: PathBuf,
    planned: Vec<Value>,
    created: Vec<Value>,
    observed: Vec<Value>,
    dirty: Vec<String>,
    owner_detail: String,
    owner_state: String,
    coordinator_detail: String,
    coordinator_state: String,
    state: String,
    journal: PathBuf,
    identity: FileIdentity,
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

pub(crate) type FileIdentity = (u64, u64, u64, String);

pub(crate) fn read_identity(path: &Path) -> Result<(Vec<u8>, FileIdentity)> {
    use std::os::unix::fs::MetadataExt as _;
    let meta = std::fs::metadata(path)
        .map_err(|e| format!("cannot read recovery journal {}: {e}", path.display()))?;
    if meta.len() > 1024 * 1024 {
        return Err(format!("recovery journal {} is too large", path.display()));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| format!("cannot read recovery journal {}: {e}", path.display()))?;
    let digest = {
        use sha2::{Digest as _, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    Ok((bytes, (meta.dev(), meta.ino(), meta.len(), digest)))
}

pub(crate) fn strict_obj(bytes: &[u8]) -> Result<Map<String, Value>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "recovery journal is not valid UTF-8".to_string())?;
    let value: Value = serde_json::from_str(text)
        .map_err(|e| format!("recovery journal is malformed JSON: {e}"))?;
    as_mapping(&value, "recovery journal").cloned()
}

fn pending_ops(control: &Path, machine: &str) -> Result<Vec<(String, Value)>> {
    let events = load_event_chain(&control.join(format!("EVENTS.{machine}")), machine)?;
    let mut pending: Vec<(String, Value)> = Vec::new();
    for event in &events {
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind == "operation-progress-recorded" {
            let journal = payload.get("journal").cloned().unwrap_or(Value::Null);
            let name = payload
                .get("journal_path")
                .and_then(Value::as_str)
                .unwrap_or("ACTIVE.journal")
                .to_string();
            if let Some(slot) = pending.iter_mut().find(|(n, _)| *n == name) {
                slot.1 = journal;
            } else {
                pending.push((name, journal));
            }
        } else if kind == "operation-completed" || kind == "operation-quarantined" {
            let name = payload
                .get("journal_path")
                .and_then(Value::as_str)
                .unwrap_or("ACTIVE.journal")
                .to_string();
            if let Some(index) = pending.iter().position(|(n, _)| *n == name) {
                pending.remove(index);
            }
        }
    }
    Ok(pending)
}

fn outstanding(control: &Path, machines: Option<&BTreeSet<String>>) -> Result<Vec<PathBuf>> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(control) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("ACTIVE.")
                || name.starts_with("CREATE.")
                || name.starts_with("FINISH."))
                && name.ends_with(".journal")
            {
                // #873: a machine-scoped read skips journals that name a
                // foreign machine; unscoped journals count as local.
                if let Some(machines) = machines {
                    if let Some(machine) = crate::global_state::journal_machine(&name) {
                        if !machines.contains(&machine) {
                            continue;
                        }
                    }
                }
                paths.insert(entry.path());
            }
            if name.starts_with("EVENTS.") && entry.path().is_dir() {
                let machine = name.trim_start_matches("EVENTS.").to_string();
                if let Some(machines) = machines {
                    if !machines.contains(&machine) {
                        continue;
                    }
                }
                for (journal_name, _) in pending_ops(control, &machine)? {
                    paths.insert(control.join(journal_name));
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub(crate) fn create_journal_path(cfg: &Cfg, slot: &str, machine: &str) -> PathBuf {
    let machine_part = format!("{}.{}", machine.chars().count(), machine);
    let slot_part = format!("{}.{}", slot.chars().count(), slot);
    cfg.control
        .join(format!("CREATE.{machine_part}.{slot_part}.journal"))
}

/// Episode machine over append-only progress/completion evidence for
/// one scoped create journal path (Python `_scoped_operation_journal_completed`).
fn scoped_completed(cfg: &Cfg, path: &Path, raw: &Map<String, Value>) -> Result<bool> {
    let machine = as_str(
        raw.get("machine").unwrap_or(&Value::Null),
        "create journal.machine",
    )?;
    let slot = validate_name(
        as_str(
            raw.get("slot").unwrap_or(&Value::Null),
            "create journal.slot",
        )?,
        "slot",
    )?;
    if path != create_journal_path(cfg, &slot, machine) {
        return Err("scoped create journal filename does not match its target".to_string());
    }
    let events = load_event_chain(&cfg.control.join(format!("EVENTS.{machine}")), machine)?;
    let mut recorded: Option<Value> = None;
    let mut completed = false;
    let mut current: Option<String> = None;
    let mut retired: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for event in &events {
        let kind = as_str(
            event.get("kind").unwrap_or(&Value::Null),
            "append-only event.kind",
        )?;
        if kind != "operation-progress-recorded" && kind != "operation-completed" {
            continue;
        }
        let payload = as_mapping(
            event.get("payload").unwrap_or(&Value::Null),
            "append-only event.payload",
        )?;
        let event_name = match payload.get("journal_path").and_then(Value::as_str) {
            Some(name) => name.to_string(),
            None => {
                let operation = payload
                    .get("operation")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let event_slot = payload
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if operation == "create" {
                    format!(
                        "CREATE.{}.{}.{}.{}.journal",
                        machine.chars().count(),
                        machine,
                        event_slot.chars().count(),
                        event_slot
                    )
                } else if operation == "finish" {
                    format!(
                        "FINISH.{}.{}.{}.{}.journal",
                        machine.chars().count(),
                        machine,
                        event_slot.chars().count(),
                        event_slot
                    )
                } else {
                    format!("ACTIVE.{machine}.journal")
                }
            }
        };
        if cfg.control.join(&event_name) != path {
            continue;
        }
        if kind == "operation-progress-recorded" {
            let journal = payload.get("journal").cloned().unwrap_or(Value::Null);
            let identity = journal
                .get("operation_id")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let opens = journal
                .get("created")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
                && journal.get("hook_progress").and_then(Value::as_i64) == Some(0);
            if completed && !(opens && identity.is_some()) {
                return Err(
                    "scoped create journal path was reused after completion without a new operation"
                        .to_string(),
                );
            }
            if let Some(id) = &identity {
                if retired.contains(id) {
                    return Err(
                        "scoped create journal path was reopened with the identity of a completed operation"
                            .to_string(),
                    );
                }
            }
            if recorded.is_none() || completed {
                current = identity.clone();
            } else if identity != current {
                return Err(
                    "scoped create journal progress changed its operation identity".to_string(),
                );
            }
            recorded = Some(journal);
            completed = false;
        } else {
            if recorded.is_none() {
                return Err(
                    "scoped create completion has no preceding progress evidence".to_string(),
                );
            }
            if let Some(id) = &current {
                retired.insert(id.clone());
            }
            completed = true;
        }
    }
    let Some(recorded) = recorded else {
        return Err(
            "scoped create journal has no matching append-only progress evidence".to_string(),
        );
    };
    if recorded != Value::Object(raw.clone()) {
        return Err("scoped create journal differs from append-only progress evidence".to_string());
    }
    Ok(completed)
}

fn git_text(repo: &Path, args: &[&str]) -> Result<String> {
    git_run(repo, args)
}

fn branch_exists(repo: &Path, branch: &str) -> Result<bool> {
    let output = git_run(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    );
    match output {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

fn listed_worktrees(repo: &Path) -> Result<BTreeSet<PathBuf>> {
    let out = git_text(repo, &["worktree", "list", "--porcelain"])?;
    let mut set = BTreeSet::new();
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            set.insert(PathBuf::from(path));
        }
    }
    Ok(set)
}

fn expected_checkout(
    cfg: &Cfg,
    slot: &str,
    name: &str,
    slot_type: &str,
) -> Result<(String, PathBuf)> {
    let dir = slot_directory(cfg, slot, slot_type)?;
    let destination = if cfg.layout == "flat" {
        dir.clone()
    } else {
        dir.join(name)
    };
    let relative = destination
        .strip_prefix(&cfg.root)
        .unwrap_or(&destination)
        .to_string_lossy()
        .replace('\\', "/");
    Ok((relative, destination))
}

fn field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Whether an interrupted journal is a cargo-home removal disjoint
/// from the requested slot: only an `ownerless-validate-remove` whose
/// fully decoded authorization names a cargo home, and whose target and
/// fenced paths cannot overlap the requested slot path. Any other
/// interrupted mutation, and any journal that does not decode cleanly,
/// keeps the blanket refusal (the Python `_ownerless_cargo_home_journal_is_unrelated`).
fn ownerless_cargo_home_journal_is_unrelated(
    cfg: &Cfg,
    raw: &Map<String, Value>,
    requested_slot: &str,
    requested_slot_type: &str,
) -> bool {
    if raw.get("kind").and_then(Value::as_str) != Some("ownerless-validate-remove") {
        return false;
    }
    let (target, fenced) = (|| -> Result<(PathBuf, PathBuf)> {
        let (target_kind, auth_path) =
            crate::journal_state::validation_authorization(&raw["authorization"])?;
        let target = crate::journal_state::recovery_target(cfg, &auth_path, &target_kind)?;
        if target_kind != "cargo-home" {
            return Err("not a cargo home".to_string());
        }
        let raw_fenced = field(&Value::Object(raw.clone()), "fenced");
        let fenced =
            crate::config_validate::relative_inside(&cfg.root, &raw_fenced, "fenced path")?.1;
        Ok((target, fenced))
    })()
    .unwrap_or_default();
    if target.as_os_str().is_empty() {
        return false;
    }
    let requested = match slot_directory(cfg, requested_slot, requested_slot_type) {
        Ok(path) => path,
        Err(_) => return false,
    };
    ![&target, &fenced]
        .iter()
        .any(|candidate| within(candidate, &requested) || within(&requested, candidate))
}

/// The live owner a non-create journal records, if it is still live
/// (mirrors the Python `_live_journal_owner`): the first parseable
/// identity in owner order decides, and a carried task scope must
/// still verify, so a pid whose scope ended is never busy.
fn live_journal_owner(raw: &Map<String, Value>) -> Option<crate::status_state::Identity> {
    let record = raw.get("record").and_then(Value::as_object);
    let candidates: Vec<&Value> = vec![
        raw.get("owner").unwrap_or(&Value::Null),
        record.and_then(|r| r.get("owner")).unwrap_or(&Value::Null),
        raw.get("actor").unwrap_or(&Value::Null),
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
        record
            .and_then(|r| r.get("coordinator_lease"))
            .unwrap_or(&Value::Null),
        raw.get("coordinator").unwrap_or(&Value::Null),
    ];
    let mut owner: Option<crate::status_state::Identity> = None;
    for candidate in candidates {
        if !candidate.is_object() {
            continue;
        }
        if let Some(identity) = crate::status_state::identity_from_value(candidate) {
            owner = Some(identity);
            break;
        }
    }
    let owner = owner?;
    let scope_raw = raw
        .get("task_scope")
        .or_else(|| record.and_then(|r| r.get("task_scope")));
    if let Some(scope_value) = scope_raw {
        let scope = crate::active_state::scope_from_obj(scope_value, "journal task_scope").ok()?;
        if scope.leader_pid != owner.pid
            || scope.leader_start_ticks != owner.start_ticks
            || scope.boot_id != owner.boot_id
            || scope.cgroup_path != owner.cgroup_path
        {
            return None;
        }
        let unit = scope_value
            .get("unit")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let invocation = scope_value
            .get("invocation_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        crate::adopt_write::verify_task_scope(&owner, unit, invocation).ok()?;
    }
    if process_state(Some(&owner)).0 == "live" {
        Some(owner)
    } else {
        None
    }
}

/// A retryable busy result (#992), distinct from a hard refusal.
#[derive(Debug)]
pub(crate) enum ClassifyError {
    Busy {
        kind: String,
        pid: i64,
        journal: String,
    },
    /// A non-orphaned validation-batch seal blocks classification
    /// (c4c4c64a): `payload` is the structured JSON for stdout in
    /// JSON mode, `first` names the first blocking seal path.
    Blocked {
        first: String,
        payload: Option<String>,
    },
    Other(String),
}

impl From<String> for ClassifyError {
    fn from(message: String) -> Self {
        ClassifyError::Other(message)
    }
}

impl std::fmt::Display for ClassifyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClassifyError::Busy { kind, pid, journal } => {
                write!(
                    formatter,
                    "busy: live '{kind}' mutation {journal} by pid {pid}; retry"
                )
            }
            ClassifyError::Blocked { first, .. } => {
                write!(
                    formatter,
                    "interrupted validation-batch seal recorded in {first}; run 'wrkslots recover' first; list blocking journals with 'wrkslots classify-create-journals --all-kinds'"
                )
            }
            ClassifyError::Other(message) => write!(formatter, "{message}"),
        }
    }
}

pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

/// Whole seconds since the journal file last changed, if readable.
pub(crate) fn journal_age_seconds(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    Some(
        modified
            .elapsed()
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0),
    )
}

/// Name the journal that blocks host-wide classification (#1086),
/// mirroring `_journal_classification_refusal`: absolute path, slot,
/// agent and age best-effort from the journal bytes, inner reason kept.
fn journal_refusal(path: &Path, inner: &str) -> String {
    let absolute = absolute_path(path);
    let parsed: Option<Value> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let slot = parsed
        .as_ref()
        .and_then(|value| value.get("slot"))
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let agent = parsed
        .as_ref()
        .and_then(|value| value.get("agent"))
        .and_then(Value::as_str)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|value| value.get("record"))
                .and_then(|record| record.get("agent"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string();
    let age = match journal_age_seconds(path) {
        Some(seconds) => format!("{seconds}s"),
        None => "unknown".to_string(),
    };
    format!(
        "host fault: journal {} (slot {slot:?}, agent {agent:?}, age={age}) blocks create-journal classification: {inner}. remedy: recover or discard the named journal with 'wrkslots recover', then rerun classify-create-journals",
        absolute.display()
    )
}

/// First parseable owner identity, in `live_journal_owner` order but
/// without its task-scope or liveness filtering, mirroring
/// `_journal_report_identity`: the report names dead owners too.
fn report_identity(raw: &Map<String, Value>) -> Option<crate::status_state::Identity> {
    let record = raw.get("record").and_then(Value::as_object);
    let candidates: Vec<&Value> = vec![
        raw.get("owner").unwrap_or(&Value::Null),
        record.and_then(|r| r.get("owner")).unwrap_or(&Value::Null),
        raw.get("actor").unwrap_or(&Value::Null),
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
        record
            .and_then(|r| r.get("coordinator_lease"))
            .unwrap_or(&Value::Null),
        raw.get("coordinator").unwrap_or(&Value::Null),
    ];
    for candidate in candidates {
        if !candidate.is_object() {
            continue;
        }
        if let Some(identity) = crate::status_state::identity_from_value(candidate) {
            return Some(identity);
        }
    }
    None
}

/// One best-effort report row, mirroring `_all_journal_row` (#1098):
/// never validates or refuses; unreadable journals keep unknown fields.
fn all_journal_row(cfg: &Cfg, path: &Path, seal: bool) -> Value {
    let parsed: Option<Value> = std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let raw: Map<String, Value> = parsed
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let record = raw.get("record").and_then(Value::as_object);
    let text = |key: &str| -> Option<String> {
        raw.get(key)
            .and_then(Value::as_str)
            .or_else(|| record.and_then(|r| r.get(key)).and_then(Value::as_str))
            .map(str::to_owned)
    };
    let kind = text("kind").unwrap_or_else(|| {
        if seal {
            "validate-batch-seal".to_string()
        } else {
            "unknown".to_string()
        }
    });
    let identity = if raw.is_empty() {
        None
    } else {
        report_identity(&raw)
    };
    let owner_state = match &identity {
        Some(identity) => process_state(Some(identity)).0,
        None => "unknown".to_string(),
    };
    let mut targets: Vec<String> = Vec::new();
    let blocks = if seal {
        if let Ok(parsed) = crate::audit_state::read_seal_json(path)
            .and_then(|value| crate::audit_state::seal_journal(cfg, path, &value))
        {
            targets = parsed.1.iter().map(|(slot, _g, _p)| slot.clone()).collect();
        }
        if seal_orphaned(cfg, path) {
            "none"
        } else {
            "lifecycle"
        }
    } else if raw.is_empty() {
        "unknown"
    } else if kind == "create" {
        "create-same-slot-agent-path"
    } else if owner_state == "live" {
        "concurrent-mutations"
    } else {
        "disjoint-create-only"
    };
    json!({
        "age_seconds": journal_age_seconds(path),
        "agent": text("agent"),
        "blocks": blocks,
        "journal": absolute_path(path).to_string_lossy(),
        "kind": kind,
        "machine": text("machine"),
        "owner_pid": identity.as_ref().map(|identity| identity.pid),
        "owner_state": owner_state,
        "slot": text("slot"),
        "targets": targets,
    })
}

/// Report every lifecycle journal, read-only, mirroring
/// `_cmd_classify_all_journals` (#1098). Seals are listed for every
/// shard, as in the Python report; other journals follow #873 scoping.
fn classify_all_journals(
    root: Option<&Path>,
    machine_override: Option<&str>,
    format_json: bool,
    all_machines: bool,
) -> std::result::Result<String, ClassifyError> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let foreign_notes: Vec<Value> = if all_machines {
        Vec::new()
    } else {
        crate::global_state::foreign_shard_notes(&cfg.control, &cfg.machine)?
    };
    let machines: Option<BTreeSet<String>> = if all_machines {
        None
    } else {
        Some(BTreeSet::from([cfg.machine.clone()]))
    };
    let mut rows: Vec<Value> = Vec::new();
    for path in outstanding(&cfg.control, machines.as_ref())? {
        rows.push(all_journal_row(&cfg, &path, false));
    }
    for path in crate::heartbeat_write::seal_paths(&cfg) {
        rows.push(all_journal_row(&cfg, &path, true));
    }
    rows.sort_by(|a, b| {
        a.get("journal")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("journal").and_then(Value::as_str).unwrap_or_default())
    });
    let blocking = rows
        .iter()
        .any(|row| row.get("blocks").and_then(Value::as_str) != Some("none"));
    if format_json {
        let payload = json!({
            "blocking": blocking,
            "foreign_shards": foreign_notes,
            "journals": rows,
            "schema": 2,
        });
        return Ok(format!(
            "{}\n",
            crate::batch_text::py_compact_pub(&sort_keys(&payload))
        ));
    }
    let blocked_count = rows
        .iter()
        .filter(|row| row.get("blocks").and_then(Value::as_str) != Some("none"))
        .count();
    let mut out = format!(
        "ALL JOURNALS: journals={} blocking={}\n",
        rows.len(),
        blocked_count
    );
    for row in &rows {
        let field = |key: &str| -> String {
            row.get(key)
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_string()
        };
        let age = match row.get("age_seconds").and_then(Value::as_u64) {
            Some(seconds) => format!("{seconds}s"),
            None => "unknown".to_string(),
        };
        let targets: Vec<String> = row
            .get("targets")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let target_text = if targets.is_empty() {
            "-".to_string()
        } else {
            targets.join(",")
        };
        let owner_pid = match row.get("owner_pid").and_then(Value::as_i64) {
            Some(pid) => pid.to_string(),
            None => "-".to_string(),
        };
        out.push_str(&format!(
            "JOURNAL: kind={} slot={} agent={} owner_pid={} owner_state={} age={} blocks={} targets={} journal={}\n",
            field("kind"),
            field("slot"),
            field("agent"),
            owner_pid,
            field("owner_state"),
            age,
            field("blocks"),
            target_text,
            field("journal"),
        ));
    }
    out.push_str(&crate::global_state::foreign_shard_note_lines(
        &foreign_notes,
    ));
    Ok(out)
}

/// Whether the seal at `path` was left by a process proven dead,
/// mirroring `_validate_batch_seal_orphaned`: valid seal, actor on
/// this host and boot, process dead, and no finish journal paired
/// with it. Any doubt keeps the seal binding.
fn seal_orphaned(cfg: &Cfg, path: &Path) -> bool {
    let Ok(value) = crate::audit_state::read_seal_json(path) else {
        return false;
    };
    let Ok((_machine, _targets)) = crate::audit_state::seal_journal(cfg, path, &value) else {
        return false;
    };
    let Some(actor) = value.get("actor") else {
        return false;
    };
    let Some(identity) = crate::status_state::identity_from_value(actor) else {
        return false;
    };
    if crate::status_state::process_state(Some(&identity)).0 != "dead" {
        return false;
    }
    // A paired finish keeps the seal binding; any outstanding finish
    // journal is treated as potentially paired (conservative).
    let machines = BTreeSet::from([cfg.machine.clone()]);
    if let Ok(paths) = outstanding(&cfg.control, Some(&machines)) {
        for other in &paths {
            if let Ok((bytes, _id)) = read_identity(other) {
                if let Ok(raw) = strict_obj(&bytes) {
                    if raw.get("kind").and_then(Value::as_str) == Some("finish") {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// Structured reasons each non-orphaned seal blocks classification,
/// mirroring `_classify_blocking_seal_reasons` (c4c4c64a).
fn seal_blocking_reasons(cfg: &Cfg) -> Vec<Value> {
    let mut reasons: Vec<Value> = Vec::new();
    for path in crate::heartbeat_write::seal_paths(cfg) {
        if seal_orphaned(cfg, &path) {
            continue;
        }
        let mut kind = "other".to_string();
        let mut operation = "unknown".to_string();
        let mut actor_state = "unknown".to_string();
        let mut targets: Vec<String> = Vec::new();
        let parsed_seal = crate::audit_state::read_seal_json(&path).and_then(|value| {
            crate::audit_state::seal_journal(cfg, &path, &value).map(|parsed| (value, parsed))
        });
        match parsed_seal {
            Ok((value, parsed)) => {
                kind = "validate-batch-seal".to_string();
                operation = "remove-validate-batch".to_string();
                targets = parsed.1.iter().map(|(slot, _g, _p)| slot.clone()).collect();
                if let Some(actor) = value.get("actor") {
                    if let Some(identity) = crate::status_state::identity_from_value(actor) {
                        let state = crate::status_state::process_state(Some(&identity)).0;
                        if state == "live" || state == "dead" {
                            actor_state = state;
                        }
                    }
                }
            }
            Err(_) => {
                if let Ok(value) = crate::audit_state::read_seal_json(&path) {
                    if value.get("kind").and_then(Value::as_str) == Some("validate-batch-seal") {
                        kind = "validate-batch-seal".to_string();
                    }
                }
            }
        }
        reasons.push(json!({
            "actor_state": actor_state,
            "kind": kind,
            "operation": operation,
            "path": path.to_string_lossy(),
            "targets": targets,
        }));
    }
    reasons
}

fn lock_device_major(device: u64) -> u32 {
    (((device >> 32) & 0xffff_f000) | ((device >> 8) & 0xfff)) as u32
}

fn lock_device_minor(device: u64) -> u32 {
    (((device >> 12) & 0xfff_ff00) | (device & 0xff)) as u32
}

/// The PID holding the registry's exclusive writer lock, if any. This
/// only reads `/proc/locks`; it never acquires the lock.
fn registry_writer_lock_holder(cfg: &Cfg) -> Option<i64> {
    use std::os::unix::fs::MetadataExt as _;

    let lock_path = cfg.control.join("ACTIVE.global.lock");
    let metadata = std::fs::metadata(lock_path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let wanted = (
        lock_device_major(metadata.dev()),
        lock_device_minor(metadata.dev()),
        metadata.ino(),
    );
    let lines = std::fs::read_to_string("/proc/locks").ok()?;
    for line in lines.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 || fields[1] != "FLOCK" || fields[3] != "WRITE" {
            continue;
        }
        let device: Vec<&str> = fields[5].split(':').collect();
        if device.len() != 3 {
            continue;
        }
        let observed = (
            u32::from_str_radix(device[0], 16).ok()?,
            u32::from_str_radix(device[1], 16).ok()?,
            device[2].parse::<u64>().ok()?,
        );
        if observed == wanted {
            return fields[4].parse::<i64>().ok();
        }
    }
    // A boxed step's /proc/locks can hide a holder in another PID
    // namespace; the holder's own descriptors still name the lock file.
    let processes = std::fs::read_dir("/proc").ok()?;
    for process in processes.flatten() {
        let name = process.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<i64>().ok()) else {
            continue;
        };
        let Ok(descriptors) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            use std::os::unix::fs::MetadataExt as _;

            let Ok(target) = std::fs::metadata(descriptor.path()) else {
                continue;
            };
            if (target.dev(), target.ino()) == (metadata.dev(), metadata.ino()) {
                return Some(pid);
            }
        }
    }
    None
}

enum StandaloneMismatch {
    Busy(i64),
    Refuse,
}

impl StandaloneMismatch {
    fn message(&self, raw: &Map<String, Value>, path: &Path) -> String {
        match self {
            StandaloneMismatch::Busy(pid) => format!(
                "busy: live '{}' mutation {} by pid {}; retry",
                raw.get("kind").and_then(Value::as_str).unwrap_or_default(),
                path.file_name().unwrap_or_default().to_string_lossy(),
                pid
            ),
            StandaloneMismatch::Refuse => {
                "standalone recovery journal differs from append-only operation evidence".to_owned()
            }
        }
    }
}

/// The standalone-journal provenance check the classify path applies
/// (#1033, mirroring Python's `_standalone_mismatch_busy`): a physical
/// journal that differs from append-only evidence is retryable busy
/// only while its writer lock is held, or while a task scope it carries
/// still verifies for its recorded owner. A merely live slot owner or
/// coordinator proves neither. With no such proof the same mismatch is
/// real corruption and keeps refusing.
fn standalone_mismatch(
    cfg: &Cfg,
    path: &Path,
    raw: &Map<String, Value>,
    machine: &str,
) -> Option<StandaloneMismatch> {
    if path != crate::journal_state::journal_legacy_path(cfg, machine) {
        return None;
    }
    let mut events_by_machine = std::collections::BTreeMap::new();
    let events = crate::global_state::load_event_chain(
        &cfg.control.join(format!("EVENTS.{machine}")),
        machine,
    )
    .ok()?;
    events_by_machine.insert(machine.to_owned(), events);
    let pending =
        crate::journal_state::pending_journal_for_path(cfg, &events_by_machine, path).ok()?;
    let pending = pending?;
    if crate::journal_state::json_equal(&Value::Object(raw.clone()), &Value::Object(pending)) {
        return None;
    }
    if let Some(holder) = registry_writer_lock_holder(cfg) {
        return Some(StandaloneMismatch::Busy(holder));
    }
    let record = raw.get("record").and_then(Value::as_object);
    let scoped = raw.get("task_scope").is_some_and(|scope| !scope.is_null())
        || record
            .and_then(|record| record.get("task_scope"))
            .is_some_and(|scope| !scope.is_null());
    if scoped {
        if let Some(owner) = live_journal_owner(raw) {
            return Some(StandaloneMismatch::Busy(owner.pid));
        }
    }
    Some(StandaloneMismatch::Refuse)
}

/// Classification state of one journal for the quarantine plan
/// (#1086c): `Ok(Some(state))` when it classifies, `Ok(None)` when it is
/// skipped as unrelated, and the refusal text otherwise.
pub(crate) fn create_journal_state(
    cfg: &Cfg,
    config: &Value,
    path: &Path,
    states: &[(String, Vec<Value>)],
    archives: &[Vec<Value>],
    slot: &str,
    slot_type: &str,
) -> Result<Option<String>> {
    classify_one(cfg, config, path, states, archives, slot, slot_type)
        .map(|row| row.map(|row| row.state))
}

/// One create-journal classification for the batch commands:
/// the journal path, its target slot path, and the Python
/// `_CreateJournalClassification.to_obj` rendering.
pub(crate) struct CreateJournalRow {
    /// The journal file.
    pub journal: PathBuf,
    /// The create target's slot directory.
    pub slot_path: PathBuf,
    /// The `to_obj` JSON rendering.
    pub obj: Value,
}

/// Python `_classify_create_journals` with no requested slot:
/// every outstanding journal across all machines, classified.
pub(crate) fn create_journal_rows(cfg: &Cfg, config: &Value) -> Result<Vec<CreateJournalRow>> {
    let mut states: Vec<(String, Vec<Value>)> = Vec::new();
    let mut archives: Vec<Vec<Value>> = Vec::new();
    for machine in crate::global_state::discover_machines(&cfg.control)? {
        let active_path = cfg.control.join(format!("ACTIVE.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&active_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                states.push((
                    machine.clone(),
                    doc.get("slots")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        let archive_path = cfg.control.join(format!("ARCHIVED.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&archive_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                archives.push(
                    doc.get("records")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
    }
    let paths = outstanding(&cfg.control, None)?;
    let mut rows: Vec<Row> = Vec::new();
    for path in &paths {
        let row =
            classify_one(cfg, config, path, &states, &archives, "", "agent").map_err(|error| {
                if error.starts_with("busy:") {
                    error
                } else {
                    journal_refusal(path, &error)
                }
            })?;
        if let Some(row) = row {
            rows.push(row);
        }
    }
    let mut seen_slots: BTreeSet<(String, String)> = BTreeSet::new();
    let mut seen_agents: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        if !seen_slots.insert((row.machine.clone(), row.slot.clone())) {
            return Err(journal_refusal(
                &row.journal,
                &format!(
                    "multiple interrupted creates claim {}/{}",
                    row.machine, row.slot
                ),
            ));
        }
        if row.slot_type == "agent" && !seen_agents.insert(row.agent.clone()) {
            return Err(journal_refusal(
                &row.journal,
                &format!("multiple interrupted creates claim agent {:?}", row.agent),
            ));
        }
    }
    Ok(rows
        .iter()
        .map(|row| CreateJournalRow {
            journal: row.journal.clone(),
            slot_path: row.slot_path.clone(),
            obj: row_obj(row),
        })
        .collect())
}

/// Python `_CreateJournalClassification.to_obj`.
fn row_obj(row: &Row) -> Value {
    json!({
        "agent": row.agent,
        "checkouts": row.observed.iter().map(|c| json!({
            "branch": field(c, "branch"),
            "dirty": row.dirty.contains(&field(c, "name")),
            "head": field(c, "head"),
            "name": field(c, "name"),
            "path": field(c, "path"),
        })).collect::<Vec<_>>(),
        "coordinator": row.coordinator_detail,
        "coordinator_state": row.coordinator_state,
        "created": row.created.len(),
        "dirty_checkouts": row.dirty,
        "journal": row.journal.to_string_lossy(),
        "journal_identity": {
            "device": row.identity.0,
            "inode": row.identity.1,
            "sha256": row.identity.3,
            "size": row.identity.2,
        },
        "machine": row.machine,
        "owner": row.owner_detail,
        "owner_state": row.owner_state,
        "planned": row.planned.len(),
        "registry_state": if row.state == "completed-create" { "exact-active" } else { "absent" },
        "slot": row.slot,
        "slot_path": row.slot_path.to_string_lossy(),
        "slot_type": row.slot_type,
        "state": row.state,
    })
}

/// Every outstanding journal path (Python `_outstanding_journals`).
pub(crate) fn outstanding_journal_paths(cfg: &Cfg) -> Result<Vec<PathBuf>> {
    outstanding(&cfg.control, None)
}

fn classify_one(
    cfg: &Cfg,
    config: &Value,
    path: &Path,
    states: &[(String, Vec<Value>)],
    archives: &[Vec<Value>],
    requested_slot: &str,
    requested_slot_type: &str,
) -> Result<Option<Row>> {
    if !path.exists() && path.symlink_metadata().is_err() {
        return Err(format!(
            "recovery journal {} is missing while append-only history still names it",
            path.display()
        ));
    }
    let (bytes, identity) = read_identity(path)?;
    let raw = strict_obj(&bytes)?;
    let (machine_s, kind, _slot_s) = validate_journal_shape(
        cfg,
        path.file_name()
            .unwrap_or_default()
            .to_str()
            .unwrap_or_default(),
        &raw,
    )?;
    if let Some(mismatch) = standalone_mismatch(cfg, path, &raw, &machine_s) {
        return Err(mismatch.message(&raw, path));
    }
    if kind != "create" {
        if ownerless_cargo_home_journal_is_unrelated(cfg, &raw, requested_slot, requested_slot_type)
        {
            return Ok(None);
        }
        if let Some(owner) = live_journal_owner(&raw) {
            return Err(format!(
                "busy: live '{kind}' mutation {} by pid {}; retry",
                path.file_name().unwrap_or_default().to_string_lossy(),
                owner.pid
            ));
        }
        return Err(format!(
            "interrupted {kind:?} mutation {} cannot coexist with an operation that permits only verified disjoint create journals",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    // Provenance: scoped create completion from append-only episodes.
    let completed = scoped_completed(cfg, path, &raw)?;
    let owner = crate::status_state::identity_from_value(raw.get("owner").unwrap_or(&Value::Null));
    let coordinator = crate::status_state::identity_from_value(
        raw.get("coordinator_lease").unwrap_or(&Value::Null),
    );
    let (coordinator_state, coordinator_detail) = process_state(coordinator.as_ref());
    // Owner-less create (#885): no owner generation was recorded, so the
    // coordinator lease decides, mirroring `_create_journal_owner_state`.
    let (owner_state, owner_detail) = if owner.is_none() {
        (
            coordinator_state.clone(),
            "no owner process generation is recorded (owner-less create; the coordinator lease decides)".to_string(),
        )
    } else {
        process_state(owner.as_ref())
    };
    if !completed {
        if owner_state == "indeterminate" || coordinator_state == "indeterminate" {
            return Err(format!(
                "create journal for slot {:?} process state is indeterminate: owner={owner_state} ({owner_detail}); coordinator={coordinator_state} ({coordinator_detail})",
                raw.get("slot").and_then(Value::as_str).unwrap_or_default()
            ));
        }
        if owner.is_some()
            && (owner_state != coordinator_state
                || !matches!(owner_state.as_str(), "live" | "dead"))
        {
            return Err(format!(
                "create journal for slot {:?} has disagreeing process state: owner={owner_state} ({owner_detail}); coordinator={coordinator_state} ({coordinator_detail})",
                raw.get("slot").and_then(Value::as_str).unwrap_or_default()
            ));
        }
    }
    let machine = machine_s;
    let slot = field(&Value::Object(raw.clone()), "slot");
    let agent = validate_name(&field(&Value::Object(raw.clone()), "agent"), "agent")?.to_string();
    let slot_type = raw
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    if !matches!(slot_type.as_str(), "agent" | "validate") {
        return Err("create journal has an invalid slot type".to_string());
    }
    if !completed
        && states
            .iter()
            .any(|(_, slots)| slots.iter().any(|r| field(r, "slot") == slot))
    {
        return Err(format!(
            "create journal for slot {slot:?} has already reached ACTIVE publication"
        ));
    }
    if archives
        .iter()
        .any(|records| records.iter().any(|r| field(r, "slot") == slot))
    {
        return Err(format!(
            "create journal for slot {slot:?} conflicts with an archive"
        ));
    }
    let planned = as_list(
        raw.get("planned").unwrap_or(&Value::Null),
        "create journal.planned",
    )?
    .clone();
    let created = as_list(
        raw.get("created").unwrap_or(&Value::Null),
        "create journal.created",
    )?
    .clone();
    if planned.is_empty() || {
        let mut names: Vec<String> = planned.iter().map(|p| field(p, "name")).collect();
        names.sort();
        names.dedup();
        names.len() != planned.len()
    } {
        return Err("create journal has an empty or duplicate checkout plan".to_string());
    }
    {
        let mut names: Vec<String> = created.iter().map(|p| field(p, "name")).collect();
        let planned_names: Vec<String> = planned.iter().map(|p| field(p, "name")).collect();
        if names.len() != created.len() && {
            names.sort();
            names.dedup();
            true
        } {
            return Err("create journal has duplicate completed checkouts".to_string());
        }
        let prefix: Vec<String> = planned_names.iter().take(created.len()).cloned().collect();
        let created_names: Vec<String> = created.iter().map(|p| field(p, "name")).collect();
        if created_names != prefix {
            return Err(
                "create journal's completed checkouts are not a planned prefix".to_string(),
            );
        }
    }
    let hooks: Vec<String> = crate::config_validate::post_provision_hooks_for(config, &slot_type);
    let journal_hooks: Vec<String> = raw
        .get("post_provision_hooks")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if journal_hooks != hooks {
        return Err("create journal post-provision hooks differ from configuration".to_string());
    }
    if raw.get("failure_policy").and_then(Value::as_str) != Some("leave-for-inspection") {
        return Err("create journal has an unsupported hook failure policy".to_string());
    }
    let progress = as_int(
        raw.get("hook_progress").unwrap_or(&Value::Null),
        "create journal.hook_progress",
        0,
    )?;
    if progress > (created.len() * hooks.len()) as i64 {
        return Err("create journal ran hooks for an unrecorded checkout".to_string());
    }
    let slot_path = slot_directory(cfg, &slot, &slot_type)?;
    let mut observed: Vec<Value> = Vec::new();
    let mut dirty: Vec<String> = Vec::new();
    let mut expected_entries: BTreeSet<String> = BTreeSet::new();
    for item in &planned {
        let name = field(item, "name");
        let (expected_path, destination) = expected_checkout(cfg, &slot, &name, &slot_type)?;
        if field(item, "destination") != expected_path {
            return Err(format!(
                "create journal destination for {name} does not match slot {slot}"
            ));
        }
        let (_rel, repository) =
            crate::finish_write::stored_repository_path(cfg, &field(item, "repository"))?;
        if field(item, "landed_ref")
            != crate::active_state::landed_ref_for_remote(cfg, &field(item, "remote"))?
        {
            return Err(format!(
                "create journal target for {name} differs from configured authority"
            ));
        }
        // Remote digest census.
        let digest = {
            use sha2::{Digest as _, Sha256};
            let url = git_text(&repository, &["remote", "get-url", &field(item, "remote")])
                .unwrap_or_default();
            format!("{:x}", Sha256::digest(url.trim().as_bytes()))
        };
        if digest != field(item, "remote_url_sha256") {
            return Err(format!("create journal remote changed for {name}"));
        }
        let recorded = created.iter().find(|c| field(c, "name") == name);
        let Some(recorded) = recorded else {
            if destination.exists() || destination.symlink_metadata().is_ok() {
                return Err(format!(
                    "create journal has an unrecorded path at {}",
                    destination.display()
                ));
            }
            if listed_worktrees(&repository)?.contains(&destination) {
                return Err(format!(
                    "create journal has an unrecorded Git worktree at {}",
                    destination.display()
                ));
            }
            if branch_exists(&repository, &field(item, "branch"))? {
                return Err(format!(
                    "create journal has an unrecorded branch {:?}",
                    field(item, "branch")
                ));
            }
            continue;
        };
        let expected = (
            field(item, "destination"),
            field(item, "repository"),
            field(item, "branch"),
            field(item, "start_point"),
            field(item, "remote"),
            field(item, "remote_url_sha256"),
            field(item, "landed_ref"),
            field(item, "start_point"),
        );
        let actual = (
            field(recorded, "path"),
            field(recorded, "repository"),
            field(recorded, "branch"),
            field(recorded, "start_point"),
            field(recorded, "remote"),
            field(recorded, "remote_url_sha256"),
            field(recorded, "landed_ref"),
            field(recorded, "head"),
        );
        if actual != expected {
            return Err(format!(
                "create journal's completed checkout {name} differs from its plan"
            ));
        }
        if destination
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
            || !destination.is_dir()
        {
            return Err(format!(
                "create journal checkout is missing or unsafe: {}",
                destination.display()
            ));
        }
        let observed_head =
            crate::finish_write::verify_existing_worktree(&repository, &destination)?;
        let observed_branch = git_text(
            &destination,
            &["symbolic-ref", "--quiet", "--short", "HEAD"],
        )
        .map_err(|_| {
            format!(
                "checkout must be on its registered branch, not detached: {}",
                destination.display()
            )
        })?;
        if observed_branch.trim() != field(recorded, "branch") {
            return Err(format!(
                "create journal checkout {name} changed its registered branch"
            ));
        }
        let mut entry = recorded.clone();
        if let Some(obj) = entry.as_object_mut() {
            obj.insert("head".to_string(), Value::String(observed_head));
            obj.insert("containing_remote_refs".to_string(), json!([]));
        }
        observed.push(entry);
        // Dirtiness with the configuration's cache-glob exclusions.
        let globs = crate::finish_write::cache_globs_for(config, &name);
        let mut args: Vec<String> = vec![
            "status".into(),
            "--porcelain=v2".into(),
            "--untracked-files=all".into(),
            "--ignored=matching".into(),
        ];
        args.push(".".into());
        for pattern in &globs {
            args.push(format!(":(exclude,glob){pattern}"));
            args.push(format!(":(exclude,glob){pattern}/**"));
        }
        let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let status = git_text(&destination, &arg_refs)?;
        if !status.trim().is_empty() {
            dirty.push(name.clone());
        }
        expected_entries.insert(name);
    }
    if slot_path.exists() || slot_path.symlink_metadata().is_ok() {
        if slot_path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(true)
            || !slot_path.is_dir()
        {
            return Err(format!(
                "create journal slot path is unsafe: {}",
                slot_path.display()
            ));
        }
        if cfg.layout == "nested" {
            let entries: BTreeSet<String> = std::fs::read_dir(&slot_path)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            if entries != expected_entries {
                return Err(format!(
                    "create journal slot {slot:?} contains unrecorded paths"
                ));
            }
        }
    } else if !created.is_empty() {
        return Err(format!(
            "create journal has checkouts below missing slot {slot:?}"
        ));
    }
    for (state_machine, slots) in states {
        for record in slots {
            if completed && *state_machine == machine && field(record, "slot") == slot {
                continue;
            }
            let active_path = slot_directory(
                cfg,
                &field(record, "slot"),
                {
                    let t = field(record, "slot_type");
                    if t.is_empty() {
                        "agent".to_string()
                    } else {
                        t
                    }
                }
                .as_str(),
            )?;
            if within(&slot_path, &active_path) || within(&active_path, &slot_path) {
                return Err(format!(
                    "create journal slot {slot:?} overlaps active slot {state_machine}/{}",
                    field(record, "slot")
                ));
            }
        }
    }
    // Stable re-read: file must not have changed while classified.
    let (rebound, rebound_identity) = read_identity(path)?;
    if rebound_identity != identity || strict_obj(&rebound)? != raw {
        return Err(format!(
            "create journal changed while it was classified: {}",
            path.display()
        ));
    }
    let state = if completed {
        "completed-create".to_string()
    } else {
        format!("{owner_state}-incomplete-create")
    };
    Ok(Some(Row {
        machine,
        slot,
        agent,
        slot_type,
        slot_path,
        planned,
        created,
        observed,
        dirty,
        owner_detail,
        owner_state,
        coordinator_detail,
        coordinator_state,
        state,
        journal: path.to_path_buf(),
        identity,
    }))
}

/// Classify outstanding create journals for the requested slot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_create_journals(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: Option<&str>,
    slot_type: &str,
    agent: Option<&str>,
    format_json: bool,
    all_machines: bool,
    all_kinds: bool,
) -> std::result::Result<String, ClassifyError> {
    if all_kinds {
        return classify_all_journals(root, machine_override, format_json, all_machines);
    }
    let (Some(slot), Some(agent)) = (slot, agent) else {
        return Err(ClassifyError::Other(
            "classify-create-journals requires SLOT and --agent (or --all-kinds to list every lifecycle journal)".to_string(),
        ));
    };
    // Pre-scan (#992): a non-create journal with a live owner is a
    // retryable busy result, not an interrupted refusal. Shape-invalid
    // journals are left for the inner classification to report.
    if let Ok((cfg, _selected)) = load_cfg(root, machine_override) {
        let machines: Option<BTreeSet<String>> = if all_machines {
            None
        } else {
            Some(BTreeSet::from([cfg.machine.clone()]))
        };
        if let Ok(paths) = outstanding(&cfg.control, machines.as_ref()) {
            for path in &paths {
                let Ok((bytes, _identity)) = read_identity(path) else {
                    continue;
                };
                let Ok(raw) = strict_obj(&bytes) else {
                    continue;
                };
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                if validate_journal_shape(&cfg, &filename, &raw).is_err() {
                    continue;
                }
                let kind = raw
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // A standalone journal mid-write by a live operation
                // is busy (or a hard provenance refusal), never an
                // interrupted classification (#1033).
                let machine = raw
                    .get("machine")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if let Some(mismatch) = standalone_mismatch(&cfg, path, &raw, machine) {
                    if let StandaloneMismatch::Busy(pid) = mismatch {
                        return Err(ClassifyError::Busy {
                            kind: kind.clone(),
                            pid,
                            journal: filename.clone(),
                        });
                    }
                    return Err(ClassifyError::Other(journal_refusal(
                        path,
                        &mismatch.message(&raw, path),
                    )));
                }
                if kind == "create"
                    || ownerless_cargo_home_journal_is_unrelated(&cfg, &raw, slot, slot_type)
                {
                    continue;
                }
                if let Some(owner) = live_journal_owner(&raw) {
                    return Err(ClassifyError::Busy {
                        kind,
                        pid: owner.pid,
                        journal: filename,
                    });
                }
            }
        }
    }
    classify_create_journals_inner(
        root,
        machine_override,
        slot,
        slot_type,
        agent,
        format_json,
        all_machines,
    )
}

fn classify_create_journals_inner(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    slot_type: &str,
    agent: &str,
    format_json: bool,
    all_machines: bool,
) -> std::result::Result<String, ClassifyError> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    if !matches!(slot_type, "agent" | "validate") {
        return Err(ClassifyError::Other(
            "slot type must be one of agent, validate".to_string(),
        ));
    }
    let slot = validate_name(slot, "slot")?.to_string();
    let agent = validate_name(agent, "agent")?.to_string();
    refuse_partial_state_allow_seals(&cfg.control)?;
    // Seal gate (c4c4c64a): a non-orphaned seal refuses, with the
    // structured reasons on stdout in JSON mode.
    let reasons = seal_blocking_reasons(&cfg);
    if !reasons.is_empty() {
        let first = reasons[0]
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let payload = if format_json {
            let body = json!({
                "blocking": true,
                "blocking_reasons": reasons,
                "journals": [],
                "schema": 2,
            });
            Some(format!(
                "{}\n",
                crate::batch_text::py_compact_pub(&sort_keys(&body))
            ))
        } else {
            None
        };
        return Err(ClassifyError::Blocked { first, payload });
    }
    crate::hold_write::validate_global(root, &cfg)?;
    // Load every machine's ACTIVE/ARCHIVED documents.
    let mut states: Vec<(String, Vec<Value>)> = Vec::new();
    let mut archives: Vec<Vec<Value>> = Vec::new();
    for machine in crate::global_state::discover_machines(&cfg.control)? {
        let active_path = cfg.control.join(format!("ACTIVE.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&active_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                states.push((
                    machine.clone(),
                    doc.get("slots")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                ));
            }
        }
        let archive_path = cfg.control.join(format!("ARCHIVED.{machine}.json"));
        if let Ok(text) = std::fs::read_to_string(&archive_path) {
            if let Ok(doc) = serde_json::from_str::<Value>(&text) {
                archives.push(
                    doc.get("records")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
            }
        }
    }
    // #873: by default only the local machine's journals are classified;
    // a foreign shard is noted, never allowed to block a local verdict.
    let foreign_notes: Vec<Value> = if all_machines {
        Vec::new()
    } else {
        crate::global_state::foreign_shard_notes(&cfg.control, &cfg.machine)?
    };
    let machines: Option<BTreeSet<String>> = if all_machines {
        None
    } else {
        Some(BTreeSet::from([cfg.machine.clone()]))
    };
    let paths = outstanding(&cfg.control, machines.as_ref())?;
    let mut rows: Vec<Row> = Vec::new();
    for path in &paths {
        // Only create journals are classified; other kinds refuse in
        // classify_one, except a validated ownerless cargo-home removal
        // disjoint from the requested slot, which is skipped.  A busy
        // result stays bare and retryable; every other per-journal
        // refusal names its journal as a host fault (#1086).
        let row = classify_one(&cfg, &config, path, &states, &archives, &slot, slot_type).map_err(
            |error| {
                if error.starts_with("busy:") {
                    error
                } else {
                    journal_refusal(path, &error)
                }
            },
        )?;
        if let Some(row) = row {
            rows.push(row);
        }
    }
    {
        let mut seen_slots: BTreeSet<(String, String)> = BTreeSet::new();
        let mut seen_agents: BTreeSet<String> = BTreeSet::new();
        for row in &rows {
            if !seen_slots.insert((row.machine.clone(), row.slot.clone())) {
                return Err(ClassifyError::Other(journal_refusal(
                    &row.journal,
                    &format!(
                        "multiple interrupted creates claim {}/{}",
                        row.machine, row.slot
                    ),
                )));
            }
            if row.slot_type == "agent" && !seen_agents.insert(row.agent.clone()) {
                return Err(ClassifyError::Other(journal_refusal(
                    &row.journal,
                    &format!("multiple interrupted creates claim agent {:?}", row.agent),
                )));
            }
        }
    }
    let target = slot_directory(&cfg, &slot, slot_type)?;
    for row in &rows {
        if row.slot == slot {
            return Err(ClassifyError::Other(format!(
                "interrupted create already targets slot {slot:?}; recover that exact slot"
            )));
        }
        if slot_type == "agent" && row.slot_type == "agent" && row.agent == agent {
            return Err(ClassifyError::Other(format!(
                "interrupted create already belongs to agent {agent:?}; recover it first"
            )));
        }
        if within(&target, &row.slot_path) || within(&row.slot_path, &target) {
            return Err(ClassifyError::Other(format!(
                "requested slot path {} overlaps interrupted create {}/{}",
                target.display(),
                row.machine,
                row.slot
            )));
        }
    }
    let journals: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "agent": row.agent,
                "checkouts": row.observed.iter().map(|c| json!({
                    "branch": field(c, "branch"),
                    "dirty": row.dirty.contains(&field(c, "name")),
                    "head": field(c, "head"),
                    "name": field(c, "name"),
                    "path": field(c, "path"),
                })).collect::<Vec<_>>(),
                "coordinator": row.coordinator_detail,
                "coordinator_state": row.coordinator_state,
                "created": row.created.len(),
                "dirty_checkouts": row.dirty,
                "journal": row.journal.to_string_lossy(),
                "journal_identity": {
                    "device": row.identity.0,
                    "inode": row.identity.1,
                    "sha256": row.identity.3,
                    "size": row.identity.2,
                },
                "machine": row.machine,
                "owner": row.owner_detail,
                "owner_state": row.owner_state,
                "planned": row.planned.len(),
                "registry_state": if row.state == "completed-create" { "exact-active" } else { "absent" },
                "slot": row.slot,
                "slot_path": row.slot_path.to_string_lossy(),
                "slot_type": row.slot_type,
                "state": row.state,
            })
        })
        .collect();
    if format_json {
        let payload = json!({
            "blocking": false,
            "foreign_shards": foreign_notes,
            "journals": journals,
            "requested": {
                "agent": agent,
                "slot": slot,
                "slot_path": target.to_string_lossy(),
                "slot_type": slot_type,
            },
            "schema": 2,
        });
        return Ok(format!(
            "{}\n",
            crate::batch_text::py_compact_pub(&sort_keys(&payload))
        ));
    }
    let live = rows
        .iter()
        .filter(|r| r.state == "live-incomplete-create")
        .count();
    let completed = rows
        .iter()
        .filter(|r| r.state == "completed-create")
        .count();
    let dead = rows
        .iter()
        .filter(|r| r.state == "dead-incomplete-create")
        .count();
    let mut out = format!(
        "NONBLOCKING: journals={} live-incomplete-create={live} completed-create={completed} dead-incomplete-create={dead}; all are unrelated to requested {slot_type} slot {slot}\n",
        rows.len()
    );
    for row in &rows {
        let dirty = if row.dirty.is_empty() {
            "none".to_string()
        } else {
            row.dirty.join(",")
        };
        out.push_str(&format!(
            "JOURNAL: state={} machine={} slot={} agent={} dirty_checkouts={} journal={}\n",
            row.state,
            row.machine,
            row.slot,
            row.agent,
            dirty,
            row.journal.display()
        ));
    }
    out.push_str(&crate::global_state::foreign_shard_note_lines(
        &foreign_notes,
    ));
    Ok(out)
}
