//! Reader: audit census (`parse-audit`).
//!
//! Reproduces the Python edition's read-only audit payload for
//! agent rows: per-record verdicts (HELD / DELETABLE / RECOVERABLE /
//! BLOCKED / UNKNOWN) with the oracle's reasons, unregistered
//! on-disk directory rows, interrupted-journal rows, validation-batch
//! seal evidence, stray Git registrations, cache byte accounting,
//! and the disk/gate summary. Validation rows are judged by
//! their run authority, including stable retained run handles.
//! Unregistered agent slots plan their caches from the Git
//! checkouts they contain, and every subject plans under a fair
//! share of the planning work and wall allowances. Resume-state
//! reuse, journal-slot cache planning, and unreadable host
//! evidence stay with the Python edition for now and refuse by
//! name. Read-only.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{
    as_int, as_list, as_mapping, as_str, exact_keys, load_cfg, parse_timestamp, slot_directory, Cfg,
};
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, refuse_symlink, relative_inside, validate_name};
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::finish_write::{
    assert_cache_policy_untracked, cache_globs_for, fnmatch_component, glob_parts, stored_path,
    stored_repository_path, verify_existing_worktree,
};
use crate::global_state::{discover_machines, load_event_chain};
use crate::handoff_state::{
    load_sidecar_artifact, read_regular_file_identity, sidecar_path, strict_json_object,
};
use crate::handoff_write::publication;
use crate::hold_write::{active_document, current_hold, validate_global};
use crate::remove_write::{assert_slot_unused, liveness_answer};
use crate::status_state::process_state;

type Result<T> = std::result::Result<T, String>;

struct JournalSlot {
    slot: String,
    machine: String,
    state: String,
}

fn journal_slots(cfg: &Cfg) -> Result<BTreeMap<String, JournalSlot>> {
    let mut found: BTreeMap<String, JournalSlot> = BTreeMap::new();
    let entries = std::fs::read_dir(&cfg.control).map_err(|error| {
        format!(
            "cannot list control directory {}: {error}",
            cfg.control.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot list control directory: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let machine = ["ACTIVE.", "CREATE.", "FINISH."].iter().find_map(|prefix| {
            name.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(".journal"))
        });
        let Some(machine) = machine else { continue };
        if machine.is_empty() {
            continue;
        }
        let text = std::fs::read_to_string(entry.path())
            .map_err(|error| format!("cannot read recovery journal {name}: {error}"))?;
        let raw: Value = serde_json::from_str(&text)
            .map_err(|error| format!("recovery journal {name} is malformed JSON: {error}"))?;
        let kind = raw.get("kind").and_then(Value::as_str).unwrap_or_default();
        if !matches!(kind, "create" | "finish" | "import-existing") {
            continue;
        }
        let slot = raw
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        validate_name(&slot, "slot")?;
        if let Some(previous) = found.get(&slot) {
            return Err(format!(
                "slot '{slot}' has recovery journals on both {} and {machine}",
                previous.machine
            ));
        }
        found.insert(
            slot.clone(),
            JournalSlot {
                slot,
                machine: machine.to_string(),
                state: format!("{kind}-journal"),
            },
        );
    }
    Ok(found)
}

/// The latest owner release for one generation and why it is void,
/// mirroring `_owner_release_state` / `_owner_release_void_reason`.
fn release_state(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
) -> Result<(Option<Value>, Option<String>)> {
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        return Ok((None, None));
    }
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        {
            latest = Some(event.clone());
        }
    }
    let Some(event) = latest else {
        return Ok((None, None));
    };
    let payload = event.get("payload").cloned().unwrap_or(Value::Null);
    let handoff = payload.get("handoff").cloned().unwrap_or(Value::Null);
    let release = json!({
        "sequence": event.get("sequence").cloned().unwrap_or(Value::Null),
        "recorded_at": event.get("recorded_at").cloned().unwrap_or(Value::Null),
        "heartbeat_at": payload.get("heartbeat_at").cloned().unwrap_or(Value::Null),
        "owner": payload.get("owner").cloned().unwrap_or(Value::Null),
        "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
        "handoff_path": handoff.get("path").cloned().unwrap_or(Value::Null),
        "sha256": handoff.get("sha256").cloned().unwrap_or(Value::Null),
        "intent_sequence": handoff.get("intent_sequence").cloned().unwrap_or(Value::Null),
    });
    let void = release_void_reason(cfg, record, events, &payload, &handoff)?;
    Ok((Some(release), void))
}

fn release_void_reason(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
    payload: &Value,
    handoff: &Value,
) -> Result<Option<String>> {
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let release_agent = payload
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if release_agent != record_agent {
        return Ok(Some(format!(
            "the slot now belongs to agent {record_agent}, not {release_agent}"
        )));
    }
    if payload.get("owner") != record.get("owner") {
        return Ok(Some(
            "the recorded owner process generation changed after the release".to_string(),
        ));
    }
    let released_heartbeat = payload
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_heartbeat = record
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if released_heartbeat != current_heartbeat {
        return Ok(Some(format!(
            "the heartbeat was renewed after the release (released at heartbeat {released_heartbeat}, now {current_heartbeat})"
        )));
    }
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let intent_sequence = handoff.get("intent_sequence").and_then(Value::as_i64);
    let published = publication(events, slot, generation);
    let publication_ok = match &published {
        Some((sequence, _artifact, completed)) => Some(*sequence) == intent_sequence && *completed,
        None => false,
    };
    if !publication_ok {
        return Ok(Some(
            "the handoff publication changed after the release".to_string(),
        ));
    }
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let artifact = match load_sidecar_artifact(cfg, machine, slot, slot_type, generation) {
        Ok(value) => value,
        Err(error) => {
            return Ok(Some(format!(
                "the released handoff sidecar cannot be read: {error}"
            )));
        }
    };
    let Some(artifact) = artifact else {
        return Ok(Some("the released handoff sidecar is missing".to_string()));
    };
    let expected_path = sidecar_path(&cfg.control, machine, slot, generation);
    let published_artifact = published
        .as_ref()
        .map(|(_sequence, artifact, _done)| artifact);
    if artifact.value.get("source").and_then(Value::as_str) != Some("write-handoff")
        || handoff.get("path").and_then(Value::as_str)
            != Some(expected_path.to_string_lossy().as_ref())
        || artifact.value.get("sha256") != handoff.get("sha256")
        || published_artifact != Some(&artifact.value)
    {
        return Ok(Some(
            "the handoff sidecar no longer matches the released bytes".to_string(),
        ));
    }
    Ok(None)
}

fn release_obj(release: &Value, void_reason: Option<&str>) -> Value {
    let recorded_at = release
        .get("recorded_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    json!({
        "state": if void_reason.is_none() { "released" } else { "void" },
        "summary": match void_reason {
            None => format!("RELEASED by live owner at {recorded_at}"),
            Some(reason) => format!("release at {recorded_at} no longer applies: {reason}"),
        },
        "event_sequence": release.get("sequence").cloned().unwrap_or(Value::Null),
        "recorded_at": release.get("recorded_at").cloned().unwrap_or(Value::Null),
        "heartbeat_at": release.get("heartbeat_at").cloned().unwrap_or(Value::Null),
        "owner": release.get("owner").cloned().unwrap_or(Value::Null),
        "handoff_sha256": release.get("sha256").cloned().unwrap_or(Value::Null),
        "void_reason": void_reason,
    })
}

/// Latest handoff-read payload for one generation (audit's read gate).
fn latest_handoff_read(events: &[Value], record: &Value) -> Option<Value> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let mut latest: Option<Value> = None;
    for event in events {
        if event.get("kind").and_then(Value::as_str) != Some("handoff-read") {
            continue;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        if payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
        {
            latest = Some(payload);
        }
    }
    latest
}

/// The oracle's `_assert_handoff_read` as an audit reason producer.
fn handoff_read_reason(
    cfg: &Cfg,
    record: &Value,
    events: &[Value],
    slot_path: &Path,
) -> Option<String> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let sidecar = load_sidecar_artifact(cfg, machine, slot, slot_type, generation);
    let latest = latest_handoff_read(events, record);
    match sidecar {
        Err(error) => Some(error),
        Ok(None) => {
            let legacy = slot_path.join("HANDOFF.md");
            if legacy.exists() || legacy.is_symlink() {
                // Legacy artifacts need the file-identity projection the
                // finish/remove slices still leave to Python for
                // quarantine; audit treats an unread legacy file like
                // the oracle's unread refusal below.
                let read_matches = latest.as_ref().is_some_and(|read| {
                    read.get("source").and_then(Value::as_str) == Some("legacy-slot-file")
                });
                if !read_matches {
                    return Some(format!(
                        "slot {slot} contains an unread HANDOFF.md or handoff sidecar. state: REFUSED -- no checkout was salvaged or removed. remedy: run 'wrkslots read-handoff {slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' and read its output, then retry remove"
                    ));
                }
                return None;
            }
            if latest.is_some() {
                return Some(format!(
                    "slot {slot} handoff artifact disappeared after it was read. state: REFUSED -- no checkout was salvaged or removed. remedy: restore the exact generation-bound sidecar or legacy HANDOFF.md"
                ));
            }
            None
        }
        Ok(Some(artifact)) => {
            let matches = latest.as_ref().is_some_and(|read| {
                read.get("sha256") == artifact.value.get("sha256")
                    && read.get("source") == artifact.value.get("source")
                    && read.get("source_path") == artifact.value.get("source_path")
                    && read.get("source_identity") == artifact.value.get("source_identity")
            });
            if !matches {
                return Some(format!(
                    "slot {slot} contains an unread HANDOFF.md or handoff sidecar. state: REFUSED -- no checkout was salvaged or removed. remedy: run 'wrkslots read-handoff {slot} --coordinator-authorized --coordinator-pid <current-coordinator-pid>' and read its output, then retry remove"
                ));
            }
            None
        }
    }
}

/// One invocation's cache-census request, mirroring the Python
/// audit options.
#[derive(Default)]
pub struct CensusRequest {
    pub state: Option<String>,
    pub work_limit: i64,
    pub wall_seconds: f64,
}

type CensusSubject = (Vec<(PathBuf, PathBuf, u64)>, Option<String>, bool);

#[derive(Clone)]
struct CacheMeasurement {
    bytes: Option<i64>,
    status: &'static str,
    error: Option<String>,
}

impl CacheMeasurement {
    fn complete(bytes: i64) -> Self {
        Self {
            bytes: Some(bytes),
            status: "complete",
            error: None,
        }
    }
    fn partial() -> Self {
        Self {
            bytes: None,
            status: "partial",
            error: None,
        }
    }
    fn error(message: String) -> Self {
        Self {
            bytes: None,
            status: "error",
            error: Some(message),
        }
    }
}

/// The mount table planning judges candidates against, read
/// once per audit: the parsed mount points, or the read failure
/// the oracle reports when it cannot prove a candidate safe.
type PlanMounts = std::result::Result<Vec<(i64, PathBuf)>, String>;

fn read_plan_mounts() -> PlanMounts {
    let text =
        std::fs::read_to_string("/proc/self/mountinfo").map_err(|error| error.to_string())?;
    let mut mounts = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let Ok(id) = fields[0].parse::<i64>() else {
            continue;
        };
        mounts.push((id, PathBuf::from(audit_unescape_mount(fields[4]))));
    }
    Ok(mounts)
}

/// Judge one planned candidate exactly as the oracle's
/// `_cache_directories_for_path` judges every glob match: no
/// symlink component, no mount component, and no mount point
/// crossed or contained. Returns the refusal, if any.
fn judge_planned_candidate(
    checkout_path: &Path,
    candidate: &Path,
    mounts: &PlanMounts,
) -> Option<String> {
    let relative = candidate.strip_prefix(checkout_path).ok()?;
    let mut current = checkout_path.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        let is_link = current
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink())
            .unwrap_or(false);
        if is_link {
            return Some(format!(
                "cache path crosses a symlink: {}",
                current.display()
            ));
        }
    }
    let mounts = match mounts {
        Ok(mounts) => mounts,
        Err(error) => {
            return Some(format!(
                "cannot prove cache path contains no mount points because \
                 /proc/self/mountinfo is unreadable: {error}"
            ));
        }
    };
    let mut current = checkout_path.to_path_buf();
    for part in relative.components() {
        current.push(part.as_os_str());
        if mounts.iter().any(|(_, point)| point == &current) {
            return Some(format!(
                "cache path crosses a mount point: {}",
                current.display()
            ));
        }
    }
    for (_, point) in mounts {
        if point != checkout_path
            && point.starts_with(checkout_path)
            && (candidate.starts_with(point) || point.starts_with(candidate))
        {
            return Some(format!(
                "cache path crosses or contains a mount point: {}",
                point.display()
            ));
        }
    }
    None
}

/// Python's `{value:g}` rendering for the wall allowance:
/// six significant digits, fixed notation from 1e-4 up to (but
/// not including) 1e6, scientific outside, exponent signed and
/// at least two digits.
fn fmt_g(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    if !value.is_finite() {
        return format!("{value}");
    }
    let scientific = format!("{value:.5e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .unwrap_or((scientific.as_str(), "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa
        .chars()
        .filter(|character| *character != '.' && *character != '-')
        .collect::<String>()
        .trim_end_matches('0')
        .to_string();
    let digits = if digits.is_empty() {
        "0".to_string()
    } else {
        digits
    };
    let sign = if value < 0.0 { "-" } else { "" };
    if !(-4..6).contains(&exponent) {
        let fraction = if digits.len() > 1 {
            format!(".{}", &digits[1..])
        } else {
            String::new()
        };
        return format!("{sign}{}{fraction}e{exponent:+03}", &digits[..1]);
    }
    if exponent >= 0 {
        let split = (exponent + 1) as usize;
        let mut integer = digits.to_string();
        while integer.len() < split {
            integer.push('0');
        }
        let (integer, fraction) = integer.split_at(split);
        if fraction.is_empty() {
            return format!("{sign}{integer}");
        }
        return format!("{sign}{integer}.{fraction}");
    }
    format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
}

#[cfg(test)]
mod fmt_g_tests {
    use super::fmt_g;

    #[test]
    fn renders_like_python_g() {
        assert_eq!(fmt_g(60.0), "60");
        assert_eq!(fmt_g(5.0), "5");
        assert_eq!(fmt_g(2.5), "2.5");
        assert_eq!(fmt_g(100_000.0), "100000");
        assert_eq!(fmt_g(123_456.0), "123456");
        assert_eq!(fmt_g(999_999.9), "1e+06");
        assert_eq!(fmt_g(1_000_000.0), "1e+06");
        assert_eq!(fmt_g(1_234_567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.001), "0.001");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(0.00001), "1e-05");
        assert_eq!(fmt_g(1e-9), "1e-09");
        assert_eq!(fmt_g(0.0), "0");
    }
}

/// One audit subject's share of cache-glob planning, mirroring
/// the oracle's `_CachePlanBudget`: one work unit per directory
/// listing, a fair share of the remaining wall allowance, and
/// the oracle's two exhaustion spellings.
struct PlanBudget {
    work_limit: i64,
    work_consumed: i64,
    seconds: f64,
    wall_total: f64,
    spent: f64,
}

impl PlanBudget {
    fn check(&self, walking: f64) -> Result<()> {
        if self.spent + walking >= self.seconds {
            return Err(format!(
                "cache planning exhausted this subject's share of the fixed wall \
                 allowance of {} seconds; no cache total can be published within \
                 this allowance",
                fmt_g(self.wall_total)
            ));
        }
        if self.work_limit - self.work_consumed <= 0 {
            return Err(format!(
                "cache planning exhausted this subject's share of {} directory \
                 listings; no cache total can be published within this allowance",
                self.work_limit
            ));
        }
        Ok(())
    }

    fn debit(&mut self, walking: f64) -> Result<()> {
        self.check(walking)?;
        self.work_consumed += 1;
        Ok(())
    }
}

/// Expand the globs below one checkout exactly as the oracle's
/// `_expand_cache_globs_walk` does: a per-pattern position walk
/// where `**` matches the directory itself and every descendant
/// reached without following a symlink, a non-final component
/// follows symlinks, and each directory listing is debited.
/// Returns the matches per pattern, in the oracle's order.
fn expand_cache_globs(
    checkout_path: &Path,
    globs: &[String],
    budget: &mut PlanBudget,
) -> Result<Vec<Vec<PathBuf>>> {
    let started = std::time::Instant::now();
    let outcome = expand_cache_globs_walk(checkout_path, globs, budget, started);
    budget.spent += started.elapsed().as_secs_f64();
    outcome
}

