//! Writer: supersede a handoff-read retirement entry and rebind the
//! slot to its live agent (`supersede-handoff`).
//!
//! Reproduces the Python mutation behind the registry locks: the
//! coordinator authorization, the handoff-read candidate check, the
//! live-owner process-tree slot-use proof (cwd/root/exe links, one
//! descriptor, one mapping, mounts), the generation advance, and the
//! `handoff-superseded` active-state event. Fixture registries only
//! until the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, slot_directory};
use crate::adopt_write::validate_agent_name;
use crate::canonical::canonical_sha256;
use crate::config_validate::validate_name;
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state, require_fixture,
    utc_now,
};
use crate::global_state::load_event_chain;
use crate::heartbeat_write::{assert_caller, identity_value};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::proc_reads;
use crate::retirement_queue::latest_handoff_read;
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

fn parent_pid(pid: i64) -> Option<i64> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    fields.get(1).and_then(|value| value.parse().ok())
}

fn descendant_pids(root_pid: i64) -> Result<Vec<i64>> {
    let mut children: Vec<(i64, i64)> = Vec::new();
    let entries = std::fs::read_dir("/proc")
        .map_err(|error| format!("cannot enumerate host processes: {error}"))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        if let (Ok(pid), Some(ppid)) = (name.parse::<i64>(), parent_pid(name.parse().unwrap_or(0)))
        {
            children.push((ppid, pid));
        }
    }
    let mut found: Vec<i64> = vec![root_pid];
    let mut queue: Vec<i64> = vec![root_pid];
    while let Some(current) = queue.pop() {
        if found.len() > 4096 {
            break;
        }
        for (ppid, pid) in &children {
            if *ppid == current {
                found.push(*pid);
                queue.push(*pid);
            }
        }
    }
    Ok(found)
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

/// Where the /proc link `path` of the process at `base` points, or None
/// without evidence, as the Python census decides: a link whose process
/// exited (ENOENT or ESRCH, or EACCES or EPERM once
/// `proc_reads::unreadable_entry_exited`; added to `gone`) or that is no
/// link (EINVAL) is no evidence; one that stat proves lies on another
/// filesystem device than the slot is not in it; an unreadable link of a
/// live process and every other failure refuse (#1173
/// census-vanished-process).
fn link_target(
    base: &Path,
    path: &Path,
    slot_device: Option<u64>,
    gone: &mut Vec<(PathBuf, std::io::Error)>,
) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::MetadataExt as _;
    let raw = match proc_reads::read_link(path) {
        Ok(raw) => raw,
        Err(error) if proc_reads::is_gone(&error) => {
            gone.push((path.to_path_buf(), error));
            return Ok(None);
        }
        Err(error) if matches!(error.raw_os_error(), Some(1 | 13)) => {
            if proc_reads::unreadable_entry_exited(&holder(base), base, path, &error) {
                gone.push((path.to_path_buf(), error));
                return Ok(None);
            }
            return Err(format!(
                "process use is indeterminate because {} is unreadable: {error}",
                path.display()
            ));
        }
        Err(error) if error.raw_os_error() == Some(22) => return Ok(None),
        Err(error) => {
            let observed = std::fs::metadata(path).ok().map(|metadata| metadata.dev());
            if slot_device.is_some() && observed.is_some() && observed != slot_device {
                return Ok(None);
            }
            return Err(format!(
                "process use is indeterminate because {} failed: {error}",
                path.display()
            ));
        }
    };
    let text = raw.to_string_lossy().into_owned();
    let text = text.strip_suffix(" (deleted)").unwrap_or(&text).to_string();
    let target = PathBuf::from(&text);
    if target.is_absolute() {
        Ok(Some(target))
    } else {
        Ok(Some(path.parent().unwrap_or(Path::new("/")).join(target)))
    }
}

/// How a census diagnostic names the process read at `base` (/proc/PID).
fn holder(base: &Path) -> String {
    format!(
        "process {}",
        base.file_name().unwrap_or_default().to_string_lossy()
    )
}

