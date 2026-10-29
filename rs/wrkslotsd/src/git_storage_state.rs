//! Registry storage inconsistency parity (Git scope).
//!
//! Reproduces the Git half of the existing storage diagnosis with the
//! same controlled Git invocation (environment stripped of `GIT_*`,
//! no system/global config, replace objects disabled): repository
//! evidence, worktree registration, and checkout identity findings.
//! Repository evidence that cannot be inspected refuses the whole
//! command, matching the non-tolerating callers. Slot-image records
//! refuse here as in `parse-storage`. Read-only.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, parse_state_ordered, Cfg};
use crate::global_state::discover_machines;
use crate::storage_state::{managed_root, py_repr, slot_root, storage_sort_key};

type Result<T> = std::result::Result<T, String>;

#[allow(clippy::too_many_arguments)]
fn note(
    kind: &str,
    scope: &str,
    slot: Option<&str>,
    slot_type: Option<&str>,
    machine: Option<&str>,
    checkout: Option<&str>,
    detail: String,
    remedy: String,
) -> Value {
    json!({
        "kind": kind,
        "scope": scope,
        "slot": slot,
        "slot_type": slot_type,
        "machine": machine,
        "checkout": checkout,
        "detail": detail,
        "remedy": remedy,
    })
}

pub(crate) fn git_run(repository: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command.args([
        "--no-replace-objects",
        "-c",
        "core.useReplaceRefs=false",
        "-C",
    ]);
    command.arg(repository);
    command.args(args);
    command.env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    command.env("LC_ALL", "C");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    command.env("GIT_CONFIG_SYSTEM", "/dev/null");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(format!(
            "Git refused in {}: git {}{}",
            repository.display(),
            args.join(" "),
            suffix
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn unsafe_path(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink() || !meta.is_dir())
        .unwrap_or(true)
}

fn git_common_directory(repository: &Path) -> Result<PathBuf> {
    let stdout = git_run(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(stdout.trim());
    if unsafe_path(&common) {
        return Err(format!(
            "Git reported an unsafe common directory: {}",
            common.display()
        ));
    }
    Ok(common)
}

fn git_listed_worktrees(repository: &Path) -> Result<BTreeSet<PathBuf>> {
    let stdout = git_run(repository, &["worktree", "list", "--porcelain"])?;
    let mut paths = BTreeSet::new();
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            paths.insert(PathBuf::from(rest));
        }
    }
    Ok(paths)
}

fn git_worktree_identity(checkout: &Path) -> Result<(PathBuf, PathBuf, String)> {
    let stdout = git_run(
        checkout,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
            "--verify",
            "HEAD^{commit}",
        ],
    )?;
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() != 3 {
        return Err(format!(
            "Git did not report one worktree identity for {}: expected three lines, got {}",
            checkout.display(),
            lines.len()
        ));
    }
    let root = PathBuf::from(lines[0].trim());
    if unsafe_path(&root) {
        return Err(format!(
            "Git reported an unsafe repository root: {}",
            root.display()
        ));
    }
    let common = PathBuf::from(lines[1].trim());
    if unsafe_path(&common) {
        return Err(format!(
            "Git reported an unsafe common directory: {}",
            common.display()
        ));
    }
    let sha = lines[2].trim().to_string();
    if !is_sha(&sha) {
        return Err(format!(
            "checkout HEAD is not a full commit: {}",
            checkout.display()
        ));
    }
    Ok((root, common, sha))
}

fn normalize_relative(raw: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            _ => parts.push(part.to_string()),
        }
    }
    parts
}

fn stored_path_reference(cfg: &Cfg, raw: &str) -> Result<PathBuf> {
    let is_absolute = raw.starts_with('/');
    let raw_parts: Vec<&str> = raw.split('/').filter(|part| !part.is_empty()).collect();
    if is_absolute || raw_parts.is_empty() || raw_parts.contains(&"..") {
        return Err(format!(
            "checkout path must be a relative path inside {}: {}",
            cfg.root.display(),
            py_repr(raw)
        ));
    }
    let parts = normalize_relative(raw);
    if parts.is_empty() {
        return Err("checkout path must not be the project root".to_string());
    }
    let mut path = cfg.root.clone();
    for part in parts {
        path.push(part);
    }
    Ok(path)
}