/// One directory awaiting its listing in the glob walk: its
/// path, its reach sequence, and the pattern positions it was
/// reached with.
type WalkPending = (PathBuf, i64, std::collections::BTreeSet<(usize, usize)>);

fn expand_cache_globs_walk(
    checkout_path: &Path,
    globs: &[String],
    budget: &mut PlanBudget,
    started: std::time::Instant,
) -> Result<Vec<Vec<PathBuf>>> {
    use std::collections::BTreeSet;
    let parts: Vec<Vec<String>> = globs.iter().map(|item| glob_parts(item)).collect();
    fn closure(
        parts: &[Vec<String>],
        positions: &BTreeSet<(usize, usize)>,
    ) -> BTreeSet<(usize, usize)> {
        let mut closed = positions.clone();
        for &(index, mut position) in positions {
            while position < parts[index].len() && parts[index][position] == "**" {
                position += 1;
                closed.insert((index, position));
            }
        }
        closed
    }
    let mut matches: Vec<Vec<((i64, i64), PathBuf)>> = parts.iter().map(|_| Vec::new()).collect();
    let mut reached: i64 = 0;
    let initial: BTreeSet<(usize, usize)> = (0..parts.len()).map(|index| (index, 0)).collect();
    let mut pending: Vec<WalkPending> =
        vec![(checkout_path.to_path_buf(), 0, closure(&parts, &initial))];
    while let Some((directory, sequence, positions)) = pending.pop() {
        for &(index, position) in &positions {
            if position == parts[index].len() {
                matches[index].push(((sequence, -1), directory.clone()));
            }
        }
        if positions
            .iter()
            .all(|&(index, position)| position == parts[index].len())
        {
            continue;
        }
        budget.debit(started.elapsed().as_secs_f64())?;
        let Ok(listing) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries: Vec<std::fs::DirEntry> = Vec::new();
        for entry in listing {
            match entry {
                Ok(entry) => entries.push(entry),
                Err(error) => {
                    return Err(format!(
                        "cannot expand cache globs in {}: {error}",
                        checkout_path.display()
                    ));
                }
            }
        }
        let mut children: Vec<WalkPending> = Vec::new();
        for (entry_index, entry) in entries.iter().enumerate() {
            let child = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let mut advanced: BTreeSet<(usize, usize)> = BTreeSet::new();
            for &(index, position) in &positions {
                let pattern = &parts[index];
                if position == pattern.len() {
                    continue;
                }
                let part = &pattern[position];
                if part == "**" {
                    let descend = entry
                        .file_type()
                        .map(|kind| kind.is_dir() && !kind.is_symlink())
                        .unwrap_or(false);
                    if descend {
                        advanced.insert((index, position));
                    }
                } else if fnmatch_component(part, &name) {
                    if position + 1 == pattern.len() {
                        matches[index].push(((sequence, entry_index as i64), child.clone()));
                    } else if child.is_dir() {
                        advanced.insert((index, position + 1));
                    }
                }
            }
            if !advanced.is_empty() {
                reached += 1;
                children.push((child, reached, closure(&parts, &advanced)));
            }
        }
        pending.extend(children.into_iter().rev());
    }
    Ok(matches
        .into_iter()
        .map(|mut found| {
            found.sort_by_key(|(order, _)| *order);
            found.into_iter().map(|(_, path)| path).collect()
        })
        .collect())
}

/// Plan one checkout's caches: the oracle's
/// `_cache_directories_for_path` checks (allowance, presence,
/// tracked/submodule policy) and glob expansion, then selection
/// of the real directories it names, shallowest first, with a
/// candidate inside a selected one dropped.
/// The identity planning brackets a checkout with: device,
/// inode, and mount id, compared before and after the Git
/// verification so a swapped checkout refuses.
fn directory_identity(path: &Path, mounts: &PlanMounts) -> (u64, u64, Option<i64>) {
    use std::os::unix::fs::MetadataExt as _;
    let (device, inode) = std::fs::metadata(path)
        .map(|meta| (meta.dev(), meta.ino()))
        .unwrap_or((0, 0));
    let mount = mounts
        .as_ref()
        .ok()
        .and_then(|mounts| mount_id_of(path, mounts));
    (device, inode, mount)
}

fn plan_one_checkout(
    checkout_path: &Path,
    globs: &[String],
    name: &str,
    source_repository: Option<&Path>,
    budget: &mut PlanBudget,
    mounts: &PlanMounts,
) -> (Vec<PathBuf>, Option<String>) {
    if let Err(error) = budget.check(0.0) {
        return (Vec::new(), Some(error));
    }
    let unsafe_checkout = checkout_path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
        .unwrap_or(true);
    if unsafe_checkout {
        return (
            Vec::new(),
            Some(format!(
                "checkout is missing or unsafe for cache inspection: {}",
                checkout_path.display()
            )),
        );
    }
    let identity = directory_identity(checkout_path, mounts);
    let verified = match source_repository {
        Some(repository) => verify_existing_worktree(repository, checkout_path).map(|_| ()),
        None => repository_root(checkout_path).and_then(|root| {
            if root == checkout_path {
                Ok(())
            } else {
                Err(format!(
                    "cache target is not a Git checkout root: {}",
                    checkout_path.display()
                ))
            }
        }),
    };
    if let Err(error) = verified {
        return (Vec::new(), Some(error));
    }
    if directory_identity(checkout_path, mounts) != identity {
        return (
            Vec::new(),
            Some(format!(
                "checkout changed during cache inspection: {}",
                checkout_path.display()
            )),
        );
    }
    if let Err(error) = assert_cache_policy_untracked(checkout_path, name, globs) {
        return (Vec::new(), Some(error));
    }
    let expanded = match expand_cache_globs(checkout_path, globs, budget) {
        Ok(expanded) => expanded,
        Err(error) => return (Vec::new(), Some(error)),
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for (pattern, found) in globs.iter().zip(expanded.iter()) {
        for candidate in found {
            if candidate == checkout_path {
                return (
                    Vec::new(),
                    Some(format!(
                        "cache glob '{pattern}' escaped checkout {}",
                        checkout_path.display()
                    )),
                );
            }
            if let Some(error) = judge_planned_candidate(checkout_path, candidate, mounts) {
                return (Vec::new(), Some(error));
            }
            if candidate.is_dir() && !candidates.contains(candidate) {
                candidates.push(candidate.clone());
            }
        }
    }
    candidates.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    let mut selected: Vec<PathBuf> = Vec::new();
    for candidate in candidates {
        if selected
            .iter()
            .any(|parent| candidate != *parent && candidate.starts_with(parent))
        {
            continue;
        }
        selected.push(candidate);
    }
    (selected, None)
}

/// Overlapping globs must not count one directory twice: a
/// candidate inside an already-selected candidate is dropped.
fn dedup_cache_roots(mut roots: Vec<(PathBuf, PathBuf, u64)>) -> Vec<(PathBuf, PathBuf, u64)> {
    roots.sort_by(|a, b| a.0.cmp(&b.0));
    let mut deduped: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    for root in roots {
        if deduped
            .iter()
            .any(|(selected, _, _)| root.0 != *selected && root.0.starts_with(selected))
        {
            continue;
        }
        deduped.push(root);
    }
    deduped
}

/// Every cache directory the configured globs name inside one
/// record's checkouts: (root, checkout, checkout device).
fn cache_roots(
    cfg: &Cfg,
    config: &Value,
    record: &Value,
    budget: &mut PlanBudget,
    mounts: &PlanMounts,
) -> (Vec<(PathBuf, PathBuf, u64)>, Option<String>) {
    use std::os::unix::fs::MetadataExt as _;
    let checkouts = record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut roots: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    for checkout in &checkouts {
        let name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let globs = cache_globs_for(config, name);
        let raw_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let checkout_path = match stored_path(cfg, raw_path, "checkout path") {
            Ok(path) => path,
            Err(error) => return (Vec::new(), Some(error)),
        };
        let raw_repository = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let repository = match stored_repository_path(cfg, raw_repository) {
            Ok((_relative, repository)) => repository,
            Err(error) => return (Vec::new(), Some(error)),
        };
        let (candidates, error) = plan_one_checkout(
            &checkout_path,
            &globs,
            name,
            Some(&repository),
            budget,
            mounts,
        );
        if let Some(error) = error {
            return (Vec::new(), Some(error));
        }
        let checkout_device = std::fs::metadata(&checkout_path)
            .map(|meta| meta.dev())
            .unwrap_or(0);
        for candidate in candidates {
            roots.push((candidate, checkout_path.clone(), checkout_device));
        }
    }
    (dedup_cache_roots(roots), None)
}

/// The Git worktree root containing `path`, with the oracle's
/// runner and refusal spellings.
fn repository_root(path: &Path) -> Result<PathBuf> {
    let stdout = crate::git_storage_state::git_run(path, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(stdout.trim());
    let unsafe_root = root
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
        .unwrap_or(true);
    if unsafe_root {
        return Err(format!(
            "Git reported an unsafe repository root: {}",
            root.display()
        ));
    }
    Ok(root)
}

/// The checkout roots an unregistered slot contains, judged
/// exactly as the oracle's `_unregistered_cache_roots` judges
/// them: the slot itself under flat layout, otherwise every
/// entry, and each must be a Git checkout root.
fn unregistered_cache_roots(cfg: &Cfg, slot: &str) -> Result<Vec<PathBuf>> {
    let slot_path = cfg.worktrees.join(slot);
    let unsafe_slot = slot_path
        .symlink_metadata()
        .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
        .unwrap_or(true);
    if unsafe_slot {
        return Err(format!(
            "unregistered slot is missing or unsafe: {}",
            slot_path.display()
        ));
    }
    let candidates: Vec<PathBuf> = if cfg.layout == "flat" {
        vec![slot_path.clone()]
    } else {
        let listing = std::fs::read_dir(&slot_path)
            .map_err(|error| format!("cannot list unregistered slot {slot}: {error}"))?;
        let mut paths: Vec<PathBuf> = Vec::new();
        for entry in listing {
            let entry =
                entry.map_err(|error| format!("cannot list unregistered slot {slot}: {error}"))?;
            paths.push(entry.path());
        }
        paths
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    for candidate in &candidates {
        let unsafe_entry = candidate
            .symlink_metadata()
            .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
            .unwrap_or(true);
        if unsafe_entry {
            return Err(format!(
                "unregistered slot {slot} contains a non-checkout entry: {}",
                candidate.display()
            ));
        }
        if repository_root(candidate)? != *candidate {
            return Err(format!(
                "unregistered slot {slot} entry is not a Git checkout root: {}",
                candidate.display()
            ));
        }
        roots.push(candidate.clone());
    }
    if roots.is_empty() {
        return Err(format!(
            "unregistered slot {slot} contains no Git checkouts"
        ));
    }
    Ok(roots)
}

/// One unregistered agent slot's census subject: its caches are
/// planned from the checkouts it contains under the global
/// `cache_globs` policy alone, because no registered repository
/// name can be trusted for it.
fn unregistered_cache_subject(
    cfg: &Cfg,
    config: &Value,
    slot: &str,
    budget: &mut PlanBudget,
    mounts: &PlanMounts,
) -> CensusSubject {
    use std::os::unix::fs::MetadataExt as _;
    if let Err(error) = budget.check(0.0) {
        return (Vec::new(), Some(error), false);
    }
    let roots = match unregistered_cache_roots(cfg, slot) {
        Ok(roots) => roots,
        Err(error) => return (Vec::new(), Some(error), false),
    };
    let global_globs: Vec<String> = config
        .get("cache_globs")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let repo_globs_configured = config
        .get("repo_cache_globs")
        .and_then(Value::as_array)
        .is_some_and(|groups| !groups.is_empty());
    if repo_globs_configured && global_globs.is_empty() {
        return (
            Vec::new(),
            Some(format!(
                "unregistered slot {slot} has no trusted repository-name mapping; \
                 import it or configure a global cache_globs policy before cleanup"
            )),
            false,
        );
    }
    let mut planned: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        let label = format!("unregistered-{}", index + 1);
        let (candidates, error) =
            plan_one_checkout(root, &global_globs, &label, None, budget, mounts);
        if let Some(error) = error {
            return (Vec::new(), Some(error), false);
        }
        let device = std::fs::metadata(root).map(|meta| meta.dev()).unwrap_or(0);
        for candidate in candidates {
            planned.push((candidate, root.clone(), device));
        }
    }
    (dedup_cache_roots(planned), None, false)
}

/// Mount identity of one path from /proc/self/mountinfo: the id of
/// the longest mount point covering it.
fn mount_id_of(path: &Path, mounts: &[(i64, PathBuf)]) -> Option<i64> {
    let mut best: Option<(usize, i64)> = None;
    for (id, point) in mounts {
        if path.starts_with(point) {
            let depth = point.components().count();
            if best.is_none_or(|(current, _)| depth >= current) {
                best = Some((depth, *id));
            }
        }
    }
    best.map(|(_, id)| id)
}

fn census_mounts() -> Result<Vec<(i64, PathBuf)>> {
    let text = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("cannot read /proc/self/mountinfo: {error}"))?;
    let mut mounts = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let Ok(id) = fields[0].parse::<i64>() else {
            continue;
        };
        mounts.push((id, PathBuf::from(audit_unescape_mount(fields[4]))));
    }
    Ok(mounts)
}

type CacheIdentity = (u64, u64, i64, i64);

/// One bounded walk over a cache root: allocated bytes plus the
/// identity of every directory and entry seen. Work is debited
/// per directory opened and per entry from the shared invocation
/// budget; budget exhaustion leaves the root partial, while wall
/// expiry is the oracle's traversal-timeout error.
fn walk_cache_root(
    root: &Path,
    device: u64,
    mounts: &[(i64, PathBuf)],
    budget: &mut i64,
    deadline: std::time::Instant,
    validation_complete: bool,
) -> std::result::Result<(i64, BTreeMap<PathBuf, CacheIdentity>), CacheMeasurement> {
    use std::os::unix::fs::MetadataExt as _;
    let timeout = || {
        CacheMeasurement::error(
            "cache census traversal exhausted the fixed wall allowance".to_string(),
        )
    };
    let root_mount = mount_id_of(root, mounts);
    let mut total: i64 = 0;
    let mut seen: BTreeMap<PathBuf, CacheIdentity> = BTreeMap::new();
    let mut pending: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        if std::time::Instant::now() >= deadline {
            return Err(timeout());
        }
        if *budget <= 0 {
            return Err(CacheMeasurement::partial());
        }
        *budget -= 1;
        let meta = std::fs::symlink_metadata(&directory).map_err(|error| {
            CacheMeasurement::error(format!(
                "cannot inspect cache entry {}: {error}",
                directory.display()
            ))
        })?;
        if meta.dev() != device {
            return Err(CacheMeasurement::error(format!(
                "cache path crossed onto another filesystem: {}",
                directory.display()
            )));
        }
        if mount_id_of(&directory, mounts) != root_mount {
            return Err(CacheMeasurement::error(format!(
                "cache path crossed a mount point: {}",
                directory.display()
            )));
        }
        total += (meta.blocks() * 512) as i64;
        seen.insert(
            directory.clone(),
            (
                meta.dev(),
                meta.ino(),
                meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
                meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
            ),
        );
        let entries = std::fs::read_dir(&directory).map_err(|error| {
            CacheMeasurement::error(format!(
                "cannot inspect cache entry {}: {error}",
                directory.display()
            ))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                CacheMeasurement::error(format!("cannot inspect cache entry: {error}"))
            })?;
            if std::time::Instant::now() >= deadline {
                return Err(timeout());
            }
            if *budget <= 0 {
                return Err(CacheMeasurement::partial());
            }
            *budget -= 1;
            let path = entry.path();
            if entry.file_name() == ".git" && !validation_complete {
                return Err(CacheMeasurement::error(format!(
                    "cache directory contains nested Git metadata: {}",
                    path.display()
                )));
            }
            let child = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(CacheMeasurement::error(format!(
                        "cannot inspect cache entry {}: {error}",
                        path.display()
                    )))
                }
            };
            if child.dev() != device {
                return Err(CacheMeasurement::error(format!(
                    "cache path crossed onto another filesystem: {}",
                    path.display()
                )));
            }
            seen.insert(
                path.clone(),
                (
                    child.dev(),
                    child.ino(),
                    child.mtime() * 1_000_000_000 + child.mtime_nsec(),
                    child.ctime() * 1_000_000_000 + child.ctime_nsec(),
                ),
            );
            let kind = entry.file_type().map_err(|error| {
                CacheMeasurement::error(format!(
                    "cannot inspect cache entry {}: {error}",
                    path.display()
                ))
            })?;
            if kind.is_dir() && !kind.is_symlink() {
                pending.push(path);
            } else {
                total += (child.blocks() * 512) as i64;
            }
        }
    }
    Ok((total, seen))
}