/// A failed read of one /proc/PID entry of the process at `base`: no
/// evidence once the process exited (ENOENT or ESRCH, or an unreadable
/// entry once `proc_reads::unreadable_entry_exited`; added to `gone`),
/// otherwise a refusal (#1173).
fn unread_process_entry(
    base: &Path,
    path: &Path,
    error: std::io::Error,
    gone: &mut Vec<(PathBuf, std::io::Error)>,
) -> Result<()> {
    let unreadable = error.kind() == std::io::ErrorKind::PermissionDenied;
    if proc_reads::is_gone(&error)
        || (unreadable && proc_reads::unreadable_entry_exited(&holder(base), base, path, &error))
    {
        gone.push((path.to_path_buf(), error));
        return Ok(());
    }
    if unreadable {
        return Err(format!(
            "process use is indeterminate because {} is unreadable: {error}",
            path.display()
        ));
    }
    Err(format!(
        "process use is indeterminate because {} cannot be read: {error}",
        path.display()
    ))
}

fn process_uses_slot(pid: i64, slot_path: &Path) -> Result<Vec<String>> {
    use std::os::unix::fs::MetadataExt as _;
    let pid_dir = PathBuf::from(format!("/proc/{pid}"));
    let slot_device = std::fs::metadata(slot_path)
        .ok()
        .map(|metadata| metadata.dev());
    // Reads that found the process exited: each is no evidence.
    let mut gone: Vec<(PathBuf, std::io::Error)> = Vec::new();
    let mut uses: Vec<String> = Vec::new();
    for name in ["cwd", "root", "exe"] {
        if let Some(target) = link_target(&pid_dir, &pid_dir.join(name), slot_device, &mut gone)? {
            if within(&target, slot_path) {
                uses.push(format!("{name}={}", target.display()));
            }
        }
    }
    let fd_dir = pid_dir.join("fd");
    match proc_reads::read_dir(&fd_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                if let Some(target) = link_target(&pid_dir, &entry.path(), slot_device, &mut gone)?
                {
                    if within(&target, slot_path) {
                        uses.push(format!(
                            "fd/{}={}",
                            entry.file_name().to_string_lossy(),
                            target.display()
                        ));
                        break;
                    }
                }
            }
        }
        Err(error) => unread_process_entry(&pid_dir, &fd_dir, error, &mut gone)?,
    }
    let maps_path = pid_dir.join("maps");
    let maps = match proc_reads::read_to_string(&maps_path) {
        Ok(maps) => maps,
        Err(error) => {
            unread_process_entry(&pid_dir, &maps_path, error, &mut gone)?;
            String::new()
        }
    };
    proc_reads::note_process_gone(&holder(&pid_dir), &pid_dir, &gone);
    let slot_text = slot_path.to_string_lossy();
    for line in maps.lines() {
        if !line.contains(slot_text.as_ref()) {
            continue;
        }
        let whitespace_fields: Vec<&str> = {
            let mut out: Vec<&str> = Vec::new();
            let mut rest = line;
            for _ in 0..5 {
                let trimmed = rest.trim_start();
                match trimmed.find(char::is_whitespace) {
                    Some(at) => {
                        out.push(&trimmed[..at]);
                        rest = &trimmed[at..];
                    }
                    None => {
                        out.push(trimmed);
                        rest = "";
                        break;
                    }
                }
            }
            out.push(rest.trim());
            out
        };
        if whitespace_fields.len() == 6 {
            let mapped = whitespace_fields[5]
                .strip_suffix(" (deleted)")
                .unwrap_or(whitespace_fields[5]);
            let mapped_path = Path::new(mapped);
            if mapped_path.is_absolute() && within(mapped_path, slot_path) {
                uses.push(format!("map={mapped}"));
                break;
            }
        }
    }
    if let Ok(mountinfo) = std::fs::read_to_string(pid_dir.join("mountinfo")) {
        for line in mountinfo.lines() {
            let Some((left, _right)) = line.split_once(" - ") else {
                continue;
            };
            let fields: Vec<&str> = left.split_whitespace().collect();
            for candidate in [fields.get(3), fields.get(4)].into_iter().flatten() {
                let decoded = candidate
                    .replace("\\040", " ")
                    .replace("\\011", "\t")
                    .replace("\\012", "\n")
                    .replace("\\134", "\\");
                let path = Path::new(&decoded);
                if within(path, slot_path) {
                    uses.push(format!("mount={decoded}"));
                    break;
                }
            }
            if uses.iter().any(|item| item.starts_with("mount=")) {
                break;
            }
        }
    }
    Ok(uses)
}

