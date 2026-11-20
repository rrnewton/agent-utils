//! Stray Git worktree reap (`reap-stray-worktrees`, #775).
//!
//! Reports registrations belonging to no slot and no live process;
//! with apply, preserves a rescue ref and removes each reapable
//! registration. Live or unpushed work is retained. Mirrors the
//! Python edition.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::{load_cfg, slot_directory};
use crate::batch_text::sort_keys;
use crate::finish_write::stored_path;
use crate::git_storage_state::git_run;
use crate::global_state::discover_machines;

type Result<T> = std::result::Result<T, String>;

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

fn live_use(path: &Path) -> Option<String> {
    let proc_dir = Path::new("/proc");
    let entries = std::fs::read_dir(proc_dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.chars().all(|c| c.is_ascii_digit()) || name.is_empty() {
            continue;
        }
        for link in ["cwd", "root", "exe"] {
            if let Ok(target) = std::fs::read_link(entry.path().join(link)) {
                let absolute = if target.is_absolute() {
                    target
                } else {
                    entry.path().join(&target)
                };
                if within(&absolute, path) {
                    return Some(format!("pid {name}: {link}={}", absolute.display()));
                }
            }
        }
    }
    None
}

fn git(repo: &Path, args: &[&str]) -> std::result::Result<String, String> {
    git_run(repo, args)
}