/// The nested-Git judgment's allowance: 900 seconds per cache
/// root across its Git commands, 120 per command, shared with
/// the census wall deadline.
struct NestedGitBudget {
    total_seconds: f64,
    spent_seconds: f64,
}

impl NestedGitBudget {
    fn command_seconds(&self) -> f64 {
        (self.total_seconds - self.spent_seconds).clamp(0.0, 120.0)
    }
}

struct NestedGitRun {
    success: bool,
    code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run one judgment Git command with the oracle's controlled
/// environment and bound; every failure is the refusal text.
fn nested_git_run(
    dotgit: &Path,
    args: &[std::ffi::OsString],
    budget: &mut NestedGitBudget,
) -> std::result::Result<NestedGitRun, String> {
    let args_text: Vec<String> = args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let refuse = |condition: String| {
        format!(
            "cache directory contains nested Git metadata: {}: not provably disposable: {condition}",
            dotgit.display()
        )
    };
    let seconds = budget.command_seconds();
    if seconds <= 0.0 {
        return Err(refuse(format!(
            "nested Git judgment spent its {}-second allowance for this removal before git {}",
            fmt_g(budget.total_seconds),
            args_text.join(" ")
        )));
    }
    let mut command = std::process::Command::new("git");
    command.args([
        "--no-pager",
        "--no-replace-objects",
        "-c",
        "core.useReplaceRefs=false",
        "-c",
        "core.commitGraph=false",
        "-c",
        "advice.graftFileDeprecated=false",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.excludesFile=/dev/null",
        "-c",
        "core.ignoreStat=false",
        "-c",
        "protocol.allow=never",
    ]);
    let mut git_dir_arg = std::ffi::OsString::from("--git-dir=");
    git_dir_arg.push(dotgit.as_os_str());
    command.arg(git_dir_arg);
    command.args(args);
    command.current_dir("/");
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        use std::os::unix::ffi::OsStrExt as _;
        if !key.as_bytes().starts_with(b"GIT_") {
            command.env(key, value);
        }
    }
    command.env("LC_ALL", "C");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    command.env("GIT_CONFIG_SYSTEM", "/dev/null");
    command.env("GIT_GRAFT_FILE", "/dev/null");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command.env("GIT_OPTIONAL_LOCKS", "0");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_NO_LAZY_FETCH", "1");
    command.env("GIT_ATTR_NOSYSTEM", "1");
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    let started = std::time::Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            budget.spent_seconds += started.elapsed().as_secs_f64();
            return Err(refuse(format!("cannot execute Git: {error}")));
        }
    };
    use std::io::Read as _;
    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut bytes);
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut bytes);
        bytes
    });
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {
                if started.elapsed().as_secs_f64() >= seconds {
                    let _ = child.kill();
                    let _ = child.wait();
                    budget.spent_seconds += started.elapsed().as_secs_f64();
                    return Err(refuse(format!(
                        "git {} did not finish within {} seconds",
                        args_text.join(" "),
                        fmt_g(seconds)
                    )));
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(error) => break Err(error),
        }
    };
    budget.spent_seconds += started.elapsed().as_secs_f64();
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    match status {
        Ok(status) => Ok(NestedGitRun {
            success: status.success(),
            code: status.code().unwrap_or(-1),
            stdout,
            stderr,
        }),
        Err(error) => Err(refuse(format!("cannot execute Git: {error}"))),
    }
}

fn nested_git_detail(run: &NestedGitRun) -> String {
    let bytes = if run.stderr.is_empty() {
        &run.stdout
    } else {
        &run.stderr
    };
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        return format!("exit {}", run.code);
    }
    text.lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

fn os_args(parts: &[&str]) -> Vec<std::ffi::OsString> {
    parts.iter().map(std::ffi::OsString::from).collect()
}

fn nested_git_output(
    dotgit: &Path,
    args: &[std::ffi::OsString],
    budget: &mut NestedGitBudget,
) -> std::result::Result<String, String> {
    let run = nested_git_run(dotgit, args, budget)?;
    if !run.success {
        let args_text: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        return Err(format!(
            "cache directory contains nested Git metadata: {}: not provably disposable: git {} failed: {}",
            dotgit.display(),
            args_text.join(" "),
            nested_git_detail(&run)
        ));
    }
    Ok(String::from_utf8_lossy(&run.stdout).into_owned())
}

/// Judge one nested repository in a completed validation's
/// cache, as completed validation removal judges it: only
/// whether deleting it reaches outside the cache. The refusal
/// text is the oracle's.
fn judge_nested_git(
    dotgit: &Path,
    budget: &mut NestedGitBudget,
) -> std::result::Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    let refuse = |condition: String| {
        format!(
            "cache directory contains nested Git metadata: {}: not provably disposable: {condition}",
            dotgit.display()
        )
    };
    let metadata = std::fs::symlink_metadata(dotgit)
        .map_err(|error| refuse(format!("cannot inspect .git: {error}")))?;
    let kind = metadata.file_type();
    if kind.is_symlink() {
        return Err(refuse(
            ".git is a symbolic link, not a repository directory".to_string(),
        ));
    }
    if kind.is_file() {
        return Err(refuse(
            ".git is a file, not a repository directory (a linked worktree or submodule pointer)"
                .to_string(),
        ));
    }
    if !kind.is_dir() {
        return Err(refuse(".git is not a directory".to_string()));
    }
    let located = nested_git_run(
        dotgit,
        &os_args(&[
            "rev-parse",
            "--path-format=absolute",
            "--git-dir",
            "--git-common-dir",
        ]),
        budget,
    )?;
    if !located.success {
        return Err(refuse(format!(
            "not a well-formed Git repository: {}",
            nested_git_detail(&located)
        )));
    }
    let identity = (metadata.dev(), metadata.ino());
    {
        use std::os::unix::ffi::OsStrExt as _;
        for line in located.stdout.split(|byte| *byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            let reported = PathBuf::from(std::ffi::OsStr::from_bytes(line));
            let same = std::fs::metadata(&reported)
                .map(|meta| (meta.dev(), meta.ino()) == identity)
                .unwrap_or(false);
            if !same {
                return Err(refuse(format!(
                    "Git reports repository directory {}, not this .git directory",
                    reported.display()
                )));
            }
        }
    }
    for config_name in ["config", "config.worktree"] {
        let config_path = dotgit.join(config_name);
        if config_name != "config" && !config_path.exists() {
            continue;
        }
        let mut config_args = os_args(&["config", "--file"]);
        config_args.push(config_path.clone().into_os_string());
        config_args.extend(os_args(&["--null", "--list", "--name-only"]));
        let names = nested_git_output(dotgit, &config_args, budget)?;
        let keys: std::collections::BTreeSet<&str> =
            names.split('\0').filter(|item| !item.is_empty()).collect();
        for key in keys {
            let lowered = key.to_lowercase();
            if lowered.starts_with("include.") || lowered.starts_with("includeif.") {
                return Err(refuse(format!(
                    "repository config sets {key}, which could load configuration this check does not inspect"
                )));
            }
            if lowered == "core.worktree" {
                return Err(refuse(
                    "repository config sets core.worktree, so its worktree may lie outside this cache"
                        .to_string(),
                ));
            }
        }
    }
    for (subdirectory, what) in [
        ("worktrees", "registered linked worktrees"),
        ("modules", "submodule repositories under .git/modules"),
    ] {
        let held: Vec<String> = match std::fs::read_dir(dotgit.join(subdirectory)) {
            Ok(listing) => {
                let mut names: Vec<String> = Vec::new();
                for entry in listing {
                    match entry {
                        Ok(entry) => names.push(entry.file_name().to_string_lossy().into_owned()),
                        Err(error) => {
                            return Err(refuse(format!(
                                "cannot inspect .git/{subdirectory}: {error}"
                            )));
                        }
                    }
                }
                names.sort();
                names
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(refuse(format!(
                    "cannot inspect .git/{subdirectory}: {error}"
                )));
            }
        };
        if !held.is_empty() {
            return Err(refuse(format!(
                "repository has {what}: {}",
                held.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            )));
        }
    }
    let _head = nested_git_run(
        dotgit,
        &os_args(&["rev-parse", "-q", "--verify", "HEAD^{commit}"]),
        budget,
    )?;
    Ok(())
}

/// The outcome of judging a completed validation cache's
/// nested repositories: clean, the first refusal, or the wall
/// deadline expiring mid-judgment (which the oracle publishes
/// as a partial root, never an error).
enum NestedJudgment {
    Clean,
    Refused(String),
    TimedOut,
}

/// Judge every nested repository a completed validation cache
/// contains, in the oracle's order; the first refusal wins.
fn judge_audited_nested(
    root: &Path,
    seen: &BTreeMap<PathBuf, CacheIdentity>,
    deadline: std::time::Instant,
) -> NestedJudgment {
    let mut dotgits: Vec<PathBuf> = seen
        .keys()
        .filter(|path| {
            path.as_path() != root && path.file_name().is_some_and(|name| name == ".git")
        })
        .cloned()
        .collect();
    dotgits.sort_by(|a, b| {
        let a_parts: Vec<String> = a
            .strip_prefix(root)
            .unwrap_or(a)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        let b_parts: Vec<String> = b
            .strip_prefix(root)
            .unwrap_or(b)
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        a_parts.cmp(&b_parts)
    });
    if dotgits.is_empty() {
        return NestedJudgment::Clean;
    }
    let remaining = deadline
        .saturating_duration_since(std::time::Instant::now())
        .as_secs_f64();
    let mut budget = NestedGitBudget {
        total_seconds: remaining.min(900.0),
        spent_seconds: 0.0,
    };
    for dotgit in &dotgits {
        if std::time::Instant::now() >= deadline {
            return NestedJudgment::TimedOut;
        }
        if let Err(error) = judge_nested_git(dotgit, &mut budget) {
            if std::time::Instant::now() >= deadline {
                return NestedJudgment::TimedOut;
            }
            return NestedJudgment::Refused(error);
        }
    }
    NestedJudgment::Clean
}

/// One cache root's bounded census: measure, re-verify the same
/// identities and bytes, then re-stat every recorded identity
/// before a total may publish as complete.
fn traverse_cache_root(
    root: &Path,
    device: u64,
    mounts: &[(i64, PathBuf)],
    budget: &mut i64,
    deadline: std::time::Instant,
    validation_complete: bool,
) -> CacheMeasurement {
    use std::os::unix::fs::MetadataExt as _;
    let (measured, seen) =
        match walk_cache_root(root, device, mounts, budget, deadline, validation_complete) {
            Ok(result) => result,
            Err(measurement) => return measurement,
        };
    let (verified, seen_again) =
        match walk_cache_root(root, device, mounts, budget, deadline, validation_complete) {
            Ok(result) => result,
            Err(measurement) => return measurement,
        };
    if verified != measured || seen_again != seen {
        let changed = seen
            .iter()
            .find(|(path, identity)| seen_again.get(*path) != Some(*identity))
            .map(|(path, _)| path.clone())
            .unwrap_or_else(|| root.to_path_buf());
        return CacheMeasurement::error(format!(
            "cache census directory changed before final publication: {}",
            changed.display()
        ));
    }
    for (path, identity) in &seen {
        if std::time::Instant::now() >= deadline {
            return CacheMeasurement::error(
                "cache census traversal exhausted the fixed wall allowance".to_string(),
            );
        }
        if *budget <= 0 {
            return CacheMeasurement::partial();
        }
        *budget -= 1;
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                let actual = (
                    meta.dev(),
                    meta.ino(),
                    meta.mtime() * 1_000_000_000 + meta.mtime_nsec(),
                    meta.ctime() * 1_000_000_000 + meta.ctime_nsec(),
                );
                if actual != *identity {
                    return CacheMeasurement::error(format!(
                        "cache census directory changed before final publication: {}",
                        path.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return CacheMeasurement::error(format!(
                    "cache census directory changed before final publication: {}",
                    path.display()
                ));
            }
            Err(error) => {
                return CacheMeasurement::error(format!(
                    "cannot inspect cache entry {}: {error}",
                    path.display()
                ))
            }
        }
    }
    if validation_complete {
        match judge_audited_nested(root, &seen, deadline) {
            NestedJudgment::Clean => {}
            NestedJudgment::Refused(error) => return CacheMeasurement::error(error),
            NestedJudgment::TimedOut => return CacheMeasurement::partial(),
        }
    }
    CacheMeasurement::complete(measured)
}

/// Whether `path` equals or lies inside `root` (lexically).
fn path_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

/// Resolve the census state path like `_audit_cache_state_path`.
/// Explicit-path violations are whole-audit refusals; default-home
/// failures only make the accounting unavailable.
fn census_state_path(cfg: &Cfg, explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(raw) = explicit {
        let candidate = PathBuf::from(raw);
        let normalized = PathBuf::from(normalize_absolute(&candidate));
        if !candidate.is_absolute()
            || candidate
                .components()
                .any(|part| part == std::path::Component::ParentDir)
            || normalized != candidate
        {
            return Err(
                "--cache-census-state must be a canonical absolute path without '..'".to_string(),
            );
        }
        if path_within(&candidate, &cfg.root) {
            return Err(
                "audit cache census state must be outside the managed project tree".to_string(),
            );
        }
        let parent = candidate.parent().unwrap_or(Path::new("/"));
        let resolved = std::fs::canonicalize(parent).map_err(|error| {
            format!(
                "audit cache census state parent must already exist: {}: {error}",
                parent.display()
            )
        })?;
        if resolved != parent || path_within(&resolved, &cfg.root) {
            return Err(
                "audit cache census state parent must be symlink-free and outside the managed project tree".to_string(),
            );
        }
        return Ok(candidate);
    }
    Err("default census state path is resolved by the census driver".to_string())
}

fn normalize_absolute(path: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in path.components() {
        match part {
            std::path::Component::RootDir => {}
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop();
            }
            other => parts.push(other.as_os_str().to_string_lossy().into_owned()),
        }
    }
    format!("/{}", parts.join("/"))
}

/// The default project-keyed state path below XDG_CACHE_HOME or
/// ~/.cache, creating the symlink-free directory chain.
fn default_census_state_path(cfg: &Cfg) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let cache_home = match std::env::var_os("XDG_CACHE_HOME") {
        Some(value) => {
            let raw = PathBuf::from(value);
            if let Ok(rest) = raw.strip_prefix("~") {
                home.join(rest)
            } else {
                raw
            }
        }
        _ => home.join(".cache"),
    };
    if !cache_home.is_absolute() {
        return Err("XDG_CACHE_HOME must be absolute for audit cache state".to_string());
    }
    // Every existing component of the cache home itself must be a
    // real directory, like the oracle's nofollow chain.
    {
        let mut prefix = PathBuf::from("/");
        for part in cache_home.components() {
            prefix.push(part);
            match std::fs::symlink_metadata(&prefix) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() || !meta.is_dir() {
                        return Err(format!(
                            "audit cache home is unavailable: {}: not a symlink-free directory",
                            cache_home.display()
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(format!(
                        "audit cache home is unavailable: {}: {error}",
                        cache_home.display()
                    ))
                }
            }
        }
    }
    if path_within(&cache_home, &cfg.root) {
        return Err("audit cache home must be outside the managed project tree".to_string());
    }
    let mut parent = cache_home.clone();
    for component in ["wrkslots", "audit-cache"] {
        parent.push(component);
        match std::fs::create_dir(&parent) {
            Ok(()) => {
                let _ = std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!(
                    "audit cache home is unavailable: {}: {error}",
                    cache_home.display()
                ))
            }
        }
        let meta = std::fs::symlink_metadata(&parent)
            .map_err(|error| format!("cannot prepare audit cache state directory: {error}"))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(format!(
                "audit cache home is unavailable: {}: not a symlink-free directory",
                cache_home.display()
            ));
        }
    }
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(cfg.root.to_string_lossy().as_bytes());
    let project: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(parent.join(format!("{project}.json")))
}