/// Supersede the named slot's handoff-read entry, printing the outcome.
#[allow(clippy::too_many_arguments)]
pub(crate) fn supersede_handoff(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    coordinator_pid: i64,
    coordinator_authorized: bool,
    reason: &str,
) -> Result<String> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    validate_agent_name(agent)?;
    if !coordinator_authorized {
        return Err("handoff supersession is coordinator-owned. state: REFUSED -- no worktree or lifecycle record was changed. remedy: ask the coordinator to perform the operation; a coordinator-delegated wrapper may rerun it with --coordinator-authorized".to_string());
    }
    if reason.trim().is_empty() {
        return Err("handoff supersession requires a non-empty --reason".to_string());
    }
    let coordinator = read_process_identity(coordinator_pid)?;
    assert_caller(&coordinator, "coordinator")?;
    let successor = read_process_identity(owner_pid)?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    let journals = parse_journals(root)?;
    if let Some(names) = journals.get("journals").and_then(Value::as_array) {
        if let Some(first) = names.first().and_then(Value::as_str) {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
            ));
        }
    }
    validate_global(root, &cfg)?;
    refuse_target_storage(root, slot)?;
    let active = active_document(root, &cfg, &selected)?;
    let record = active
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        })
        .cloned()
        .ok_or_else(|| format!("slot '{slot}' is not active on machine {selected}"))?;
    let generation = record
        .get("generation")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if generation != expected_generation {
        return Err(format!(
            "stale generation for slot {slot}: expected {expected_generation}, current {generation}"
        ));
    }
    if record.get("slot_type").and_then(Value::as_str) != Some("agent") {
        let slot_type = record
            .get("slot_type")
            .and_then(Value::as_str)
            .unwrap_or("agent");
        return Err(format!(
            "slot {slot} is a {slot_type} slot; supersede-handoff applies to agent slots"
        ));
    }
    let directory = cfg.control.join(format!("EVENTS.{selected}"));
    let events = load_event_chain(&directory, &selected)?;
    let latest = latest_handoff_read(&events, slot, generation)?;
    let Some(latest) = latest.filter(|read| read.enqueued) else {
        return Err(format!(
            "slot {slot} is not a handoff-read retirement candidate; there is no handoff-read entry to supersede"
        ));
    };
    let slot_dir = slot_directory(&cfg, slot, "agent")?;
    let mut slot_use: Option<String> = None;
    for pid in descendant_pids(successor.pid)? {
        let found = process_uses_slot(pid, &slot_dir)?;
        if let Some(first) = found.into_iter().next() {
            slot_use = Some(format!("pid {pid}: {first}"));
            break;
        }
    }
    let Some(slot_use) = slot_use else {
        return Err(format!(
            "no process in new owner PID {}'s tree uses slot {slot} ({}); supersede-handoff requires a live owner that is using the slot. remedy: have the new owner's lane run with a working directory or open file inside the slot, then rerun",
            successor.pid,
            slot_dir.display()
        ));
    };
    if read_process_identity(successor.pid)? != successor {
        return Err("owner process generation changed during supersede-handoff".to_string());
    }
    let released = events.iter().any(|event| {
        if event.get("kind").and_then(Value::as_str) != Some("owner-released") {
            return false;
        }
        let payload = event.get("payload").cloned().unwrap_or(Value::Null);
        payload.get("slot").and_then(Value::as_str) == Some(slot)
            && payload.get("generation").and_then(Value::as_i64) == Some(generation)
    });
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    let previous_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let previous_owner = record.get("owner").cloned().unwrap_or(Value::Null);
    let mut superseded = record.clone();
    if let Some(object) = superseded.as_object_mut() {
        object.insert("agent".to_string(), json!(agent));
        object.insert("owner".to_string(), identity_value(&successor));
        object.insert("generation".to_string(), json!(generation + 1));
        object.insert("heartbeat_at".to_string(), json!(utc_now()));
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "handoff-superseded",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": superseded,
            "evidence": {
                "agent": agent,
                "coordinator": identity_value(&coordinator),
                "owner": identity_value(&successor),
                "previous_agent": previous_agent,
                "previous_owner": previous_owner,
                "reason": reason,
                "slot_use": slot_use,
                "superseded_handoff_read_sequence": latest.sequence,
                "superseded_handoff_sha256": latest.sha256,
                "voided_owner_release": released,
            },
        }),
    )?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = superseded.clone();
        }
    }
    let mut document = active.clone();
    if let Some(object) = document.as_object_mut() {
        object.insert("revision".to_string(), json!(previous_revision + 1));
        object.insert("slots".to_string(), json!(slots));
    }
    let active_path = cfg.control.join(format!("ACTIVE.{selected}.json"));
    atomic_write_json(&active_path, &document)?;
    validate_global(root, &cfg)?;
    Ok(format!(
        "superseded slot={slot} agent={agent} generation={} owner_pid={} previous_agent={previous_agent}\n",
        generation + 1,
        successor.pid
    ))
}