pub(crate) fn reap_stray_worktrees(
    root: Option<&Path>,
    machine_override: Option<&str>,
    repositories_in: &[String],
    apply: bool,
    format_json: bool,
) -> Result<(String, bool)> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let mut owned: BTreeSet<PathBuf> = BTreeSet::new();
    let mut repositories: BTreeSet<PathBuf> = BTreeSet::new();
    // #1433: archived rows whose source repository is gone cannot own
    // any live registration: both modes skip and name them. Active
    // rows with an unresolvable repository refuse (see the walk).
    let mut unresolvable_archives: Vec<Value> = Vec::new();
    for machine in discover_machines(&cfg.control)? {
        for (doc_name, key) in [
            (format!("ACTIVE.{machine}.json"), "slots"),
            (format!("ARCHIVED.{machine}.json"), "records"),
        ] {
            let Ok(text) = std::fs::read_to_string(cfg.control.join(&doc_name)) else {
                continue;
            };
            let Ok(doc) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let Some(records) = doc.get(key).and_then(Value::as_array) else {
                continue;
            };
            for record in records {
                if key == "slots" {
                    let slot = record
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let slot_type = record
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or("agent");
                    if let Ok(dir) = slot_directory(&cfg, slot, slot_type) {
                        owned.insert(dir);
                    }
                }
                if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
                    for checkout in checkouts {
                        if key == "slots" {
                            if let Some(raw) = checkout.get("path").and_then(Value::as_str) {
                                if let Ok(path) = stored_path(&cfg, raw, "checkout path") {
                                    owned.insert(path);
                                }
                            }
                        }
                        if let Some(raw) = checkout.get("repository").and_then(Value::as_str) {
                            match crate::finish_write::stored_repository_path(&cfg, raw) {
                                Ok((_, repo)) => {
                                    repositories.insert(repo);
                                }
                                Err(reason) if key == "records" => {
                                    let row = json!({
                                        "machine": machine,
                                        "slot": record
                                            .get("slot")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned),
                                        "repository": raw,
                                        "reason": reason,
                                    });
                                    if !unresolvable_archives.contains(&row) {
                                        unresolvable_archives.push(row);
                                    }
                                }
                                // #1433: an active row's repository must
                                // resolve in both modes.
                                Err(reason) if key == "slots" => return Err(reason),
                                Err(_) => {}
                            }
                        }
                    }
                }
            }
        }
    }
    for raw in repositories_in {
        let candidate = PathBuf::from(raw);
        repositories.insert(if candidate.is_absolute() {
            candidate
        } else {
            cfg.root.join(candidate)
        });
    }
    if repositories.is_empty() {
        return Err("no source repository is known; pass --repository PATH".to_string());
    }
    let mut stray: Vec<Value> = Vec::new();
    let mut retained: Vec<Value> = Vec::new();
    for repository in &repositories {
        let listed = git(repository, &["worktree", "list", "--porcelain", "-z"])
            .map_err(|e| format!("cannot list worktrees of {}: {e}", repository.display()))?;
        // git_run returns stdout text; -z fields separated by NUL.
        let mut entries: Vec<std::collections::BTreeMap<String, String>> = Vec::new();
        let mut current = std::collections::BTreeMap::new();
        for field in listed.split('\0') {
            // Records also split on blank lines encoded as empty fields.
            if field.is_empty() {
                if !current.is_empty() {
                    entries.push(current);
                    current = std::collections::BTreeMap::new();
                }
                continue;
            }
            for line in field.lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                current.insert(key.to_string(), value.to_string());
            }
        }
        if !current.is_empty() {
            entries.push(current);
        }
        entries.sort_by_key(|entry| entry.get("worktree").cloned().unwrap_or_default());
        for entry in &entries {
            let Some(raw_path) = entry.get("worktree") else {
                continue;
            };
            if entry.contains_key("bare") {
                continue;
            }
            let path = PathBuf::from(raw_path);
            if path == *repository {
                continue;
            }
            if owned
                .iter()
                .any(|owned_path| within(&path, owned_path) || within(owned_path, &path))
            {
                continue;
            }
            let head = entry.get("HEAD").cloned().unwrap_or_default();
            let row = json!({
                "repository": repository.to_string_lossy(),
                "path": path.to_string_lossy(),
                "head": head,
                "branch": entry.get("branch").cloned().unwrap_or_default(),
            });
            if let Some(found) = live_use(&path) {
                retained.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "retained-live", "reason": found,
                }));
                continue;
            }
            let is_sha = head.len() == 40 && head.chars().all(|c| c.is_ascii_hexdigit());
            let contained = is_sha
                && git(repository, &["branch", "-r", "--contains", &head])
                    .map(|out| !out.trim().is_empty())
                    .unwrap_or(false);
            if !contained && !entry.contains_key("prunable") {
                retained.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "retained-unpushed",
                    "reason": "HEAD is not contained in any remote branch",
                }));
                continue;
            }
            if !apply {
                stray.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "would-reap",
                    "reason": "no slot, live process, or unpushed work owns it",
                }));
                continue;
            }
            if live_use(&path).is_some() {
                retained.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "retained-live", "reason": "a live process began using it",
                }));
                continue;
            }
            let mut rescue_ref = String::new();
            if is_sha {
                let label: String = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "worktree".to_string())
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                            c
                        } else {
                            '-'
                        }
                    })
                    .collect();
                rescue_ref = format!(
                    "refs/rescue/wrkslots/stray-{}-{}",
                    label.trim_matches('-'),
                    &head[..12]
                );
                if git(repository, &["update-ref", &rescue_ref, &head]).is_err()
                    || git(repository, &["rev-parse", "--verify", &rescue_ref])
                        .map(|out| out.trim().to_string())
                        .unwrap_or_default()
                        != head
                {
                    retained.push(json!({
                        "repository": row["repository"], "path": row["path"],
                        "head": row["head"], "branch": row["branch"],
                        "outcome": "retained-rescue-failed", "reason": "rescue ref did not read back",
                    }));
                    continue;
                }
            }
            let removed = if path.exists() {
                git(
                    repository,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                )
            } else {
                git(repository, &["worktree", "prune", "--expire", "now"])
            };
            match removed {
                Ok(_) => stray.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "reaped", "rescue_ref": rescue_ref,
                })),
                Err(error) => retained.push(json!({
                    "repository": row["repository"], "path": row["path"],
                    "head": row["head"], "branch": row["branch"],
                    "outcome": "retained-remove-failed", "reason": error,
                })),
            }
        }
    }
    if format_json {
        let payload = json!({
            "schema": 1,
            "applied": apply,
            "repositories": repositories.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "stray": stray,
            "retained": retained,
            // #877: this reader is already lenient per record, so the
            // named-malformed list is empty here.
            "malformed_archives": Vec::<Value>::new(),
            "unresolvable_archives": unresolvable_archives,
        });
        return Ok((
            format!(
                "{}\n",
                serde_json::to_string_pretty(&sort_keys(&payload)).unwrap_or_default()
            ),
            !retained.is_empty(),
        ));
    }
    let mut out = format!(
        "applied={} stray={} retained={}\n",
        if apply { "true" } else { "false" },
        stray.len(),
        retained.len()
    );
    for item in &stray {
        out.push_str(&format!(
            "STRAY: {} head={}\n",
            item.get("path").and_then(Value::as_str).unwrap_or_default(),
            item.get("head").and_then(Value::as_str).unwrap_or_default()
        ));
    }
    for item in &retained {
        out.push_str(&format!(
            "RETAINED: {} outcome={} reason={}\n",
            item.get("path").and_then(Value::as_str).unwrap_or_default(),
            item.get("outcome")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            item.get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
    }
    for item in &unresolvable_archives {
        out.push_str(&format!(
            "UNRESOLVABLE-ARCHIVE: machine={} slot={} repository={} reason={}\n",
            item.get("machine")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            item.get("slot").and_then(Value::as_str).unwrap_or_default(),
            item.get("repository")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            item.get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ));
    }
    Ok((out, !retained.is_empty()))
}

/// Entry budget for one stray worktree's size walk (#1930), as in Python.
const STRAY_SIZE_ENTRY_LIMIT: usize = 20000;

fn bounded_tree_size(path: &Path) -> (u64, bool) {
    fn walk(dir: &Path, total: &mut u64, seen: &mut usize) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&entry_path) else {
                continue;
            };
            if metadata.is_dir() {
                if entry.file_name() == ".git" {
                    continue;
                }
                if walk(&entry_path, total, seen) {
                    return true;
                }
            } else {
                *seen += 1;
                if *seen > STRAY_SIZE_ENTRY_LIMIT {
                    return true;
                }
                *total += metadata.len();
            }
        }
        false
    }
    let mut total = 0u64;
    let mut seen = 0usize;
    let truncated = walk(path, &mut total, &mut seen);
    (total, truncated)
}