/// Read or create the 32-byte owner-bound census key beside the
/// state file, publishing it without replacing a concurrent key.
fn establish_census_key(state_path: &Path) -> Result<Vec<u8>> {
    use std::io::Write as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let parent = state_path.parent().unwrap_or(Path::new("/"));
    let key_path = parent.join(format!(
        "{}.key",
        state_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let euid = std::fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(0);
    let read_key = |path: &Path| -> Result<Vec<u8>> {
        let meta = std::fs::symlink_metadata(path)
            .map_err(|error| format!("cannot read census key {}: {error}", path.display()))?;
        if !meta.is_file()
            || meta.file_type().is_symlink()
            || meta.uid() != euid
            || meta.nlink() != 1
            || meta.permissions().mode() & 0o7777 != 0o600
            || meta.len() != 32
        {
            return Err(
                "audit cache census key must be one owner-bound 0600 regular file of exactly 32 bytes"
                    .to_string(),
            );
        }
        let contents = std::fs::read(path)
            .map_err(|error| format!("cannot read census key {}: {error}", path.display()))?;
        if contents.len() != 32 {
            return Err("audit cache census key must contain exactly 32 bytes".to_string());
        }
        Ok(contents)
    };
    if key_path.exists() {
        return read_key(&key_path);
    }
    // A per-process candidate plus a no-replace hard-link publish
    // gives the same single-winner key without a shared lock file.
    let candidate = parent.join(format!(
        ".{}.key.pending-{}",
        state_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    let mut options = std::fs::OpenOptions::new();
    options
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let mut file = options
        .open(&candidate)
        .map_err(|error| format!("cannot create census key candidate: {error}"))?;
    // A pre-existing candidate is validated before it is truncated.
    {
        let meta = file
            .metadata()
            .map_err(|error| format!("cannot read census key candidate: {error}"))?;
        if !meta.is_file()
            || meta.uid() != euid
            || meta.nlink() != 1
            || meta.permissions().mode() & 0o7777 != 0o600
            || meta.len() > 32
        {
            return Err(
                "audit cache census key candidate must be one owner-bound 0600 regular file of at most 32 bytes"
                    .to_string(),
            );
        }
    }
    if key_path.exists() {
        let _ = std::fs::remove_file(&candidate);
        return read_key(&key_path);
    }
    file.set_len(0)
        .map_err(|error| format!("cannot create census key candidate: {error}"))?;
    let mut generated = vec![0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| std::io::Read::read_exact(&mut source, &mut generated))
        .map_err(|error| format!("cannot generate census key: {error}"))?;
    file.write_all(&generated)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("cannot create census key candidate: {error}"))?;
    match std::fs::hard_link(&candidate, &key_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("cannot publish census key: {error}")),
    }
    let _ = std::fs::remove_file(&candidate);
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    read_key(&key_path)
}

/// Run the bounded census over every registered subject. State
/// establishment follows the oracle (an unavailable store makes
/// accounting unavailable, never invented); traversal restarts
/// each invocation rather than resuming persisted progress, so
/// measurements for stable caches match while resume-state reuse
/// stays with the Python edition.
fn cache_state_mac(key: &[u8], payload: &Value) -> String {
    use sha2::Digest as _;
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&sha2::Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let text = serde_json::to_string(payload).unwrap_or_default();
    let inner: Vec<u8> = block
        .iter()
        .map(|byte| byte ^ 0x36)
        .chain(text.bytes())
        .collect();
    let inner_hash = sha2::Sha256::digest(&inner);
    let outer: Vec<u8> = block
        .iter()
        .map(|byte| byte ^ 0x5c)
        .chain(inner_hash.iter().copied())
        .collect();
    sha2::Sha256::digest(&outer)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Persist the census state envelope (fresh roots; resuming
/// persisted progress stays with the Python edition) and report
/// failure exactly as the oracle's census does.
fn write_census_state(state_path: &Path, key: &[u8], registry_revision: &str) -> Result<()> {
    use std::io::Write as _;
    let stored = json!({
        "schema": "wrkslots-audit-cache-census/v6",
        "registry_revision": registry_revision,
        "roots": {},
        "next_subject": Value::Null,
    });
    let mut envelope = stored.clone();
    envelope.as_object_mut().expect("state object").insert(
        "hmac_sha256".to_string(),
        json!(cache_state_mac(key, &stored)),
    );
    let encoded = serde_json::to_string(&envelope).unwrap_or_default();
    let parent = state_path.parent().unwrap_or(Path::new("/"));
    let name = state_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temporary = parent.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    let write = (|| -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, state_path)?;
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();
    if write.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    write.map_err(|error| format!("cannot persist audit cache census: {error}"))
}

fn cache_census(
    cfg: &Cfg,
    subjects: &[CensusSubject],
    request: &CensusRequest,
    registry_revision: &str,
) -> Result<Vec<CacheMeasurement>> {
    if request.work_limit < 1 {
        return Err("--cache-work-limit must be positive".to_string());
    }
    if !request.wall_seconds.is_finite() || request.wall_seconds <= 0.0 {
        return Err("--cache-wall-seconds must be finite and positive".to_string());
    }
    let unavailable = |detail: String| -> Vec<CacheMeasurement> {
        subjects
            .iter()
            .map(|(roots, error, _validation)| {
                if let Some(error) = error {
                    CacheMeasurement::error(error.clone())
                } else if roots.is_empty() {
                    CacheMeasurement::complete(0)
                } else {
                    CacheMeasurement::error(detail.clone())
                }
            })
            .collect()
    };
    if !subjects.iter().any(|(roots, _, _)| !roots.is_empty()) {
        return Ok(subjects
            .iter()
            .map(|(_, error, _)| match error {
                Some(error) => CacheMeasurement::error(error.clone()),
                None => CacheMeasurement::complete(0),
            })
            .collect());
    }
    let state_path = match &request.state {
        Some(raw) => Some(census_state_path(cfg, Some(raw))?),
        None => match default_census_state_path(cfg) {
            Ok(path) => Some(path),
            Err(error) => {
                return Ok(unavailable(format!(
                    "default audit cache storage is unavailable: {error}"
                )))
            }
        },
    };
    let state_path = state_path.expect("census state path");
    // The state store must be establishable exactly as the oracle
    // establishes it; its progress contents are not consumed here.
    let parent = state_path.parent().unwrap_or(Path::new("/"));
    let parent_ok = std::fs::canonicalize(parent)
        .is_ok_and(|resolved| resolved == parent && !path_within(&resolved, &cfg.root));
    if !parent_ok {
        return Ok(unavailable(
            "cannot establish authenticated audit cache census state: audit cache census state parent is unsafe"
                .to_string(),
        ));
    }
    let census_key = match establish_census_key(&state_path) {
        Ok(key) => key,
        Err(error) => {
            return Ok(unavailable(format!(
                "cannot establish authenticated audit cache census state: {error}"
            )))
        }
    };
    let mounts = match census_mounts() {
        Ok(mounts) => mounts,
        Err(error) => return Ok(unavailable(error)),
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs_f64(request.wall_seconds);
    let mut budget: i64 = request.work_limit;
    let mut measurements = Vec::new();
    for (roots, error, validation_complete) in subjects {
        if let Some(error) = error {
            measurements.push(CacheMeasurement::error(error.clone()));
            continue;
        }
        if roots.is_empty() {
            measurements.push(CacheMeasurement::complete(0));
            continue;
        }
        let mut total: i64 = 0;
        let mut outcome: Option<CacheMeasurement> = None;
        for (root, _checkout, device) in roots {
            let measured = traverse_cache_root(
                root,
                *device,
                &mounts,
                &mut budget,
                deadline,
                *validation_complete,
            );
            match measured.status {
                "complete" => total += measured.bytes.unwrap_or(0),
                "partial" => {
                    outcome = Some(CacheMeasurement::partial());
                    break;
                }
                _ => {
                    outcome = Some(measured);
                    break;
                }
            }
        }
        measurements.push(outcome.unwrap_or_else(|| CacheMeasurement::complete(total)));
    }
    if let Err(error) = write_census_state(&state_path, &census_key, registry_revision) {
        for (index, measurement) in measurements.iter_mut().enumerate() {
            if !subjects[index].0.is_empty() && measurement.status != "error" {
                *measurement = CacheMeasurement::error(error.clone());
            }
        }
    }
    Ok(measurements)
}

/// The record digest the oracle publishes: `_event_digest` of
/// `_record_to_obj` (the stored object, with `slot_type` defaulted).
fn record_digest(record: &Value) -> Result<String> {
    let mut obj = record.clone();
    if obj.get("slot_type").is_none() {
        obj.as_object_mut()
            .expect("record object")
            .insert("slot_type".to_string(), json!("agent"));
    }
    canonical_sha256(&obj).map_err(|error| error.to_string())
}

fn heartbeat_diagnosis(record: &Value) -> Result<(i64, bool)> {
    let stamp = record
        .get("heartbeat_at")
        .and_then(Value::as_str)
        .unwrap_or_default();
    parse_timestamp(stamp, "heartbeat_at")?;
    let parsed = chrono::DateTime::parse_from_rfc3339(stamp)
        .map_err(|error| format!("heartbeat timestamp is invalid: {error}"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    let age = now - parsed.timestamp();
    let ttl = record
        .get("heartbeat_ttl_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let age = age.max(0);
    Ok((age, age > ttl))
}

/// Journal and stray-registration overrides for one registered row,
/// in the oracle's order: an interrupted journal blocks the row,
/// then (for DELETABLE/BLOCKED rows with the directory present) stray
/// Git registrations block it too.
fn apply_overrides(
    row: &mut Value,
    journals: &BTreeMap<String, JournalSlot>,
    strays: &BTreeMap<(String, String), Vec<String>>,
    slot: &str,
    slot_type: &str,
    slot_absent: bool,
    sealed_generation: Option<i64>,
) {
    if let Some(journal) = journals.get(slot) {
        let state_text = journal.state.replace('-', " ");
        let row_reasons = row
            .get_mut("reasons")
            .and_then(Value::as_array_mut)
            .expect("reasons list");
        row_reasons.insert(
            0,
            json!(format!(
                "interrupted {state_text} requires wrkslots recover"
            )),
        );
        row.as_object_mut()
            .expect("row object")
            .insert("verdict".to_string(), json!("BLOCKED"));
    }
    if let Some(sealed_generation) = sealed_generation {
        let active = row.get("generation").and_then(Value::as_i64).unwrap_or(0);
        let (reason, verdict) = if sealed_generation == active {
            (
                "interrupted validation-batch seal requires wrkslots recover".to_string(),
                "BLOCKED",
            )
        } else {
            (
                format!(
                    "validation-batch seal cannot be attributed to the active generation: sealed={sealed_generation} active={active}"
                ),
                "UNKNOWN",
            )
        };
        let row_reasons = row
            .get_mut("reasons")
            .and_then(Value::as_array_mut)
            .expect("reasons list");
        row_reasons.insert(0, json!(reason));
        row.as_object_mut()
            .expect("row object")
            .insert("verdict".to_string(), json!(verdict));
    }
    let verdict = row
        .get("verdict")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if matches!(verdict.as_str(), "DELETABLE" | "BLOCKED") && !slot_absent {
        if let Some(stray) = strays.get(&(slot_type.to_string(), slot.to_string())) {
            let row_reasons = row
                .get_mut("reasons")
                .and_then(Value::as_array_mut)
                .expect("reasons list");
            row_reasons.extend(stray.iter().map(|item| json!(item)));
            row.as_object_mut()
                .expect("row object")
                .insert("verdict".to_string(), json!("BLOCKED"));
        }
    }
}

/// The stable host identity (`/etc/machine-id`, then the D-Bus
/// machine id), like `_host_id`.
fn host_id() -> Result<String> {
    let mut failures: Vec<String> = Vec::new();
    for raw in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        let path = Path::new(raw);
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let value = text.trim().to_string();
                if !value.is_empty() {
                    return Ok(value);
                }
                failures.push(format!("{raw}: empty"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => failures.push(format!("{raw}: {error}")),
        }
    }
    let detail = if failures.is_empty() {
        String::new()
    } else {
        format!(" ({})", failures.join("; "))
    };
    Err(format!(
        "cannot establish the stable machine identity{detail}"
    ))
}

/// `_liveness_batch_diagnostic`: bound and flatten one diagnostic.
fn liveness_diagnostic(value: &str) -> String {
    let limit = 4096;
    let marker = " [truncated]";
    let excerpt: String = value.chars().take(limit).collect();
    let printable: String = excerpt
        .chars()
        .map(|c| {
            if c.is_control() || c == '\u{7f}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = printable.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > limit || collapsed.len() > limit {
        let mut truncated: Vec<u8> = collapsed.as_bytes()[..limit - marker.len()].to_vec();
        while !std::str::from_utf8(&truncated).is_ok() {
            truncated.pop();
        }
        let text = String::from_utf8_lossy(&truncated).trim_end().to_string();
        return format!("{text}{marker}");
    }
    if collapsed.is_empty() {
        "no detail".to_string()
    } else {
        collapsed
    }
}

struct SystemdUnit {
    id: String,
    object_path: String,
    active_state: String,
    job_id: i64,
}

/// The user-manager unit listing, parsed from the same `busctl`
/// call the oracle makes.
fn systemd_units() -> Result<Vec<SystemdUnit>> {
    let output = std::process::Command::new("/usr/bin/busctl")
        .args([
            "--user",
            "--json=short",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "ListUnitsByPatterns",
            "asas",
            "0",
            "2",
            "*.service",
            "*.scope",
        ])
        .output()
        .map_err(|error| format!("cannot execute busctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "busctl unit listing failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let document: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("user-systemd unit listing is malformed: {error}"))?;
    if document.get("type").and_then(Value::as_str) != Some("a(ssssssouso)") {
        return Err("user-systemd unit listing has an unexpected signature".to_string());
    }
    let rows = document
        .pointer("/data/0")
        .and_then(Value::as_array)
        .ok_or_else(|| "user-systemd unit listing has an invalid shape".to_string())?;
    let mut units = Vec::new();
    for row in rows {
        let fields = row
            .as_array()
            .ok_or_else(|| "user-systemd unit listing row is not a tuple".to_string())?;
        if fields.len() != 10 {
            return Err("user-systemd unit listing row is not a tuple".to_string());
        }
        let text = |index: usize| -> Result<String> {
            fields[index]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| "user-systemd unit listing field is not a string".to_string())
        };
        units.push(SystemdUnit {
            id: text(0)?,
            active_state: text(3)?,
            object_path: text(6)?,
            job_id: fields[7].as_i64().unwrap_or(0),
        });
    }
    Ok(units)
}

/// Every string leaf of one unit's property replies, flattened:
/// the Unit interface properties plus the Service (or Scope)
/// properties, as the oracle's snapshot reads them.
fn property_text(unit: &SystemdUnit) -> Result<String> {
    fn collect(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::String(text) => out.push(text.clone()),
            Value::Array(items) => items.iter().for_each(|item| collect(item, out)),
            Value::Object(map) => map.values().for_each(|item| collect(item, out)),
            _ => {}
        }
    }
    fn read(unit: &SystemdUnit, interface: &str, properties: &[&str]) -> Result<Vec<String>> {
        let mut args: Vec<String> = vec![
            "--user".to_string(),
            "--json=short".to_string(),
            "get-property".to_string(),
            "org.freedesktop.systemd1".to_string(),
            unit.object_path.clone(),
            interface.to_string(),
        ];
        args.extend(properties.iter().map(|item| item.to_string()));
        let output = std::process::Command::new("/usr/bin/busctl")
            .args(&args)
            .output()
            .map_err(|error| format!("cannot execute busctl: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "busctl property read for {} failed: {}",
                unit.id,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let mut leaves: Vec<String> = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Ok(document) = serde_json::from_str::<Value>(line) {
                collect(&document, &mut leaves);
            }
        }
        Ok(leaves)
    }
    let mut leaves = read(
        unit,
        "org.freedesktop.systemd1.Unit",
        &[
            "Id",
            "LoadState",
            "ActiveState",
            "SubState",
            "RequiresMountsFor",
        ],
    )?;
    if unit.id.ends_with(".service") {
        leaves.extend(read(
            unit,
            "org.freedesktop.systemd1.Service",
            &[
                "ControlGroup",
                "MainPID",
                "WorkingDirectory",
                "ExecStart",
                "ExecStartPre",
                "ExecStartPost",
                "ExecReload",
                "ExecStop",
                "ExecStopPost",
                "Environment",
                "EnvironmentFiles",
                "RootDirectory",
                "RootImage",
                "BindPaths",
                "BindReadOnlyPaths",
                "ReadWritePaths",
                "ReadOnlyPaths",
                "InaccessiblePaths",
                "StandardInput",
                "StandardOutput",
                "StandardError",
            ],
        )?);
    } else {
        leaves.extend(read(
            unit,
            "org.freedesktop.systemd1.Scope",
            &["ControlGroup"],
        )?);
    }
    Ok(leaves.join("\n"))
}

/// Whether one unit's property text names any row path: the path
/// spelling appears in the text, or a path word in it is the row
/// path or lies inside it.
fn unit_names_path(text: &str, paths: &[PathBuf]) -> Option<PathBuf> {
    let spellings: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    for (path, spelling) in paths.iter().zip(&spellings) {
        if text.contains(spelling.as_str()) {
            return Some(path.clone());
        }
    }
    for word in text.split(|c: char| c.is_whitespace() || c == '\0') {
        let word = word.trim_matches(|c| matches!(c, '"' | '\'' | ',' | ';'));
        let Some(start) = word.find('/') else {
            continue;
        };
        let candidate = PathBuf::from(&word[start..]);
        let resolved = std::fs::canonicalize(&candidate).unwrap_or(candidate);
        for path in paths {
            let root = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            if resolved == root || resolved.starts_with(&root) {
                return Some(path.clone());
            }
        }
    }
    None
}

const RETAINED_HANDLE_BYTES_LIMIT: usize = 256 * 1024 * 1024;
const RETAINED_HANDLE_COUNT_LIMIT: usize = 65_536;
const RETAINED_HANDLE_STABLE_NS: i64 = 2_000_000_000;
const RETAINED_HANDLE_CLOCK_STEP_NS: i64 = 500_000_000;

type ProcessGeneration = (String, i64, i64);

#[derive(Clone)]
struct RetainedHandle {
    path: PathBuf,
    unit: String,
    pid: Option<i64>,
    start_ticks: Option<i64>,
    boot_id: Option<String>,
    file_generation: (u64, u64, u64, String, i64, i64),
    recent: bool,
    clock_offsets_ns: (i64, i64),
}

impl RetainedHandle {
    fn same_generation(&self, other: &Self) -> bool {
        self.path == other.path
            && self.unit == other.unit
            && self.pid == other.pid
            && self.start_ticks == other.start_ticks
            && self.boot_id == other.boot_id
            && self.file_generation == other.file_generation
    }

    fn generation(&self) -> Option<ProcessGeneration> {
        Some((self.boot_id.clone()?, self.pid?, self.start_ticks?))
    }
}

fn monotonic_ns() -> i64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as i64
}

fn realtime_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as i64)
        .unwrap_or(0)
}

fn retained_handle_clocks() -> (i64, i64, i64) {
    let before = monotonic_ns();
    let realtime = realtime_ns();
    let after = monotonic_ns();
    (realtime, realtime - after, realtime - before)
}

fn resolved_spelling(candidate: &Path) -> PathBuf {
    if let Ok(resolved) = std::fs::canonicalize(candidate) {
        return resolved;
    }
    let mut existing = candidate.to_path_buf();
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    while std::fs::symlink_metadata(&existing).is_err() {
        let Some(name) = existing.file_name().map(|name| name.to_os_string()) else {
            return candidate.to_path_buf();
        };
        suffix.push(name);
        if !existing.pop() {
            return candidate.to_path_buf();
        }
    }
    let mut resolved = std::fs::canonicalize(&existing).unwrap_or(existing);
    for part in suffix.iter().rev() {
        resolved.push(part);
    }
    resolved
}

fn retained_path_names(candidate: &Path, row_paths: &[PathBuf]) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let resolved_candidate = resolved_spelling(candidate);
    for root in row_paths {
        if candidate == root || candidate.starts_with(root) {
            return true;
        }
        if let (Ok(candidate_file), Ok(root_file)) = (
            std::fs::symlink_metadata(candidate),
            std::fs::symlink_metadata(root),
        ) {
            if candidate_file.dev() == root_file.dev() && candidate_file.ino() == root_file.ino() {
                return true;
            }
        }
        // A bind-mounted spelling of the row directory has the
        // row's own file identity at the candidate or an ancestor.
        for ancestor in candidate.ancestors() {
            if let (Ok(candidate_file), Ok(root_file)) = (
                std::fs::symlink_metadata(ancestor),
                std::fs::symlink_metadata(root),
            ) {
                if candidate_file.dev() == root_file.dev()
                    && candidate_file.ino() == root_file.ino()
                {
                    return true;
                }
            }
        }
        let resolved_root = resolved_spelling(root);
        if resolved_candidate == resolved_root || resolved_candidate.starts_with(&resolved_root) {
            return true;
        }
    }
    false
}

fn retained_strict_object(contents: &[u8], path: &Path) -> Result<serde_json::Map<String, Value>> {
    let label = format!(
        "cannot prove retained validation handle is unrelated: {}",
        path.display()
    );
    strict_json_object(contents, &label).map_err(|error| {
        if let Some(start) = error.find("duplicate key \"") {
            let rest = &error[start + "duplicate key \"".len()..];
            if let Some(end) = rest.find('"') {
                return format!(
                    "{label} is malformed JSON: duplicate key '{}'",
                    &rest[..end]
                );
            }
        }
        error
    })
}

fn retained_handles(
    cfg: &Cfg,
    row_paths: &[PathBuf],
    validation_paths: &[PathBuf],
) -> Result<Vec<RetainedHandle>> {
    use std::os::unix::fs::MetadataExt as _;

    let started_at = std::time::Instant::now();
    let deadline = started_at + std::time::Duration::from_secs(30);
    let handles_dir = cfg.root.join("ignored").join("validate").join("runs");
    if handles_dir.is_symlink() || (handles_dir.exists() && !handles_dir.is_dir()) {
        return Err(format!(
            "retained validation handle directory is unsafe: {}",
            handles_dir.display()
        ));
    }
    let started = retained_handle_clocks();
    let mut handle_paths: Vec<PathBuf> = Vec::new();
    let mut declared_bytes: u64 = 0;
    if handles_dir.is_dir() {
        let entries = std::fs::read_dir(&handles_dir)
            .map_err(|error| format!("cannot enumerate retained validation handles: {error}"))?;
        for entry in entries {
            if std::time::Instant::now() >= deadline {
                return Err(
                    "retained validation handle enumeration exceeded its time bound".to_string(),
                );
            }
            let entry =
                entry.map_err(|error| format!("cannot enumerate retained handles: {error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".json") {
                continue;
            }
            if handle_paths.len() >= RETAINED_HANDLE_COUNT_LIMIT {
                return Err(format!(
                    "retained validation handles exceed the file-count census bound of {RETAINED_HANDLE_COUNT_LIMIT} *.json files in {}",
                    handles_dir.display()
                ));
            }
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                format!("cannot enumerate retained validation handles: {error}")
            })?;
            if !metadata.is_file() {
                return Err(format!(
                    "retained validation handle is unsafe: {}",
                    path.display()
                ));
            }
            declared_bytes += metadata.len();
            if declared_bytes > RETAINED_HANDLE_BYTES_LIMIT as u64 {
                return Err(format!(
                    "retained validation handles exceed the 256 MiB census bound in {}",
                    handles_dir.display()
                ));
            }
            handle_paths.push(path);
        }
    }
    handle_paths.sort();
    let mut found: Vec<RetainedHandle> = Vec::new();
    let mut total_bytes: usize = 0;
    for path in handle_paths {
        if std::time::Instant::now() >= deadline {
            return Err("retained validation handle census exceeded its time bound".to_string());
        }
        let (contents, read) = read_regular_file_identity(
            &path,
            "retained validation handle",
            RETAINED_HANDLE_BYTES_LIMIT,
        )?;
        let after = std::fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "cannot read retained validation handle {}: {error}",
                path.display()
            )
        })?;
        if after.dev() != read.device || after.ino() != read.inode {
            return Err(format!(
                "retained validation handle was replaced while it was read: {}",
                path.display()
            ));
        }
        total_bytes += contents.len();
        if total_bytes > RETAINED_HANDLE_BYTES_LIMIT {
            return Err(format!(
                "retained validation handles exceed the 256 MiB census bound in {}",
                handles_dir.display()
            ));
        }
        let value = retained_strict_object(&contents, &path)?;
        let mut related = false;
        let mut related_any = false;
        for field in ["checkout", "source_checkout"] {
            let Some(raw) = value.get(field) else {
                continue;
            };
            if raw.is_null() {
                continue;
            }
            let Some(raw) = raw.as_str() else {
                return Err(format!(
                    "cannot prove retained validation handle is unrelated: {} has a non-string {field}",
                    path.display()
                ));
            };
            if raw.is_empty() {
                return Err(format!("{}.{field} is empty", path.display()));
            }
            let candidate = if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                cfg.root.join(raw)
            };
            if retained_path_names(&candidate, row_paths) {
                related = true;
            }
            if retained_path_names(&candidate, validation_paths) {
                related_any = true;
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err("retained validation handle census exceeded its time bound".to_string());
        }
        if !related_any {
            continue;
        }
        let unit = value
            .get("unit")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut unit_stem = unit.to_string();
        if let Some(stem) = unit_stem.strip_suffix(".service") {
            unit_stem = stem.to_string();
        }
        if let Some(stem) = unit_stem.strip_suffix(".scope") {
            unit_stem = stem.to_string();
        }
        let unit_stem = Some(unit_stem).filter(|stem| !stem.is_empty());
        let valid_unit = !unit.is_empty()
            && (unit.ends_with(".service") || unit.ends_with(".scope"))
            && unit
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '@' | '-'))
            && unit_stem
                .as_ref()
                .is_some_and(|stem| file_name == format!("{stem}.json"));
        if !valid_unit {
            return Err(format!(
                "retained validation handle {} has no matching exact unit identity",
                path.display()
            ));
        }
        let (mut pid, mut start_ticks, mut boot_id) = (None, None, None);
        if let Some(identity) = value.get("process_identity").filter(|item| !item.is_null()) {
            let label = format!("{}.process_identity", path.display());
            let raw = as_mapping(identity, &label)?;
            exact_keys(raw, &["pid", "start_ticks", "boot_id"], &[], &label)?;
            pid = Some(as_int(&raw["pid"], &format!("{label}.pid"), 1)?);
            start_ticks = Some(as_int(
                &raw["start_ticks"],
                &format!("{label}.start_ticks"),
                1,
            )?);
            let boot = as_str(&raw["boot_id"], &format!("{label}.boot_id"))?;
            if boot.is_empty() {
                return Err(format!(
                    "retained validation handle {} has an empty boot identity",
                    path.display()
                ));
            }
            boot_id = Some(boot.to_string());
        }
        // A sibling validation row's unit is part of the batched
        // census: an unreadable member census poisons every row.
        retained_unit_member_count(unit)?;
        if !related {
            continue;
        }
        found.push(RetainedHandle {
            path: path.clone(),
            unit: unit.to_string(),
            pid,
            start_ticks,
            boot_id,
            file_generation: (
                read.device,
                read.inode,
                read.size,
                read.sha256,
                after.mtime() * 1_000_000_000 + after.mtime_nsec(),
                after.ctime() * 1_000_000_000 + after.ctime_nsec(),
            ),
            recent: false,
            clock_offsets_ns: (0, 0),
        });
    }
    if std::time::Instant::now() >= deadline {
        return Err("retained validation handle census exceeded its time bound".to_string());
    }
    let finished = retained_handle_clocks();
    let stable_before_ns = started.0.min(finished.0) - RETAINED_HANDLE_STABLE_NS;
    let clock_offsets_ns = (started.1.min(finished.1), started.2.max(finished.2));
    for handle in &mut found {
        handle.recent = handle.file_generation.5 > stable_before_ns;
        handle.clock_offsets_ns = clock_offsets_ns;
    }
    Ok(found)
}