#[cfg(test)]
mod census_gone_tests {
    //! #1173 census-vanished-process: a read that finds the process
    //! exited is no evidence; an unreadable one still refuses.
    use super::process_uses_slot;
    use crate::proc_reads::test_support::{
        holding_fd, in_cwd, proc_path, running_from, stop, Slot,
    };
    use crate::proc_reads::{inject, reset};

    const ENOENT: i32 = 2;
    const ESRCH: i32 = 3;
    const EACCES: i32 = 13;

    fn uses_with(
        child: &std::process::Child,
        slot: &Slot,
        entries: &[&str],
        errno: i32,
    ) -> (Result<Vec<String>, String>, Vec<String>) {
        reset();
        for entry in entries {
            inject(&proc_path(child, entry), errno);
        }
        let found = process_uses_slot(child.id() as i64, &slot.slot);
        (found, reset())
    }

    #[test]
    fn a_process_gone_on_each_read_is_skipped_and_noted() {
        let slot = Slot::new("supersede");
        let children = [
            (in_cwd(&slot), vec!["cwd"]),
            (holding_fd(&slot), vec!["fd/3"]),
            (holding_fd(&slot), vec!["fd"]),
            (running_from(&slot), vec!["exe", "maps"]),
        ];
        for (child, entries) in &children {
            let (found, notes) = uses_with(child, &slot, &[], ESRCH);
            assert!(
                !found.unwrap().is_empty(),
                "{entries:?} must be a use uninjected"
            );
            assert!(notes.is_empty(), "{notes:?}");
            for errno in [ENOENT, ESRCH] {
                let (found, notes) = uses_with(child, &slot, entries, errno);
                assert_eq!(
                    found.unwrap(),
                    Vec::<String>::new(),
                    "{entries:?} errno {errno}"
                );
                if errno == ESRCH {
                    assert_eq!(
                        notes,
                        [format!(
                            "census: process {} exited during the census ({}: ESRCH); skipped",
                            child.id(),
                            proc_path(child, entries[0]).display()
                        )]
                    );
                } else {
                    // The process's stat is still there: an absent entry,
                    // not proof that it exited.
                    assert!(notes.is_empty(), "{notes:?}");
                }
            }
        }
        // root has no positive case without privilege; a gone root read
        // still completes the census with the other evidence.
        let (child, _entries) = &children[0];
        for errno in [ENOENT, ESRCH] {
            let (found, _notes) = uses_with(child, &slot, &["root"], errno);
            assert_eq!(found.unwrap().len(), 1);
        }
        for (child, _entries) in children {
            stop(child);
        }
    }

    #[test]
    fn an_unreadable_read_still_refuses() {
        let slot = Slot::new("supersede-eacces");
        let child = holding_fd(&slot);
        for entry in ["cwd", "root", "exe", "fd", "fd/3", "maps"] {
            let (found, notes) = uses_with(&child, &slot, &[entry], EACCES);
            let error = found.unwrap_err();
            assert!(
                error.contains("process use is indeterminate because")
                    && error.contains(&format!(
                        "{} is unreadable",
                        proc_path(&child, entry).display()
                    )),
                "{entry}: {error}"
            );
            assert!(notes.is_empty(), "{notes:?}");
        }
        stop(child);
    }

    #[test]
    fn an_unreaped_zombie_is_gone_although_its_fd_directory_is_unreadable() {
        let slot = Slot::new("supersede-zombie");
        let mut child = std::process::Command::new("true")
            .current_dir(&slot.slot)
            .spawn()
            .unwrap();
        let base = std::path::PathBuf::from(format!("/proc/{}", child.id()));
        while !crate::proc_reads::has_exited(&base) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        reset();
        assert_eq!(
            process_uses_slot(child.id() as i64, &slot.slot).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            reset(),
            [format!(
                "census: process {} exited during the census ({}: ENOENT); skipped",
                child.id(),
                base.join("cwd").display()
            )]
        );
        child.wait().unwrap();
    }

