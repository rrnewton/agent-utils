//! `services prune`: delete snapshots nothing needs any more (#1917).
//!
//! A snapshot is kept when a live unit runs it, when a rollback record
//! under `ROOT/refreshes/` names it (the run directory's name, its
//! `report.json`, a `UNIT.before`, or `ROLLBACK.sh`: rolling back must
//! find the old binary still there), when a turnover pin protects the
//! unit that runs it (turnover will never move that unit, so its build
//! stays), or when it is among the newest `--keep` snapshots. Everything
//! else is deleted. The default is a dry run that names each deletion
//! and the reason each kept snapshot is kept; `--apply` deletes.
//!
//! Prune is its own subcommand rather than a step at the end of build
//! or refresh: it can be audited and run in an emergency without
//! building anything or restarting a service, and a build never
//! surprises its caller by deleting the snapshot a unit still runs.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::discover::{self, OptionTables, Service};
use super::host::Host;
use super::snapshot::{self, Snapshot};

/// What `services prune` was asked to do.
#[derive(Clone, Debug)]
pub struct PruneOptions {
    /// Where snapshots and refresh records live.
    pub root: PathBuf,
    /// How many of the newest otherwise-unreferenced snapshots to keep.
    pub keep_newest: usize,
    /// Delete; without this the run only reports.
    pub apply: bool,
    /// The global agentctl binary a `ROOT/global.pin` can protect.
    pub global_binary: Option<PathBuf>,
}

/// Outcome of a whole prune.
pub struct PruneReport {
    /// The JSON report (schema `agentctl-services-prune/v1`).
    pub document: Value,
    /// The text report.
    pub text: String,
    /// 0: the plan is safe (and, with `--apply`, was carried out);
    /// 1: something could not be read, so nothing was deleted, or a
    /// deletion failed.
    pub exit_code: i32,
}

fn is_live(service: &Service) -> bool {
    matches!(service.state.active_state.as_str(), "active" | "reloading")
}

/// The snapshot a service runs, by its binary or its plugin home.
fn snapshot_index(service: &Service, snapshots: &[Snapshot]) -> Option<usize> {
    let binary = discover::normalize(&service.binary);
    let home = service.plugin_home().map(|home| discover::normalize(&home));
    snapshots.iter().position(|snapshot| {
        discover::normalize(&snapshot.agentctl()) == binary
            || home.as_deref() == Some(discover::normalize(&snapshot.home()).as_path())
    })
}

/// The reason a pin file names; `None` when it does not exist. An empty
/// pin is an error, as in turnover: it names no reason.
fn pin_reason(host: &dyn Host, path: &Path) -> Result<Option<String>, String> {
    let Some(bytes) = host.read_file(path) else {
        return Ok(None);
    };
    let reason = String::from_utf8_lossy(&bytes).trim().to_owned();
    if reason.is_empty() {
        return Err(format!(
            "pin {} names no reason; write why it is pinned or remove it",
            path.display()
        ));
    }
    Ok(Some(reason))
}

/// Every snapshot a refresh record names, with the record's directory
/// name as the reason.
fn rollback_references(root: &Path, snapshots: &[Snapshot]) -> BTreeMap<usize, Vec<String>> {
    let mut referenced: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let Ok(runs) = fs::read_dir(root.join("refreshes")) else {
        return referenced;
    };
    let mut run_dirs: Vec<PathBuf> = runs
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    run_dirs.sort();
    for run_dir in run_dirs {
        let mut text = run_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if let Ok(entries) = fs::read_dir(&run_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Ok(bytes) = fs::read(&path) {
                        text.push('\n');
                        text.push_str(&String::from_utf8_lossy(&bytes));
                    }
                }
            }
        }
        let record = run_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        for (index, snapshot) in snapshots.iter().enumerate() {
            if text.contains(&snapshot.commit)
                || text.contains(&snapshot.path.to_string_lossy().into_owned())
            {
                referenced
                    .entry(index)
                    .or_default()
                    .push(format!("rollback record {record}"));
            }
        }
    }
    referenced
}