fn settle_retained_handles(handles: &[RetainedHandle]) -> bool {
    let changes: Vec<i64> = handles
        .iter()
        .filter(|handle| handle.recent)
        .map(|handle| handle.file_generation.5)
        .collect();
    if changes.is_empty() {
        return false;
    }
    let wait_ns = changes.iter().max().unwrap_or(&0) + RETAINED_HANDLE_STABLE_NS - realtime_ns();
    if wait_ns > 0 && wait_ns <= RETAINED_HANDLE_STABLE_NS + 250_000_000 {
        std::thread::sleep(
            std::time::Duration::from_nanos(wait_ns as u64) + std::time::Duration::from_millis(10),
        );
    }
    true
}

fn merge_retained_handles(groups: &[&[RetainedHandle]]) -> Vec<RetainedHandle> {
    let mut merged: Vec<RetainedHandle> = Vec::new();
    for group in groups {
        for handle in *group {
            if !merged.iter().any(|item| item.same_generation(handle)) {
                merged.push(handle.clone());
            }
        }
    }
    merged
}

fn realtime_clock_stepped_back(before: &RetainedHandle, after: &RetainedHandle) -> bool {
    before.clock_offsets_ns.1.max(after.clock_offsets_ns.1)
        - before.clock_offsets_ns.0.min(after.clock_offsets_ns.0)
        > RETAINED_HANDLE_CLOCK_STEP_NS
}

fn retained_handle_changes(
    before: &[RetainedHandle],
    after: &[RetainedHandle],
    window: bool,
    during: &str,
) -> Vec<(RetainedHandle, String)> {
    let mut changes: Vec<(RetainedHandle, String)> = Vec::new();
    for handle in after {
        match before.iter().find(|item| item.path == handle.path) {
            Some(previous) if previous.same_generation(handle) => {}
            Some(_) => changes.push((handle.clone(), format!("changed {during}"))),
            None => changes.push((handle.clone(), format!("appeared {during}"))),
        }
    }
    if window {
        for handle in after {
            if let Some(previous) = before.iter().find(|item| item.path == handle.path) {
                if previous.same_generation(handle) && (previous.recent || handle.recent) {
                    changes.push((
                        handle.clone(),
                        format!(
                            "may have been rewritten alike within one file timestamp tick {during}"
                        ),
                    ));
                }
            }
        }
    }
    for handle in after {
        if let Some(previous) = before.iter().find(|item| item.path == handle.path) {
            if previous.same_generation(handle) && realtime_clock_stepped_back(previous, handle) {
                changes.push((
                    handle.clone(),
                    format!(
                        "may have been rewritten alike while this host's realtime clock stepped back {during}"
                    ),
                ));
            }
        }
    }
    for handle in before {
        if !after.iter().any(|item| item.path == handle.path) {
            changes.push((
                handle.clone(),
                format!("disappeared or stopped naming the row {during}"),
            ));
        }
    }
    changes
}

fn recorded_generations(
    handles: &[RetainedHandle],
) -> (
    BTreeSet<ProcessGeneration>,
    BTreeMap<ProcessGeneration, String>,
) {
    let mut running: BTreeSet<ProcessGeneration> = BTreeSet::new();
    let mut unreadable: BTreeMap<ProcessGeneration, String> = BTreeMap::new();
    let mut current_boot: Option<std::result::Result<String, String>> = None;
    for handle in handles {
        let Some(generation) = handle.generation() else {
            continue;
        };
        if running.contains(&generation) || unreadable.contains_key(&generation) {
            continue;
        }
        if current_boot.is_none() {
            current_boot = Some(crate::status_state::boot_id());
        }
        match current_boot.as_ref().expect("boot read") {
            Ok(boot) if boot == &generation.0 => {}
            Ok(_) => continue,
            Err(error) => {
                unreadable.insert(generation, error.clone());
                continue;
            }
        }
        match crate::status_state::process_start_ticks(generation.1) {
            Ok(Some(ticks)) if ticks == generation.2 => {
                running.insert(generation);
            }
            Ok(_) => {}
            Err(error) => {
                unreadable.insert(generation, error);
            }
        }
    }
    (running, unreadable)
}