    #[test]
    fn an_unreadable_entry_of_a_process_that_exits_within_the_wait_is_gone() {
        let slot = Slot::new("supersede-exit-wait");
        // Not in do_exit: an unreadable entry of a live process refuses at
        // once, without waiting.
        let live = holding_fd(&slot);
        reset();
        inject(&proc_path(&live, "fd"), EACCES);
        let started = std::time::Instant::now();
        let found = process_uses_slot(live.id() as i64, &slot.slot);
        let notes = reset();
        assert!(found.unwrap_err().contains("is unreadable"));
        assert!(started.elapsed() < crate::proc_reads::CENSUS_EXIT_WAIT);
        assert!(notes.is_empty(), "{notes:?}");
        stop(live);

        // In do_exit (its stat says so until it really is a zombie): it
        // counts as gone once it exits within the wait.
        let mut child = std::process::Command::new("sleep")
            .arg("0.02")
            .current_dir(&slot.outside)
            .spawn()
            .unwrap();
        let base = std::path::PathBuf::from(format!("/proc/{}", child.id()));
        let fd_dir = base.join("fd");
        let stat = std::fs::read(base.join("stat")).unwrap();
        reset();
        inject(&fd_dir, EACCES);
        crate::proc_reads::replace_until_exit(&base.join("stat"), &exiting_stat(&stat));
        let found = process_uses_slot(child.id() as i64, &slot.slot);
        let notes = reset();
        child.wait().unwrap();
        assert_eq!(found.unwrap(), Vec::<String>::new());
        assert_eq!(
            notes,
            [format!(
                "census: process {} exited during the census ({}: EACCES); skipped",
                child.id(),
                fd_dir.display()
            )]
        );
    }

    #[test]
    fn an_unreadable_entry_of_a_process_still_exiting_after_the_wait_refuses() {
        let slot = Slot::new("supersede-exit-stuck");
        let child = in_cwd(&slot);
        let base = std::path::PathBuf::from(format!("/proc/{}", child.id()));
        let fd_dir = base.join("fd");
        let stat = std::fs::read(base.join("stat")).unwrap();
        reset();
        inject(&fd_dir, EACCES);
        crate::proc_reads::replace(&base.join("stat"), &exiting_stat(&stat));
        let started = std::time::Instant::now();
        let error = process_uses_slot(child.id() as i64, &slot.slot).unwrap_err();
        let notes = reset();
        assert!(started.elapsed() >= crate::proc_reads::CENSUS_EXIT_WAIT);
        assert!(
            error.contains(&format!("{} is unreadable", fd_dir.display())),
            "{error}"
        );
        assert_eq!(
            notes,
            [format!(
                "census: process {} still exiting after 200 ms ({}: EACCES); indeterminate",
                child.id(),
                fd_dir.display()
            )]
        );
        stop(child);
    }

    /// `stat` with State D and PF_EXITING set: a task stuck in do_exit.
    fn exiting_stat(stat: &[u8]) -> Vec<u8> {
        let text = String::from_utf8_lossy(stat).into_owned();
        let closing = text.rfind(") ").unwrap();
        let mut fields: Vec<String> = text[closing + 2..]
            .split_whitespace()
            .map(str::to_string)
            .collect();
        fields[0] = "D".to_string();
        let flags: u64 = fields[6].parse().unwrap();
        fields[6] = (flags | 0x4).to_string();
        format!("{}) {}\n", &text[..closing], fields.join(" ")).into_bytes()
    }

    #[test]
    fn processes_exiting_during_the_census_never_refuse() {
        let slot = Slot::new("supersede-churn");
        for _ in 0..200 {
            let mut child = std::process::Command::new("true")
                .current_dir(&slot.slot)
                .spawn()
                .unwrap();
            let pid = child.id() as i64;
            std::thread::scope(|scope| {
                scope.spawn(|| child.wait().unwrap());
                for _ in 0..20 {
                    process_uses_slot(pid, &slot.slot)
                        .unwrap_or_else(|error| panic!("exiting PID {pid}: {error}"));
                }
            });
            let _ = reset();
        }
    }
}