fn stray_age_seconds(path: &Path) -> Option<i64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if let Ok(out) = git(path, &["log", "-1", "--format=%ct", "HEAD"]) {
        if let Ok(moment) = out.trim().parse::<i64>() {
            return Some((now - moment).max(0));
        }
    }
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| (now - d.as_secs() as i64).max(0))
}

/// Read-only `doctor --stray-worktrees` census (#1930), mirroring Python.
pub(crate) fn doctor_stray_worktrees(
    root: Option<&Path>,
    machine_override: Option<&str>,
    repositories_in: &[String],
    format_json: bool,
) -> Result<String> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let mut owned: BTreeSet<PathBuf> = BTreeSet::new();
    let mut slot_dirs: Vec<(PathBuf, String)> = Vec::new();
    let mut repositories: BTreeSet<PathBuf> = BTreeSet::new();
    for machine in discover_machines(&cfg.control)? {
        for (doc_name, key) in [
            (format!("ACTIVE.{machine}.json"), "slots"),
            (format!("ARCHIVED.{machine}.json"), "records"),
        ] {
            let Ok(text) = std::fs::read_to_string(cfg.control.join(&doc_name)) else {
                continue;
            };
            let Ok(doc) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let Some(records) = doc.get(key).and_then(Value::as_array) else {
                continue;
            };
            for record in records {
                if key == "slots" {
                    let slot = record
                        .get("slot")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let slot_type = record
                        .get("slot_type")
                        .and_then(Value::as_str)
                        .unwrap_or("agent");
                    if let Ok(dir) = slot_directory(&cfg, slot, slot_type) {
                        owned.insert(dir.clone());
                        slot_dirs.push((dir, slot.to_string()));
                    }
                }
                if let Some(checkouts) = record.get("checkouts").and_then(Value::as_array) {
                    for checkout in checkouts {
                        if checkout.get("vcs").and_then(Value::as_str) == Some("eden") {
                            continue;
                        }
                        if key == "slots" {
                            if let Some(raw) = checkout.get("path").and_then(Value::as_str) {
                                if let Ok(path) = stored_path(&cfg, raw, "checkout path") {
                                    owned.insert(path);
                                }
                            }
                        }
                        if let Some(raw) = checkout.get("repository").and_then(Value::as_str) {
                            if let Ok((_, repo)) =
                                crate::finish_write::stored_repository_path(&cfg, raw)
                            {
                                repositories.insert(repo);
                            }
                        }
                    }
                }
            }
        }
    }
    for raw in repositories_in {
        let candidate = PathBuf::from(raw);
        repositories.insert(if candidate.is_absolute() {
            candidate
        } else {
            cfg.root.join(candidate)
        });
    }
    let mut rows: Vec<Value> = Vec::new();
    let mut reported: BTreeSet<PathBuf> = BTreeSet::new();
    let mut add_row = |repository: &Path,
                       path: &Path,
                       branch: &str,
                       head: &str,
                       kind: &str,
                       slot: Option<&str>,
                       rows: &mut Vec<Value>| {
        if !reported.insert(path.to_path_buf()) {
            return;
        }
        let (size, truncated) = if path.is_dir() {
            bounded_tree_size(path)
        } else {
            (0, false)
        };
        let is_sha = head.len() == 40 && head.chars().all(|c| c.is_ascii_hexdigit());
        let unpushed = if is_sha {
            git(repository, &["branch", "-r", "--contains", head])
                .ok()
                .map(|out| out.trim().is_empty())
        } else {
            None
        };
        rows.push(json!({
            "repository": repository.to_string_lossy(),
            "path": path.to_string_lossy(),
            "branch": branch,
            "head": head,
            "kind": kind,
            "slot": slot,
            "age_seconds": stray_age_seconds(path),
            "size_bytes": size,
            "size_truncated": truncated,
            "has_unpushed_commits": unpushed,
        }));
    };
    for repository in &repositories {
        let listed = git(repository, &["worktree", "list", "--porcelain", "-z"])
            .map_err(|e| format!("cannot list worktrees of {}: {e}", repository.display()))?;
        let mut entries: Vec<std::collections::BTreeMap<String, String>> = Vec::new();
        let mut current = std::collections::BTreeMap::new();
        for field in listed.split('\0') {
            if field.is_empty() {
                if !current.is_empty() {
                    entries.push(current);
                    current = std::collections::BTreeMap::new();
                }
                continue;
            }
            for line in field.lines() {
                let (key, value) = line.split_once(' ').unwrap_or((line, ""));
                current.insert(key.to_string(), value.to_string());
            }
        }
        if !current.is_empty() {
            entries.push(current);
        }
        for entry in &entries {
            let Some(raw_path) = entry.get("worktree") else {
                continue;
            };
            if entry.contains_key("bare") {
                continue;
            }
            let path = PathBuf::from(raw_path);
            if path == *repository || owned.contains(&path) {
                continue;
            }
            let enclosing = slot_dirs.iter().find(|(dir, _)| within(&path, dir));
            let branch = entry
                .get("branch")
                .cloned()
                .unwrap_or_default()
                .strip_prefix("refs/heads/")
                .map(str::to_string)
                .unwrap_or_else(|| entry.get("branch").cloned().unwrap_or_default());
            add_row(
                repository,
                &path,
                &branch,
                entry.get("HEAD").map(String::as_str).unwrap_or_default(),
                if enclosing.is_some() {
                    "nested-in-slot"
                } else {
                    "outside-slot"
                },
                enclosing.map(|(_, name)| name.as_str()),
                &mut rows,
            );
        }
    }
    slot_dirs.sort();
    for (slot_dir, slot) in &slot_dirs {
        let Ok(children) = std::fs::read_dir(slot_dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = children.flatten().map(|e| e.path()).collect();
        children.sort();
        for child in children {
            if owned.contains(&child) || !child.join(".git").exists() {
                continue;
            }
            let Ok(head) = git(&child, &["rev-parse", "HEAD"]) else {
                continue;
            };
            let branch = git(&child, &["branch", "--show-current"]).unwrap_or_default();
            let repository = git(
                &child,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
            )
            .ok()
            .map(|out| {
                PathBuf::from(out.trim())
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| child.clone())
            })
            .unwrap_or_else(|| child.clone());
            add_row(
                &repository,
                &child,
                branch.trim(),
                head.trim(),
                "nested-in-slot",
                Some(slot),
                &mut rows,
            );
        }
    }
    rows.sort_by(|a, b| {
        a.get("path")
            .and_then(Value::as_str)
            .cmp(&b.get("path").and_then(Value::as_str))
    });
    if format_json {
        let payload = json!({"schema": 1, "stray_worktrees": rows});
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&sort_keys(&payload)).unwrap_or_default()
        ));
    }
    let bool_text = |value: Option<bool>| match value {
        Some(true) => "True",
        Some(false) => "False",
        None => "None",
    };
    let mut out = format!("stray_worktrees={}\n", rows.len());
    for row in &rows {
        let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or_default();
        out.push_str(&format!(
            "STRAY-WORKTREE: path={} repository={} branch={} head={} kind={} slot={} age_seconds={} size_bytes={} size_truncated={} has_unpushed_commits={}\n",
            text("path"),
            text("repository"),
            if text("branch").is_empty() { "-" } else { text("branch") },
            text("head"),
            text("kind"),
            row.get("slot").and_then(Value::as_str).unwrap_or("-"),
            row.get("age_seconds")
                .and_then(Value::as_i64)
                .map(|v| v.to_string())
                .unwrap_or_else(|| "None".to_string()),
            row.get("size_bytes").and_then(Value::as_u64).unwrap_or_default(),
            match row.get("size_truncated").and_then(Value::as_bool) {
                Some(true) => "true",
                _ => "false",
            },
            bool_text(row.get("has_unpushed_commits").and_then(Value::as_bool)),
        ));
    }
    Ok(out)
}