fn user_manager_cgroup() -> Result<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;

    let root = PathBuf::from("/sys/fs/cgroup");
    let uid = std::fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .map_err(|error| format!("this process's control group is unreadable: {error}"))?;
    let manager = format!("user@{uid}.service");
    let text = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|error| format!("this process's control group is unreadable: {error}"))?;
    {
        for line in text.lines() {
            let mut fields = line.splitn(3, ':');
            if fields.next() != Some("0") {
                continue;
            }
            let _ = fields.next();
            let Some(path) = fields.next() else { continue };
            let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
            if let Some(index) = parts.iter().position(|part| *part == manager) {
                return Ok(root.join(parts[..=index].join("/")));
            }
        }
    }
    Ok(root
        .join("user.slice")
        .join(format!("user-{uid}.slice"))
        .join(manager))
}

fn retained_unit_member_count(unit: &str) -> Result<i64> {
    let root = PathBuf::from("/sys/fs/cgroup");
    if !root.join("cgroup.controllers").is_file() {
        return Err(format!(
            "no cgroup v2 hierarchy is mounted at {}",
            root.display()
        ));
    }
    let manager = user_manager_cgroup()?;
    if !manager.is_dir() {
        return Err(format!(
            "this user's service manager control group {} is not visible, so a retained run unit's control-group members cannot be read",
            manager.display()
        ));
    }
    let mut total = 0i64;
    let mut visited = 0usize;
    let mut pending = vec![manager.clone()];
    while let Some(directory) = pending.pop() {
        visited += 1;
        if visited > 65_536 {
            return Err("user-manager control groups exceed the census bound".to_string());
        }
        let relative = directory.strip_prefix(&manager).unwrap_or(Path::new(""));
        let named = relative
            .components()
            .any(|part| part.as_os_str().to_string_lossy() == unit);
        if named {
            match std::fs::read_to_string(directory.join("cgroup.procs")) {
                Ok(text) => {
                    total += text.lines().filter(|line| !line.trim().is_empty()).count() as i64
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "control group {} member list is unreadable: {error}",
                        directory.display()
                    ))
                }
            }
        }
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        pending.push(path);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("user-manager control group is unreadable: {error}")),
        }
    }
    Ok(total)
}

fn unit_control_group(unit: &SystemdUnit) -> Result<Option<String>> {
    let interface = if unit.id.ends_with(".service") {
        "org.freedesktop.systemd1.Service"
    } else {
        "org.freedesktop.systemd1.Scope"
    };
    let output = std::process::Command::new("/usr/bin/busctl")
        .args([
            "--user",
            "--json=short",
            "get-property",
            "org.freedesktop.systemd1",
            &unit.object_path,
            interface,
            "ControlGroup",
        ])
        .output()
        .map_err(|error| format!("cannot execute busctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "busctl property read for {} failed: {}",
            unit.id,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Ok(document) = serde_json::from_str::<Value>(line) {
            if let Some(value) = document.get("data").and_then(Value::as_str) {
                return Ok(Some(value.to_string()));
            }
        }
    }
    Err(format!(
        "busctl property read for {} has an unexpected ControlGroup shape",
        unit.id
    ))
}

struct AuditMountEntry {
    point: String,
    fstype: String,
    root: String,
    super_options: String,
}

fn audit_unescape_mount(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

fn audit_mount_entries() -> Result<Vec<AuditMountEntry>> {
    let text = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("cannot read /proc/self/mountinfo: {error}"))?;
    let mut entries = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if left_fields.len() < 5 || right_fields.len() < 3 {
            continue;
        }
        entries.push(AuditMountEntry {
            root: audit_unescape_mount(left_fields[3]),
            point: audit_unescape_mount(left_fields[4]),
            fstype: right_fields[0].to_string(),
            super_options: right_fields[2].to_string(),
        });
    }
    Ok(entries)
}

/// The host process/cgroup view proof: retained-handle `dead`
/// judgements are only as good as the process table and the
/// whole cgroup hierarchy being visible from here.
fn assert_host_process_view() -> Result<()> {
    use std::os::unix::fs::MetadataExt as _;

    for (name, expected) in [("pid", 0xEFFF_FFFCu64), ("cgroup", 0xEFFF_FFFBu64)] {
        let metadata = std::fs::metadata(format!("/proc/self/ns/{name}"))
            .map_err(|error| format!("cannot read this process's {name} namespace: {error}"))?;
        if metadata.ino() != expected {
            return Err(format!(
                "this process is not in the host's initial {name} namespace (inode {}), so its process table need not show the recorded runs",
                metadata.ino()
            ));
        }
    }
    let own = std::fs::read_link("/proc/self")
        .map_err(|error| format!("cannot resolve /proc/self: {error}"))?;
    if own.to_string_lossy() != std::process::id().to_string() {
        return Err(
            "/proc is not the process filesystem of this process's PID namespace".to_string(),
        );
    }
    let entries = audit_mount_entries()?;
    let proc_index = entries
        .iter()
        .rposition(|entry| entry.point == "/proc")
        .ok_or_else(|| "no process filesystem is mounted at /proc".to_string())?;
    if entries[proc_index].fstype != "proc" {
        return Err("no process filesystem is mounted at /proc".to_string());
    }
    let hidden: Vec<&str> = entries[proc_index]
        .super_options
        .split(',')
        .filter(|option| {
            option
                .strip_prefix("hidepid=")
                .is_some_and(|value| value != "0" && value != "off")
        })
        .collect();
    if let Some(option) = hidden.last() {
        return Err(format!(
            "/proc is mounted with {option}, so its process table need not show the recorded runs"
        ));
    }
    for entry in entries.iter().skip(proc_index + 1) {
        let mut parts = entry.point.split('/').filter(|part| !part.is_empty());
        if parts.next() == Some("proc")
            && parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(format!(
                "a {} mount at {} masks that process's entries in /proc, so its process table need not show the recorded runs",
                entry.fstype, entry.point
            ));
        }
    }
    let cgroup_root = "/sys/fs/cgroup";
    let cgroup_index = entries
        .iter()
        .rposition(|entry| entry.point == cgroup_root)
        .ok_or_else(|| format!("no cgroup v2 hierarchy is mounted at {cgroup_root}"))?;
    if entries[cgroup_index].fstype != "cgroup2" {
        return Err(format!(
            "no cgroup v2 hierarchy is mounted at {cgroup_root}"
        ));
    }
    if entries[cgroup_index].root != "/" {
        return Err(format!(
            "the cgroup v2 mount at {cgroup_root} shows only {} of the hierarchy, so a retained run unit's control group can be outside it",
            entries[cgroup_index].root
        ));
    }
    for entry in entries.iter().skip(cgroup_index + 1) {
        if entry.point.starts_with(&format!("{cgroup_root}/")) {
            return Err(format!(
                "a {} mount at {} covers part of the cgroup v2 hierarchy, so a retained run unit's control-group members can read empty",
                entry.fstype, entry.point
            ));
        }
    }
    Ok(())
}

type ProcessObservation = (i64, i64, String);

fn process_observations() -> Result<Vec<ProcessObservation>> {
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate processes in /proc: {error}"))?;
    let mut observations = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot enumerate processes in /proc: {error}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        let ticks = match crate::status_state::process_start_ticks(pid) {
            Ok(Some(ticks)) => ticks,
            Ok(None) => continue,
            Err(error) => return Err(error),
        };
        let Ok(text) = std::fs::read_to_string(entry.path().join("cgroup")) else {
            continue;
        };
        for line in text.lines() {
            let mut fields = line.splitn(3, ':');
            if fields.next() != Some("0") {
                continue;
            }
            let _ = fields.next();
            if let Some(path) = fields.next() {
                let trimmed = path.trim_end_matches('/');
                observations.push((
                    pid,
                    ticks,
                    if trimmed.is_empty() {
                        "/".to_string()
                    } else {
                        trimmed.to_string()
                    },
                ));
            }
        }
    }
    Ok(observations)
}

fn effective_cgroup_paths(
    before: &[ProcessObservation],
    after: &[ProcessObservation],
) -> Vec<String> {
    let mut paths: Vec<String> = after.iter().map(|item| item.2.clone()).collect();
    for item in before {
        if after
            .iter()
            .any(|other| other.0 == item.0 && other.1 == item.1)
            && !after.contains(item)
        {
            paths.push(item.2.clone());
        }
    }
    paths
}

fn cgroup_matches(recorded: &str, observed: &str) -> bool {
    let prefix = recorded.trim_end_matches('/');
    let prefix = if prefix.is_empty() { "/" } else { prefix };
    observed == prefix
        || (prefix != "/" && observed.starts_with(&format!("{prefix}/")))
        || (prefix == "/" && observed.starts_with('/'))
}

fn validation_row_paths(cfg: &Cfg, record: &Value) -> Vec<PathBuf> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Ok(slot_path) = slot_directory(cfg, slot, "validate") else {
        return Vec::new();
    };
    let mut paths = vec![slot_path.clone()];
    if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
        for checkout in checkouts {
            if let Some(raw) = checkout.get("path").and_then(Value::as_str) {
                if let Ok(path) = stored_path(cfg, raw, "checkout path") {
                    paths.push(path);
                }
            }
        }
    }
    if let Some(parent) = slot_path.parent() {
        let generation = record
            .get("generation")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let prefix = format!(".{slot}.fenced.{generation}.");
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    paths.push(entry.path());
                }
            }
        }
    }
    paths
}