fn stored_repository_path(cfg: &Cfg, raw: &str) -> Result<PathBuf> {
    let (stored, unresolved, allowed_root) =
        crate::active_state::stored_repository_parts(cfg, raw)?;
    crate::config_validate::ensure_no_symlink_components(&allowed_root, &unresolved, "repository")?;
    let absolute = unresolved.canonicalize().map_err(|_| {
        format!(
            "source repository {} does not exist or cannot be resolved: {}",
            py_repr(&stored),
            unresolved.display()
        )
    })?;
    if !absolute.is_dir() {
        return Err(format!(
            "repository does not exist or is unsafe: {}",
            absolute.display()
        ));
    }
    let managed = managed_root(cfg);
    if absolute == managed || absolute.starts_with(&managed) {
        return Err(format!(
            "source repository must be outside the managed worktrees directory: {}",
            absolute.display()
        ));
    }
    Ok(absolute)
}

/// Diagnose Git drift between the active rows and worktree
/// registrations, printing the sorted findings.
pub(crate) fn parse_storage_git(root: Option<&Path>) -> Result<Value> {
    let (cfg, _selected) = load_cfg(root, None)?;
    let machines = discover_machines(&cfg.control)?;
    let mut findings: Vec<Value> = Vec::new();
    let mut records: Vec<(String, String, String, Value)> = Vec::new();
    for machine in &machines {
        let (active, order) = parse_state_ordered(root, Some(machine))?;
        let slots = active
            .get("slots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let by_slot: HashMap<String, Value> = slots
            .into_iter()
            .filter_map(|slot| {
                let name = slot.get("slot")?.as_str()?.to_string();
                Some((name, slot))
            })
            .collect();
        for item in &order {
            let stored = by_slot.get(&item.slot).cloned().unwrap_or(Value::Null);
            records.push((
                machine.clone(),
                item.slot.clone(),
                item.slot_type.clone(),
                stored,
            ));
        }
    }
    records.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    let mut listed_by_repository: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut common_by_repository: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let mut listed_by_common: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut recorded_by_common: BTreeMap<PathBuf, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut source_by_common: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let mut record_by_identity: HashMap<(String, String), String> = HashMap::new();
    for (machine, slot, slot_type, _) in &records {
        record_by_identity.insert((slot_type.clone(), slot.clone()), machine.clone());
    }
    for (machine, slot, slot_type, stored) in &records {
        let slot_path = slot_root(&cfg, slot_type).join(slot);
        // An unmounted image is never Git-inspected (the filesystem
        // diagnosis owns the image-not-mounted finding), but its
        // checkouts still join the repository census below, exactly as
        // the oracle's `slot_inspectable = False` path does.
        let unmounted =
            crate::storage_state::unmounted_image_detail(&cfg, slot, slot_type, &slot_path)
                .is_some();
        let mut inspectable = !unmounted
            && slot_path.is_dir()
            && !slot_path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
        let layout = stored
            .get("layout")
            .and_then(Value::as_str)
            .unwrap_or(&cfg.layout);
        if inspectable && layout == "nested" {
            let readable = match std::fs::read_dir(&slot_path) {
                Ok(entries) => entries.collect::<std::io::Result<Vec<_>>>().is_ok(),
                Err(_) => false,
            };
            if !readable {
                inspectable = false;
            }
        }
        let checkouts = stored
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
            let path = stored_path_reference(&cfg, raw_path)?;
            let raw_repository = checkout
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let repository = stored_repository_path(&cfg, raw_repository)?;
            if !listed_by_repository.contains_key(&repository) {
                let common = git_common_directory(&repository)?;
                let listed = git_listed_worktrees(&repository)?;
                common_by_repository.insert(repository.clone(), common);
                listed_by_repository.insert(repository.clone(), listed);
            }
            let common = common_by_repository[&repository].clone();
            let listed = listed_by_repository[&repository].clone();
            source_by_common
                .entry(common.clone())
                .or_insert_with(|| repository.clone());
            listed_by_common
                .entry(common.clone())
                .or_default()
                .extend(listed.iter().cloned());
            recorded_by_common
                .entry(common.clone())
                .or_default()
                .insert(path.clone());
            if !inspectable {
                continue;
            }
            let registered = listed.contains(&path);
            let present = path.symlink_metadata().is_ok();
            if !present {
                if registered {
                    findings.push(note(
                        "git-registration-without-directory",
                        "row",
                        Some(slot),
                        Some(slot_type),
                        Some(machine),
                        Some(&name),
                        format!(
                            "Git still registers absent checkout path {}",
                            path.display()
                        ),
                        "inspect 'git worktree list --porcelain' and repair or remove the stale registration through Git before reusing this path".to_string(),
                    ));
                }
                continue;
            }
            let is_symlink = path
                .symlink_metadata()
                .map(|meta| meta.file_type().is_symlink())
                .unwrap_or(false);
            if is_symlink || !path.is_dir() {
                findings.push(note(
                    "unsafe-checkout-path",
                    "row",
                    Some(slot),
                    Some(slot_type),
                    Some(machine),
                    Some(&name),
                    format!(
                        "recorded checkout path is not a real directory: {}",
                        path.display()
                    ),
                    "preserve and inspect the recorded path; replace no filesystem object until its ownership is established".to_string(),
                ));
                continue;
            }
            if !registered {
                findings.push(note(
                    "checkout-without-git-registration",
                    "row",
                    Some(slot),
                    Some(slot_type),
                    Some(machine),
                    Some(&name),
                    format!(
                        "checkout exists but Git does not register it at {}",
                        path.display()
                    ),
                    "inspect 'git worktree list --porcelain' and repair the exact worktree registration before mutation".to_string(),
                ));
                continue;
            }
            match git_worktree_identity(&path) {
                Err(message) => findings.push(note(
                    "git-worktree-unreadable",
                    "row",
                    Some(slot),
                    Some(slot_type),
                    Some(machine),
                    Some(&name),
                    format!(
                        "cannot verify recorded checkout {}: {message}",
                        path.display()
                    ),
                    "preserve the checkout and repair its Git metadata before mutation".to_string(),
                )),
                Ok((worktree_root, worktree_common, _head)) => {
                    if worktree_root != path || worktree_common != common {
                        findings.push(note(
                            "git-worktree-mismatch",
                            "row",
                            Some(slot),
                            Some(slot_type),
                            Some(machine),
                            Some(&name),
                            format!(
                                "checkout {} does not identify the recorded repository worktree",
                                path.display()
                            ),
                            "preserve the checkout and repair the exact Git worktree registration before mutation".to_string(),
                        ));
                    }
                }
            }
        }
    }
    let mut roots: Vec<(String, PathBuf)> = vec![
        ("agent".to_string(), slot_root(&cfg, "agent")),
        ("validate".to_string(), slot_root(&cfg, "validate")),
    ];
    roots.sort_by_key(|(_, path)| std::cmp::Reverse(path.components().count()));
    for (common, registered_paths) in &listed_by_common {
        let source = &source_by_common[common];
        let recorded = recorded_by_common.get(common).cloned().unwrap_or_default();
        let mut extra: Vec<&PathBuf> = registered_paths.difference(&recorded).collect();
        extra.sort();
        for registered_path in extra {
            let managed = roots
                .iter()
                .find(|(_, root)| registered_path == root || registered_path.starts_with(root));
            let Some((slot_type, managed_root_path)) = managed else {
                continue;
            };
            let relative = registered_path
                .strip_prefix(managed_root_path)
                .unwrap_or(Path::new(""));
            let slot = relative
                .components()
                .next()
                .map(|part| part.as_os_str().to_string_lossy().into_owned());
            let matched_machine = slot.as_ref().and_then(|slot_name| {
                record_by_identity.get(&(slot_type.clone(), slot_name.clone()))
            });
            let (scope, machine) = match matched_machine {
                Some(machine) => ("row", Some(machine.as_str())),
                None => ("directory", None),
            };
            let remedy = if slot_type == "validate" {
                format!(
                    "inspect 'git -C {} worktree list --porcelain'; for a present ownerless validation checkout preserve and inspect it; destructive recovery is disabled because the checkout and Git administration cannot both be fenced against same-UID replacement; use import-existing only for a demonstrably live owner, and repair a missing stale registration before reusing any overlapping path",
                    source.display()
                )
            } else {
                format!(
                    "inspect 'git -C {} worktree list --porcelain'; restore and register the live slot or remove the stale Git registration before reusing any overlapping path",
                    source.display()
                )
            };
            findings.push(note(
                "git-registration-without-row",
                scope,
                slot.as_deref(),
                Some(slot_type),
                machine,
                None,
                format!(
                    "source repository {} (Git common directory {}) registers {} inside the managed {slot_type} root, but no active checkout row for that repository names it",
                    source.display(),
                    common.display(),
                    registered_path.display()
                ),
                remedy,
            ));
        }
    }
    findings.sort_by_key(storage_sort_key);
    Ok(json!({
        "machines": machines,
        "findings": findings,
    }))
}