/// Remove one snapshot tree; snapshot files are read-only, so make its
/// directories writable first (unlinking needs a writable directory,
/// never a writable file).
fn remove_snapshot(path: &Path) -> Result<(), String> {
    fn unlock(directory: &Path) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in fs::read_dir(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?
            .flatten()
        {
            let child = entry.path();
            if child.is_dir() && !child.is_symlink() {
                unlock(&child)?;
            }
        }
        Ok(())
    }
    unlock(path)?;
    fs::remove_dir_all(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// Run `services prune`.
pub fn prune(
    host: &dyn Host,
    tables: &dyn OptionTables,
    options: &PruneOptions,
) -> Result<PruneReport, String> {
    let root = &options.root;
    let snapshots = snapshot::list(root);
    let (services, unreadable) = discover::discover_all(host, tables)?;
    let mut reasons: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut problems: Vec<String> = unreadable
        .iter()
        .map(|(unit, error)| format!("{unit}: runs agentctl but cannot be read: {error}"))
        .collect();
    let mut keep = |index: usize, reason: String| {
        let entry = reasons.entry(index).or_default();
        if !entry.contains(&reason) {
            entry.push(reason);
        }
    };
    // Live units, and the snapshots turnover pins protect.
    for service in &services {
        let Some(index) = snapshot_index(service, &snapshots) else {
            continue;
        };
        if is_live(service) {
            keep(index, format!("live unit {}", service.unit));
        }
        let registry = discover::registry_identity(&service.registry);
        for pin_path in [
            registry
                .join("turnover-pins")
                .join(format!("{}.pin", service.unit)),
            registry.join("turnover.pin"),
        ] {
            match pin_reason(host, &pin_path) {
                Ok(Some(reason)) => keep(
                    index,
                    format!(
                        "turnover pin {} pins {} ({reason})",
                        pin_path.display(),
                        service.unit
                    ),
                ),
                Ok(None) => {}
                Err(error) => problems.push(error),
            }
        }
    }
    // The global binary's pin protects the snapshot it was built from.
    if let Some(global) = &options.global_binary {
        match pin_reason(host, &root.join("global.pin")) {
            Ok(Some(reason)) => {
                if let Some(commit) = host.binary_commit(global) {
                    if let Some(index) = snapshots
                        .iter()
                        .position(|snapshot| snapshot.commit == commit)
                    {
                        keep(
                            index,
                            format!(
                                "turnover pin {} pins the global binary ({reason})",
                                root.join("global.pin").display()
                            ),
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(error) => problems.push(error),
        }
    }
    // Rollback records: rolling back must find the old binary still there.
    for (index, mut record_reasons) in rollback_references(root, &snapshots) {
        for reason in record_reasons.drain(..) {
            keep(index, reason);
        }
    }
    // The newest few survive for a quick step back.
    let first_newest = snapshots.len().saturating_sub(options.keep_newest);
    for index in first_newest..snapshots.len() {
        keep(
            index,
            format!("among the newest {} kept", options.keep_newest),
        );
    }
    let blocked = !problems.is_empty();
    let mut rows: Vec<Value> = Vec::new();
    let mut text = String::new();
    let _ = writeln!(
        text,
        "services prune: {} snapshot(s) under {}{}",
        snapshots.len(),
        snapshot::snapshots_dir(root).display(),
        if options.apply { "" } else { " (dry run)" }
    );
    let mut deleted: Vec<String> = Vec::new();
    let mut delete_failed = false;
    for (index, snapshot) in snapshots.iter().enumerate() {
        let kept_reasons = reasons.get(&index);
        if let Some(kept_reasons) = kept_reasons {
            let _ = writeln!(
                text,
                "KEEP {} {}: {}",
                snapshot.short(),
                snapshot.path.display(),
                kept_reasons.join("; ")
            );
            rows.push(json!({
                "commit": snapshot.commit,
                "path": snapshot.path,
                "kept": true,
                "reasons": kept_reasons,
                "deleted": false,
            }));
        } else if blocked {
            let _ = writeln!(
                text,
                "KEEP {} {}: not deleted: the run is blocked (see below)",
                snapshot.short(),
                snapshot.path.display()
            );
            rows.push(json!({
                "commit": snapshot.commit,
                "path": snapshot.path,
                "kept": true,
                "reasons": ["not deleted: the run is blocked"],
                "deleted": false,
            }));
        } else if options.apply {
            match remove_snapshot(&snapshot.path) {
                Ok(()) => {
                    deleted.push(snapshot.commit.clone());
                    let _ = writeln!(
                        text,
                        "DELETE {} {}: deleted",
                        snapshot.short(),
                        snapshot.path.display()
                    );
                    rows.push(json!({
                        "commit": snapshot.commit,
                        "path": snapshot.path,
                        "kept": false,
                        "reasons": [],
                        "deleted": true,
                    }));
                }
                Err(error) => {
                    delete_failed = true;
                    problems.push(format!(
                        "could not delete {}: {error}",
                        snapshot.path.display()
                    ));
                    let _ = writeln!(
                        text,
                        "DELETE {} {}: FAILED: {error}",
                        snapshot.short(),
                        snapshot.path.display()
                    );
                    rows.push(json!({
                        "commit": snapshot.commit,
                        "path": snapshot.path,
                        "kept": false,
                        "reasons": [],
                        "deleted": false,
                    }));
                }
            }
        } else {
            let _ = writeln!(
                text,
                "DELETE {} {}: would delete",
                snapshot.short(),
                snapshot.path.display()
            );
            rows.push(json!({
                "commit": snapshot.commit,
                "path": snapshot.path,
                "kept": false,
                "reasons": [],
                "deleted": false,
            }));
        }
    }
    if snapshots.is_empty() {
        let _ = writeln!(text, "no snapshots to prune");
    }
    for problem in &problems {
        let _ = writeln!(text, "problem: {problem}");
    }
    let kept_rows = rows.iter().filter(|row| row["kept"] == true).count();
    let would_delete = rows.iter().filter(|row| row["kept"] == false).count();
    let _ = writeln!(
        text,
        "{} kept, {} {}{}",
        kept_rows,
        if options.apply {
            deleted.len()
        } else {
            would_delete
        },
        if options.apply {
            "deleted"
        } else {
            "would be deleted"
        },
        if blocked {
            "; nothing was deleted: fix the problem(s) above first"
        } else if !options.apply {
            "; nothing changed (dry run; --apply deletes)"
        } else {
            ""
        }
    );
    let exit_code = i32::from(blocked || delete_failed);
    let document = json!({
        "schema": "agentctl-services-prune/v1",
        "root": root,
        "keep_newest": options.keep_newest,
        "dry_run": !options.apply,
        "blocked": blocked,
        "snapshots": rows,
        "problems": problems,
        "exit_code": exit_code,
    });
    Ok(PruneReport {
        document,
        text,
        exit_code,
    })
}