/// Validation-row liveness from the run authority. Stable
/// retained run handles are judged with the oracle's process,
/// unit, control-group, and late-change evidence; unreadable
/// host evidence refuses by name in the no-handle case.
fn validation_liveness(
    cfg: &Cfg,
    config: &Value,
    record: &Value,
    validation_paths: &[PathBuf],
) -> Result<crate::remove_write::Liveness> {
    use crate::remove_write::Liveness;
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let selected = config
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let current_host = match host_id() {
        Ok(value) => value,
        Err(error) => {
            return Ok(Liveness {
                state: "unverifiable".to_string(),
                detail: liveness_diagnostic(&format!(
                    "validation-run evidence is unverifiable: {error}"
                )),
            })
        }
    };
    if machine != selected {
        return Ok(Liveness {
            state: "unverifiable".to_string(),
            detail: format!(
                "validation row is registered on machine {machine}, and run, process and user-systemd evidence is local to {selected}"
            ),
        });
    }
    let mut recorded: BTreeSet<String> = BTreeSet::new();
    if let Some(lease_host) = record
        .get("coordinator_lease")
        .and_then(|lease| lease.get("host_id"))
        .and_then(Value::as_str)
    {
        recorded.insert(lease_host.to_string());
    } else {
        recorded.insert(String::new());
    }
    if let Some(owner) = record.get("owner").filter(|owner| !owner.is_null()) {
        recorded.insert(
            owner
                .get("host_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        );
    }
    let local_error = if recorded.contains("") || recorded.len() != 1 {
        Some(format!(
            "validation row {machine}/{slot} has missing or mixed stable host identities"
        ))
    } else {
        let expected = recorded.iter().next().expect("one host identity");
        if *expected != current_host {
            Some(format!(
                "validation row {machine}/{slot} belongs to stable host {expected}, not current host {current_host}"
            ))
        } else {
            None
        }
    };
    if let Some(error) = local_error {
        return Ok(Liveness {
            state: "unverifiable".to_string(),
            detail: format!(
                "{error}, and run, process and user-systemd evidence is local to this host"
            ),
        });
    }
    let row_paths = validation_row_paths(cfg, record);
    let unverifiable = |error: String| -> Liveness {
        Liveness {
            state: "unverifiable".to_string(),
            detail: liveness_diagnostic(&format!(
                "validation-run evidence is unverifiable: {error}"
            )),
        }
    };
    let alive = |message: String| -> Liveness {
        Liveness {
            state: "alive".to_string(),
            detail: liveness_diagnostic(&format!(
                "validation run may still use the row: {message}"
            )),
        }
    };
    let first = match retained_handles(cfg, &row_paths, validation_paths) {
        Ok(handles) => handles,
        Err(error) => return Ok(unverifiable(error)),
    };
    let earlier = first.clone();
    let settled = settle_retained_handles(&first);
    let first = if settled {
        match retained_handles(cfg, &row_paths, validation_paths) {
            Ok(handles) => handles,
            Err(error) => return Ok(unverifiable(error)),
        }
    } else {
        first
    };
    let (running, unreadable) = recorded_generations(&first);
    if let Err(error) = assert_host_process_view() {
        return Ok(unverifiable(error));
    }
    let processes_before = match process_observations() {
        Ok(observations) => observations,
        Err(error) => return Ok(unverifiable(error)),
    };
    let units = match systemd_units() {
        Ok(units) => units,
        Err(error) if !first.is_empty() => return Ok(unverifiable(error)),
        Err(error) => {
            return Err(format!(
                "audit validation-run host evidence stays with the Python edition for now: {error}"
            ))
        }
    };
    let later = match retained_handles(cfg, &row_paths, validation_paths) {
        Ok(handles) => handles,
        Err(error) => return Ok(unverifiable(error)),
    };
    let processes_after = match process_observations() {
        Ok(observations) => observations,
        Err(error) => return Ok(unverifiable(error)),
    };
    let effective_cgroups = effective_cgroup_paths(&processes_before, &processes_after);
    let bindings = merge_retained_handles(&[&earlier, &first, &later]);
    let mut member_counts: BTreeMap<String, i64> = BTreeMap::new();
    for handle in &bindings {
        if member_counts.contains_key(&handle.unit) {
            continue;
        }
        match retained_unit_member_count(&handle.unit) {
            Ok(count) => {
                member_counts.insert(handle.unit.clone(), count);
            }
            Err(error) => return Ok(unverifiable(error)),
        }
    }
    // A recorded generation that was running before the host
    // evidence, or is running now, is a run that may use the row.
    for handle in &bindings {
        let Some(generation) = handle.generation() else {
            continue;
        };
        if running.contains(&generation) {
            let live = crate::status_state::process_start_ticks(generation.1)
                .is_ok_and(|ticks| ticks == Some(generation.2));
            return Ok(alive(format!(
                "retained validation handle {} {} for row {slot}",
                handle.path.display(),
                if live {
                    format!("has live exact process generation {}", generation.1)
                } else {
                    format!(
                        "records process generation {}, which ran just before the host evidence was read",
                        generation.1
                    )
                }
            )));
        }
    }
    if bindings.iter().any(|handle| handle.generation().is_some()) {
        let current_boot = match crate::status_state::boot_id() {
            Ok(boot) => boot,
            Err(error) => return Ok(unverifiable(error)),
        };
        for handle in &bindings {
            let Some(generation) = handle.generation() else {
                continue;
            };
            if let Some(error) = unreadable.get(&generation) {
                return Ok(unverifiable(error.clone()));
            }
            if generation.0 != current_boot {
                continue;
            }
            match crate::status_state::process_start_ticks(generation.1) {
                Ok(Some(ticks)) if ticks == generation.2 => {
                    return Ok(alive(format!(
                        "retained validation handle {} has live exact process generation {} for row {slot}",
                        handle.path.display(),
                        generation.1
                    )));
                }
                Ok(_) => {}
                Err(error) => return Ok(unverifiable(error)),
            }
        }
    }
    let cgroup_paths = effective_cgroups;
    for handle in &bindings {
        if cgroup_paths
            .iter()
            .any(|path| path.split('/').any(|part| part == handle.unit))
        {
            return Ok(alive(format!(
                "retained validation unit {} still has a live cgroup process for row {slot}",
                handle.unit
            )));
        }
        if member_counts.get(&handle.unit).copied().unwrap_or(0) != 0 {
            return Ok(alive(format!(
                "retained validation unit {} control group now holds {} process(es) for row {slot}",
                handle.unit,
                member_counts.get(&handle.unit).copied().unwrap_or(0)
            )));
        }
        for unit in units.iter().filter(|unit| unit.id == handle.unit) {
            let active = !matches!(unit.active_state.as_str(), "inactive" | "failed");
            if active || unit.job_id != 0 {
                return Ok(alive(format!(
                    "retained validation unit {} may still use row {slot}",
                    handle.unit
                )));
            }
            match unit_control_group(unit) {
                Ok(Some(control_group)) => {
                    let control_group = control_group.trim_end_matches('/');
                    if !control_group.is_empty()
                        && cgroup_paths
                            .iter()
                            .any(|path| cgroup_matches(control_group, path))
                    {
                        return Ok(alive(format!(
                            "retained validation unit {} still has a live process for row {slot}",
                            handle.unit
                        )));
                    }
                }
                Ok(None) => {}
                Err(error) if !bindings.is_empty() => return Ok(unverifiable(error)),
                Err(error) => {
                    return Err(format!("audit validation-run host evidence stays with the Python edition for now: {error}"))
                }
            }
        }
    }
    for unit in &units {
        let active = !matches!(unit.active_state.as_str(), "inactive" | "failed");
        if !active && unit.job_id == 0 {
            continue;
        }
        let text = match property_text(unit) {
            Ok(text) => text,
            Err(error) if !bindings.is_empty() => return Ok(unverifiable(error)),
            Err(error) => {
                return Err(format!(
                "audit validation-run host evidence stays with the Python edition for now: {error}"
            ))
            }
        };
        if let Some(named) = unit_names_path(&text, &row_paths) {
            return Ok(alive(format!(
                "user-systemd unit {} names validation row {slot} path {}",
                unit.id,
                named.display()
            )));
        }
    }
    let mut changes =
        retained_handle_changes(&first, &later, true, "while the host evidence was read");
    if settled {
        changes.extend(retained_handle_changes(
            &earlier,
            &first,
            false,
            "while their timestamp window was waited out",
        ));
    }
    if let Some((handle, change)) = changes.into_iter().next() {
        return Ok(alive(format!(
            "retained validation handle {} for row {slot} {change}, so its unit {} may have been queued after the user-systemd enumerations",
            handle.path.display(),
            handle.unit
        )));
    }
    Ok(Liveness {
        state: "dead".to_string(),
        detail: format!(
            "{} retained run handle(s) name this row, each with an inactive, unqueued unit with no live process and any recorded process generation dead since before the host evidence was read; no active or queued user-systemd unit names its paths",
            bindings.len()
        ),
    })
}

#[allow(clippy::too_many_arguments)]
fn audit_record(
    root: Option<&Path>,
    cfg: &Cfg,
    config: &Value,
    record: &Value,
    events: &[Value],
    journals: &BTreeMap<String, JournalSlot>,
    strays: &BTreeMap<(String, String), Vec<String>>,
    sealed_generation: Option<i64>,
    validation_paths: &[PathBuf],
    measurement: &CacheMeasurement,
) -> Result<(Value, bool)> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent")
        .to_string();
    let machine = record
        .get("machine")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let owner_identity =
        crate::status_state::identity_from_value(record.get("owner").unwrap_or(&Value::Null));
    let (owner_state, owner_detail) = process_state(owner_identity.as_ref());
    let liveness = if slot_type == "validate" {
        validation_liveness(cfg, config, record, validation_paths)?
    } else {
        liveness_answer(cfg, config, record)?
    };
    let (heartbeat_age, heartbeat_expired) = heartbeat_diagnosis(record)?;
    let mut agent_running = !(liveness.state == "dead" && owner_state == "dead");
    let hold = current_hold(root, cfg, &machine, &slot)?;
    let (release, void_reason) = release_state(cfg, record, events)?;
    let release_honoured = release.is_some()
        && void_reason.is_none()
        && matches!(owner_state.as_str(), "live" | "dead")
        && record.get("owner").is_some_and(|owner| !owner.is_null());
    let release_fields = match &release {
        Some(release) => release_obj(release, void_reason.as_deref()),
        None => Value::Null,
    };
    let slot_path = slot_directory(cfg, &slot, &slot_type)?;
    let slot_absent = !slot_path.exists() && !slot_path.is_symlink();
    let (cache_bytes_value, cache_status, cache_error) = if slot_absent {
        (json!(0), "complete", None)
    } else {
        match measurement.status {
            "complete" => (json!(measurement.bytes.unwrap_or(0)), "complete", None),
            "partial" => (Value::Null, "partial", None),
            _ => (Value::Null, "error", measurement.error.clone()),
        }
    };
    let record_sha256 = record_digest(record)?;
    let base = |verdict: &str, reasons: Vec<String>| -> Value {
        let mut row = json!({
            "slot": slot,
            "machine": machine,
            "agent": record.get("agent").cloned().unwrap_or(Value::Null),
            "slot_type": slot_type,
            "generation": generation,
            "record_sha256": record_sha256,
            "verdict": verdict,
            "reasons": reasons,
            "owner_state": owner_state,
            "liveness_state": liveness.state,
            "heartbeat_age_seconds": heartbeat_age,
            "heartbeat_expired": heartbeat_expired,
            "cache_bytes": cache_bytes_value.clone(),
            "cache_error": cache_error.clone(),
            "cache_status": cache_status,
        });
        if let Some(release) = &release {
            let _ = release;
            row.as_object_mut()
                .expect("row object")
                .insert("owner_release".to_string(), release_fields.clone());
        }
        row
    };
    if !hold.is_null() {
        let reason = hold
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let mut row = base("HELD", vec![reason]);
        apply_overrides(
            &mut row,
            journals,
            strays,
            &slot,
            &slot_type,
            slot_absent,
            sealed_generation,
        );
        return Ok((row, agent_running));
    }
    let mut reasons: Vec<String> = Vec::new();
    let mut removal_only_reasons: Vec<String> = Vec::new();
    if !release_honoured {
        if !heartbeat_expired {
            let ttl = record
                .get("heartbeat_ttl_seconds")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            reasons.push(format!(
                "heartbeat age {heartbeat_age}s has not exceeded the {ttl}s time-to-live"
            ));
        }
        if liveness.state != "dead" {
            reasons.push(format!(
                "liveness is {}: {}",
                liveness.state, liveness.detail
            ));
        }
        if record.get("owner").is_none_or(|owner| owner.is_null()) {
            reasons
                .push("no owner process generation is recorded, so death is unknown".to_string());
            removal_only_reasons.push(reasons.last().expect("just pushed").clone());
        } else if owner_state != "dead" {
            reasons.push(format!("recorded owner is {owner_state}: {owner_detail}"));
        }
    }
    if !slot_absent {
        if let Err(error) = crate::finish_write::assert_slot_contents(cfg, record) {
            reasons.push(error);
        } else {
            if let Some(reason) = handoff_read_reason(cfg, record, events, &slot_path) {
                reasons.push(reason);
            }
            let checkouts = record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for checkout in &checkouts {
                let name = checkout
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let raw_path = checkout
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let raw_repo = checkout
                    .get("repository")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let outcome = (|| -> Result<()> {
                    let path = stored_path(cfg, raw_path, "checkout path")?;
                    let (_rel, repository) = stored_repository_path(cfg, raw_repo)?;
                    verify_existing_worktree(&repository, &path)?;
                    let branch = {
                        let output = std::process::Command::new("git")
                            .args([
                                "-C",
                                &path.to_string_lossy(),
                                "symbolic-ref",
                                "--quiet",
                                "--short",
                                "HEAD",
                            ])
                            .output()
                            .map_err(|error| format!("cannot execute Git: {error}"))?;
                        if !output.status.success() {
                            return Err(format!("checkout {name} branch changed"));
                        }
                        String::from_utf8_lossy(&output.stdout).trim().to_string()
                    };
                    if branch
                        != checkout
                            .get("branch")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    {
                        return Err(format!("checkout {name} branch changed"));
                    }
                    let (authority, _url) = crate::finish_write::remote_authority(
                        &path,
                        checkout
                            .get("remote")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                    )?;
                    if authority
                        != checkout
                            .get("remote_url_sha256")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    {
                        return Err(format!("checkout {name} remote URL changed"));
                    }
                    Ok(())
                })();
                if let Err(error) = outcome {
                    reasons.push(error);
                }
            }
        }
    }
    if slot_absent {
        reasons.retain(|reason| !removal_only_reasons.contains(reason));
    }
    if slot_absent && release_honoured && owner_state != "dead" {
        reasons.push(format!(
            "recorded owner is {owner_state}: {owner_detail}; the recover-absent commands require the recorded owner generation to be dead"
        ));
    }
    if (release_honoured
        || (heartbeat_expired && liveness.state == "dead" && owner_state == "dead"))
        && reasons.is_empty()
        && !slot_absent
    {
        if let Err(error) = assert_slot_unused(&slot_path, std::process::id() as i64) {
            reasons.push(error);
            agent_running = true;
        }
    }
    if slot_absent {
        let row_digest = record_digest(record)?;
        let plan = if slot_type == "validate" {
            format!(
                "wrkslots recover-absent-validate-rows --input FILE', where FILE holds '{{\"rows\": [{{\"generation\": {generation}, \"machine\": \"{machine}\", \"record_sha256\": \"{row_digest}\", \"slot\": \"{slot}\"}}], \"schema\": 1}}"
            )
        } else {
            format!("wrkslots recover-absent-agent-rows --row {slot}={generation}={row_digest}")
        };
        reasons.push(format!(
            "slot directory is absent: {}; nothing occupies the path, so this row is retired by recovery, not removal: on machine {machine}, run '{plan}' for the read-only plan, then repeat it with '--apply --coordinator-authorized --coordinator-pid <coordinator-pid>'",
            slot_path.display()
        ));
    }
    if let Some(error) = &cache_error {
        if !slot_absent {
            reasons.push(format!("cache inspection failed: {error}"));
        }
    }
    if cache_status == "partial" && !slot_absent {
        reasons.push("cache inspection incomplete: bounded census is partial".to_string());
    }
    let verdict = if slot_absent {
        if reasons.len() == 1 {
            "RECOVERABLE"
        } else {
            "BLOCKED"
        }
    } else if reasons.is_empty() {
        "DELETABLE"
    } else {
        "BLOCKED"
    };
    let mut row = base(verdict, reasons);
    apply_overrides(
        &mut row,
        journals,
        strays,
        &slot,
        &slot_type,
        slot_absent,
        sealed_generation,
    );
    Ok((row, agent_running))
}

type SealTargets = Vec<(String, i64, PathBuf)>;
type SealErrors = Vec<(Option<String>, String, String)>;

struct SealTarget {
    machine: String,
    slot: String,
    generation: i64,
    path: PathBuf,
}

/// The machine a seal journal filename names, when it matches the
/// oracle's `VALIDATE-BATCH-SEAL.<machine>.journal` pattern.
fn seal_machine_from_name(name: &str) -> Option<String> {
    let middle = name
        .strip_prefix("VALIDATE-BATCH-SEAL.")?
        .strip_suffix(".journal")?;
    if middle.is_empty() || middle.len() > 64 {
        return None;
    }
    let mut chars = middle.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !middle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return None;
    }
    Some(middle.to_string())
}

fn seal_actor(value: &Value, label: &str) -> Result<()> {
    if value.is_null() {
        return Err("validation-batch seal journal has no actor".to_string());
    }
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["pid", "start_ticks", "boot_id", "host_id", "cgroup_path"],
        &[],
        label,
    )?;
    as_int(&raw["pid"], &format!("{label}.pid"), 1)?;
    as_int(&raw["start_ticks"], &format!("{label}.start_ticks"), 1)?;
    let boot_id = as_str(&raw["boot_id"], &format!("{label}.boot_id"))?;
    let host_id = as_str(&raw["host_id"], &format!("{label}.host_id"))?;
    let cgroup_path = as_str(&raw["cgroup_path"], &format!("{label}.cgroup_path"))?;
    if boot_id.is_empty() || host_id.is_empty() || !cgroup_path.starts_with('/') {
        return Err(format!(
            "{label} has an invalid boot, machine, or cgroup identity"
        ));
    }
    Ok(())
}

fn cleanup_identity(value: &Value, label: &str) -> Result<()> {
    let values = as_list(value, label)?;
    if values.len() == 3 {
        for (index, item) in values.iter().enumerate() {
            as_int(item, &format!("{label}[{index}]"), 1)?;
        }
        return Ok(());
    }
    if values.len() != 4 {
        return Err(format!(
            "{label} must contain dev, ino, file-handle type, and file-handle bytes"
        ));
    }
    as_int(&values[0], &format!("{label}[0]"), 1)?;
    as_int(&values[1], &format!("{label}[1]"), 1)?;
    as_int(&values[2], &format!("{label}[2]"), 0)?;
    let handle = as_str(&values[3], &format!("{label}[3]"))?;
    if handle.is_empty()
        || handle.len() > 256
        || handle.len() % 2 != 0
        || !handle
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
    {
        return Err(format!(
            "{label}[3] must be bounded lowercase hexadecimal bytes"
        ));
    }
    Ok(())
}

fn cleanup_target(cfg: &Cfg, value: &Value, label: &str) -> Result<(String, i64, PathBuf)> {
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &["slot", "generation", "path", "original_mode", "identity"],
        &[],
        label,
    )?;
    let slot = validate_name(as_str(&raw["slot"], &format!("{label}.slot"))?, "slot")?;
    let generation = as_int(&raw["generation"], &format!("{label}.generation"), 1)?;
    let (_relative, path) = relative_inside(
        &cfg.root,
        as_str(&raw["path"], &format!("{label}.path"))?,
        &format!("{label}.path"),
    )?;
    if path != slot_directory(cfg, &slot, "validate")? {
        return Err(format!(
            "{label}.path does not match validation slot {slot}"
        ));
    }
    let original_mode = as_int(&raw["original_mode"], &format!("{label}.original_mode"), 0)?;
    if original_mode > 0o7777 {
        return Err(format!("{label}.original_mode exceeds permission bits"));
    }
    cleanup_identity(&raw["identity"], &format!("{label}.identity"))?;
    Ok((slot, generation, path))
}

/// One seal journal, validated like `_validate_batch_seal_journal`.
fn seal_journal(cfg: &Cfg, path: &Path, value: &Value) -> Result<(String, SealTargets)> {
    let label = "validation-batch seal journal";
    let raw = as_mapping(value, label)?;
    exact_keys(
        raw,
        &[
            "schema",
            "kind",
            "machine",
            "created_at",
            "actor",
            "targets",
        ],
        &[],
        label,
    )?;
    if as_int(&raw["schema"], &format!("{label}.schema"), 0)? != 1 {
        return Err("unsupported validation-batch seal journal schema".to_string());
    }
    if raw["kind"] != Value::String("validate-batch-seal".to_string()) {
        return Err("validation-batch seal journal has the wrong operation kind".to_string());
    }
    let machine = validate_name(
        as_str(&raw["machine"], &format!("{label}.machine"))?,
        "machine",
    )?;
    let expected = cfg
        .control
        .join(format!("VALIDATE-BATCH-SEAL.{machine}.journal"));
    if path != expected {
        return Err(
            "validation-batch seal journal filename does not match its machine".to_string(),
        );
    }
    parse_timestamp(
        as_str(&raw["created_at"], &format!("{label}.created_at"))?,
        &format!("{label}.created_at"),
    )?;
    seal_actor(&raw["actor"], &format!("{label}.actor"))?;
    let items = as_list(&raw["targets"], &format!("{label}.targets"))?;
    let mut targets: Vec<(String, i64, PathBuf)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        targets.push(cleanup_target(
            cfg,
            item,
            &format!("{label}.targets[{index}]"),
        )?);
    }
    if targets.len() > 8 {
        return Err("validation-batch seal journal exceeds the batch limit".to_string());
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    if targets
        .iter()
        .any(|(slot, _generation, _path)| !seen.insert(slot.as_str()))
    {
        return Err("validation-batch seal journal repeats a slot".to_string());
    }
    Ok((machine, targets))
}

fn read_seal_json(path: &Path) -> Result<Value> {
    refuse_symlink(path, "validation-batch seal journal")?;
    let text = std::fs::read_to_string(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            format!("missing validation-batch seal journal: {}", path.display())
        } else {
            format!(
                "cannot read validation-batch seal journal {}: {error}",
                path.display()
            )
        }
    })?;
    serde_json::from_str(&text)
        .map_err(|error| format!("validation-batch seal journal is malformed JSON: {error}"))
}

/// Seal targets and per-journal errors, in the oracle's order: one
/// bad journal hides neither the other journals nor the rows.
fn seal_evidence(cfg: &Cfg) -> (Vec<SealTarget>, SealErrors) {
    let mut targets: Vec<SealTarget> = Vec::new();
    let mut errors: Vec<(Option<String>, String, String)> = Vec::new();
    for path in crate::heartbeat_write::seal_paths(cfg) {
        let name = path
            .file_name()
            .map(|item| item.to_string_lossy().into_owned())
            .unwrap_or_default();
        let machine = seal_machine_from_name(&name);
        let parsed = read_seal_json(&path).and_then(|value| seal_journal(cfg, &path, &value));
        match parsed {
            Ok((parsed_machine, parsed_targets)) => {
                targets.extend(parsed_targets.into_iter().map(
                    |(slot, generation, target_path)| SealTarget {
                        machine: parsed_machine.clone(),
                        slot,
                        generation,
                        path: target_path,
                    },
                ));
            }
            Err(error) => errors.push((machine, name, error)),
        }
    }
    (targets, errors)
}

/// The audit payload (`{"schema": 2, ...}`), minus the metrics phase
/// timings the oracle measures live.
pub(crate) fn parse_audit(
    root: Option<&Path>,
    machine_override: Option<&str>,
    census: CensusRequest,
) -> Result<Value> {
    if census.work_limit < 1 {
        return Err("--cache-work-limit must be positive".to_string());
    }
    if !census.wall_seconds.is_finite() || census.wall_seconds <= 0.0 {
        return Err("--cache-wall-seconds must be finite and positive".to_string());
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    validate_global(root, &cfg)?;
    let journals = journal_slots(&cfg)?;
    let machines = discover_machines(&cfg.control)?;
    let mut records: Vec<Value> = Vec::new();
    let mut shard_revisions: Vec<(String, i64)> = Vec::new();
    let mut events_by_machine: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for machine in &machines {
        let document = active_document(root, &cfg, machine)?;
        shard_revisions.push((
            machine.clone(),
            document
                .get("revision")
                .and_then(Value::as_i64)
                .unwrap_or(0),
        ));
        if let Some(slots) = document.get("slots").and_then(Value::as_array) {
            records.extend(slots.iter().cloned());
        }
        let directory = cfg.control.join(format!("EVENTS.{machine}"));
        let events = if directory.is_dir() {
            load_event_chain(&directory, machine)?
        } else {
            Vec::new()
        };
        events_by_machine.insert(machine.clone(), events);
    }
    let (seal_targets, seal_errors) = seal_evidence(&cfg);
    let mut seal_by_slot: BTreeMap<(String, String), (i64, PathBuf)> = BTreeMap::new();
    for target in &seal_targets {
        seal_by_slot.insert(
            (target.machine.clone(), target.slot.clone()),
            (target.generation, target.path.clone()),
        );
    }
    let mut represented_seals: BTreeSet<(String, String, i64)> = BTreeSet::new();
    // Stray Git registrations inside the managed root.
    let mut strays: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let git_doc = crate::git_storage_state::parse_storage_git(root)?;
    if let Some(findings) = git_doc.get("findings").and_then(Value::as_array) {
        for finding in findings {
            if finding.get("kind").and_then(Value::as_str) == Some("git-registration-without-row") {
                let key = (
                    finding
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or("agent")
                        .to_string(),
                    finding
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
                strays.entry(key).or_default().push(
                    finding
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
        }
    }
    // On-disk slot directories.
    let validate_root = crate::storage_state::managed_root(&cfg).join("validate");
    let mut on_disk: BTreeSet<(String, String)> = BTreeSet::new();
    for (slot_type, root_path) in [
        ("agent", cfg.worktrees.clone()),
        ("validate", validate_root.clone()),
    ] {
        if !root_path.is_dir() {
            continue;
        }
        let listing = std::fs::read_dir(&root_path).map_err(|error| {
            format!(
                "cannot enumerate worktree slots in {}: {error}",
                root_path.display()
            )
        })?;
        for entry in listing {
            let entry =
                entry.map_err(|error| format!("cannot enumerate worktree slots: {error}"))?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("EVENTS.") {
                continue;
            }
            // The validate-slots directory itself is not an agent slot.
            if slot_type == "agent" && path == validate_root {
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                on_disk.insert((slot_type.to_string(), name));
            }
        }
    }
    let registered: BTreeSet<(String, String)> = records
        .iter()
        .map(|record| {
            (
                record
                    .get("slot_type")
                    .and_then(Value::as_str)
                    .unwrap_or("agent")
                    .to_string(),
                record
                    .get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect();
    let validation_paths: Vec<PathBuf> = records
        .iter()
        .filter(|record| record.get("slot_type").and_then(Value::as_str) == Some("validate"))
        .flat_map(|record| validation_row_paths(&cfg, record))
        .collect();
    let plan_mounts = read_plan_mounts();
    // Planning targets in the oracle's order: every record, then
    // unregistered agent slots. Each target plans with a fair
    // share of what earlier targets left of the work and wall
    // allowances. A slot with an interrupted journal plans from
    // its journal in the oracle; that stays Python-owned, as does
    // an unregistered validation directory (no cache subject).
    // Target positions follow the oracle: records first, then
    // unregistered agent slots in sorted order. A journal slot
    // occupies its position (and so shrinks its peers' shares)
    // even though its own planning stays Python-owned.
    let mut target_actions: Vec<Option<(String, String)>> = Vec::new();
    for (slot_type, slot) in on_disk.difference(&registered) {
        if slot_type != "agent" || journals.contains_key(slot) {
            if slot_type == "agent" {
                target_actions.push(None);
            }
            continue;
        }
        target_actions.push(Some((slot_type.clone(), slot.clone())));
    }
    let target_count = records.len() + target_actions.len();
    let mut census_subjects: Vec<CensusSubject> = Vec::new();
    let mut unregistered_subjects: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut plan_work_consumed: i64 = 0;
    let mut plan_seconds = census.wall_seconds;
    let make_budget = |position: usize, consumed: i64, seconds_left: f64| PlanBudget {
        work_limit: (census.work_limit - consumed) / (target_count - position) as i64,
        work_consumed: 0,
        seconds: seconds_left / (target_count - position) as f64,
        wall_total: census.wall_seconds,
        spent: 0.0,
    };
    for (position, record) in records.iter().enumerate() {
        let mut budget = make_budget(position, plan_work_consumed, plan_seconds);
        let (roots, error) = cache_roots(&cfg, &config, record, &mut budget, &plan_mounts);
        plan_work_consumed += budget.work_consumed;
        plan_seconds = (plan_seconds - budget.spent).max(0.0);
        let validation = record.get("slot_type").and_then(Value::as_str) == Some("validate");
        census_subjects.push((roots, error, validation));
    }
    for (offset, action) in target_actions.iter().enumerate() {
        let Some((slot_type, slot)) = action else {
            continue;
        };
        let position = records.len() + offset;
        let mut budget = make_budget(position, plan_work_consumed, plan_seconds);
        let subject = unregistered_cache_subject(&cfg, &config, slot, &mut budget, &plan_mounts);
        plan_work_consumed += budget.work_consumed;
        plan_seconds = (plan_seconds - budget.spent).max(0.0);
        unregistered_subjects.insert((slot_type.clone(), slot.clone()), census_subjects.len());
        census_subjects.push(subject);
    }
    let registry_revision = {
        use sha2::Digest as _;
        let mut shards = shard_revisions.clone();
        shards.sort();
        let identity: Vec<Value> = shards
            .iter()
            .map(|(machine, revision)| json!({"machine": machine, "revision": revision}))
            .collect();
        let digest = sha2::Sha256::digest(
            serde_json::to_string(&Value::Array(identity))
                .unwrap_or_default()
                .as_bytes(),
        );
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let census_measurements = cache_census(&cfg, &census_subjects, &census, &registry_revision)?;
    let mut rows: Vec<Value> = Vec::new();
    let mut running_agent_count: i64 = 0;
    for (index, record) in records.iter().enumerate() {
        let machine = record
            .get("machine")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = record
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let sealed = seal_by_slot.get(&(machine.to_string(), slot.to_string()));
        let events = events_by_machine.get(machine).cloned().unwrap_or_default();
        let (row, running) = audit_record(
            root,
            &cfg,
            &config,
            record,
            &events,
            &journals,
            &strays,
            sealed.map(|(generation, _path)| *generation),
            &validation_paths,
            &census_measurements[index],
        )?;
        running_agent_count += i64::from(running);
        if let Some((generation, _path)) = sealed {
            represented_seals.insert((machine.to_string(), slot.to_string(), *generation));
        }
        rows.push(row);
    }
    for (slot_type, slot) in on_disk.difference(&registered) {
        let journal = journals.get(slot);
        let sealed = seal_by_slot.get(&(selected.clone(), slot.clone()));
        let (reason, owner_state) = if let Some(journal) = journal {
            (
                format!(
                    "interrupted {} requires wrkslots recover",
                    journal.state.replace('-', " ")
                ),
                journal.state.clone(),
            )
        } else if sealed.is_some() {
            (
                "interrupted validation-batch seal requires wrkslots recover".to_string(),
                "validation-batch-seal".to_string(),
            )
        } else if slot_type == "validate" {
            (
                "validation directory has no active registry row; preserve and inspect it, and use import-existing only for a demonstrably live owner; destructive ownerless Git worktree recovery is disabled".to_string(),
                "unregistered".to_string(),
            )
        } else {
            (
                "directory has no active registry row; inspect or import it".to_string(),
                "unregistered".to_string(),
            )
        };
        let (machine_value, generation_value) = if let Some(journal) = journal {
            (json!(journal.machine), Value::Null)
        } else if let Some((generation, _path)) = sealed {
            (json!(selected), json!(generation))
        } else {
            (Value::Null, Value::Null)
        };
        // A validation directory is not a cache subject, so the
        // oracle's census reports it complete with no bytes. An
        // agent slot's caches were planned as a census subject;
        // a journal slot's planning stays with the Python edition.
        let (cache_bytes, cache_error, cache_status) = if slot_type == "validate" {
            (json!(0), Value::Null, "complete")
        } else if let Some(measurement) = unregistered_subjects
            .get(&(slot_type.clone(), slot.clone()))
            .map(|index| &census_measurements[*index])
        {
            match measurement.status {
                "complete" => (
                    json!(measurement.bytes.unwrap_or(0)),
                    Value::Null,
                    "complete",
                ),
                "partial" => (Value::Null, Value::Null, "partial"),
                _ => (
                    Value::Null,
                    measurement
                        .error
                        .clone()
                        .map_or(Value::Null, |error| json!(error)),
                    "error",
                ),
            }
        } else {
            (
                Value::Null,
                json!(format!(
                    "unregistered slot {slot} contains no Git checkouts"
                )),
                "error",
            )
        };
        rows.push(json!({
            "slot": slot,
            "slot_type": slot_type,
            "machine": machine_value,
            "agent": Value::Null,
            "generation": generation_value,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [reason],
            "owner_state": owner_state,
            "liveness_state": "unverifiable",
            "cache_bytes": cache_bytes,
            "cache_error": cache_error,
            "cache_status": cache_status,
        }));
        if let Some((generation, _path)) = sealed {
            represented_seals.insert((selected.clone(), slot.clone(), *generation));
        }
    }
    for journal in journals.values() {
        let known = registered.iter().any(|(_type, slot)| slot == &journal.slot)
            || on_disk.iter().any(|(_type, slot)| slot == &journal.slot);
        if known {
            continue;
        }
        rows.push(json!({
            "slot": journal.slot,
            "slot_type": Value::Null,
            "machine": journal.machine,
            "agent": Value::Null,
            "generation": Value::Null,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [format!(
                "interrupted {} requires wrkslots recover",
                journal.state.replace('-', " ")
            )],
            "owner_state": journal.state,
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    for target in &seal_targets {
        if represented_seals.contains(&(
            target.machine.clone(),
            target.slot.clone(),
            target.generation,
        )) {
            continue;
        }
        rows.push(json!({
            "slot": target.slot,
            "slot_type": "validate",
            "machine": target.machine,
            "agent": Value::Null,
            "generation": target.generation,
            "record_sha256": Value::Null,
            "verdict": "BLOCKED",
            "reasons": [format!(
                "interrupted validation-batch seal requires wrkslots recover; recorded path={}",
                target.path.display()
            )],
            "owner_state": "validation-batch-seal",
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    for (machine, journal_name, error) in &seal_errors {
        rows.push(json!({
            "slot": journal_name,
            "slot_type": "validate",
            "machine": machine,
            "agent": Value::Null,
            "generation": Value::Null,
            "record_sha256": Value::Null,
            "verdict": "UNKNOWN",
            "reasons": [format!(
                "validation-batch seal evidence is unreadable: {error}"
            )],
            "owner_state": "validation-batch-seal",
            "liveness_state": "unverifiable",
            "cache_bytes": 0,
            "cache_error": Value::Null,
            "cache_status": "complete",
        }));
    }
    rows.sort_by(|a, b| {
        a.get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(b.get("slot").and_then(Value::as_str).unwrap_or_default())
    });
    let worktree_count = on_disk.len();
    let disk = disk_status(&cfg, &config);
    let attention: Vec<String> = first_seen_names(
        rows.iter()
            .filter(|row| {
                row.get("verdict").and_then(Value::as_str) == Some("DELETABLE")
                    || row.get("verdict").and_then(Value::as_str) == Some("RECOVERABLE")
                    || row.get("owner_state").and_then(Value::as_str) == Some("unregistered")
                    || row
                        .get("reasons")
                        .and_then(Value::as_array)
                        .is_some_and(|reasons| {
                            reasons.iter().any(|reason| {
                                reason
                                    .as_str()
                                    .is_some_and(|text| text.starts_with("interrupted "))
                            })
                        })
            })
            .map(|row| {
                row.get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    );
    let attention_set: BTreeSet<&String> = attention.iter().collect();
    let unknown: Vec<String> = first_seen_names(
        rows.iter()
            .filter(|row| {
                let verdict = row
                    .get("verdict")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                verdict == "UNKNOWN"
                    || (verdict == "BLOCKED"
                        && row.get("heartbeat_expired").and_then(Value::as_bool) == Some(true)
                        && (!matches!(
                            row.get("owner_state").and_then(Value::as_str),
                            Some("live" | "dead")
                        ) || !matches!(
                            row.get("liveness_state").and_then(Value::as_str),
                            Some("alive" | "dead")
                        ) || !row.get("cache_error").is_some_and(|value| value.is_null())
                            || row.get("cache_status").and_then(Value::as_str) == Some("partial")
                            || row.get("reasons").and_then(Value::as_array).is_some_and(
                                |reasons| {
                                    reasons.iter().any(|reason| {
                                        reason.as_str().is_some_and(|text| {
                                            let lower = text.to_lowercase();
                                            ["cannot", "failed", "unavailable", "unknown"]
                                                .iter()
                                                .any(|token| lower.contains(token))
                                        })
                                    })
                                },
                            )))
            })
            .map(|row| {
                row.get("slot")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect(),
    )
    .into_iter()
    .filter(|name| !attention_set.contains(name))
    .collect();
    let (gate_state, summary) = if !attention.is_empty() {
        (
            "actionable",
            format!(
                "{} worktree slot(s) need coordinator attention",
                attention.len()
            ),
        )
    } else if !unknown.is_empty() {
        (
            "unknown",
            format!(
                "could not determine reclaim state for {} worktree slot(s)",
                unknown.len()
            ),
        )
    } else {
        (
            "ok",
            "no worktree slot currently needs coordinator attention".to_string(),
        )
    };
    Ok(json!({
        "schema": 2,
        "worktree_count": worktree_count,
        "running_agent_count": running_agent_count,
        "leak": worktree_count as i64 > running_agent_count,
        "attention_count": attention.len(),
        "attention_slots": attention,
        "unknown_count": unknown.len(),
        "unknown_slots": unknown,
        "state": gate_state,
        "summary": summary,
        "remote_refs_refreshed": false,
        "disk": disk,
        "slots": rows,
    }))
}

fn first_seen_names(names: Vec<String>) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    for name in names {
        if seen.insert(name.clone()) {
            ordered.push(name);
        }
    }
    ordered
}

fn disk_status(cfg: &Cfg, config: &Value) -> Value {
    let free = free_bytes(&cfg.worktrees);
    let advisory = config.get("disk_advisory_bytes").and_then(Value::as_i64);
    let provisioning = config
        .get("disk_provisioning_floor_bytes")
        .and_then(Value::as_i64);
    let emergency = config.get("disk_emergency_bytes").and_then(Value::as_i64);
    let state = match (advisory, provisioning, emergency) {
        (Some(advisory), Some(provisioning), Some(emergency)) => {
            if free < emergency {
                "emergency"
            } else if free < provisioning {
                "provisioning-blocked"
            } else if free < advisory {
                "advisory"
            } else {
                "healthy"
            }
        }
        _ => "unconfigured",
    };
    json!({
        "free_bytes": free,
        "state": state,
        "advisory_bytes": advisory,
        "provisioning_floor_bytes": provisioning,
        "emergency_bytes": emergency,
    })
}

fn free_bytes(path: &Path) -> i64 {
    rustix::fs::statvfs(path)
        .map(|stat| (stat.f_bavail * stat.f_frsize) as i64)
        .unwrap_or(0)
}
