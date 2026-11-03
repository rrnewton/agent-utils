//! Slot-state home reclamation (`reclaim-orphan-state`, #796).
//!
//! Removal reclaims `slot-state/<slot_type>/<slot>` with the slot;
//! this command plans (or, with apply, performs) the same reclaim for
//! homes whose slot is already archived. Mirrors the Python edition.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::active_state::load_cfg;
use crate::batch_text::sort_keys;
use crate::config_validate::load_config;
use crate::freeze::assert_not_frozen;
use crate::git_storage_state::git_run;
use crate::global_state::discover_machines;

type Result<T> = std::result::Result<T, String>;

pub(crate) fn state_home(control: &Path, slot_type: &str, slot: &str) -> PathBuf {
    control.join("slot-state").join(slot_type).join(slot)
}

fn within(candidate: &Path, parent: &Path) -> bool {
    candidate == parent || candidate.starts_with(parent)
}

pub(crate) fn live_use(path: &Path) -> Option<String> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit()) {
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
        if let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) {
            for fd in fds.flatten() {
                if let Ok(target) = std::fs::read_link(fd.path()) {
                    if within(&target, path) {
                        return Some(format!("pid {name}: fd={}", target.display()));
                    }
                }
            }
        }
    }
    None
}

fn git_repos(home: &Path) -> Vec<PathBuf> {
    let mut repos = Vec::new();
    let mut stack = vec![home.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<_> = entries.flatten().collect();
        names.sort_by_key(|e| e.file_name());
        for entry in names {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                repos.push(dir.clone());
                continue;
            }
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            }
        }
    }
    repos.sort();
    repos
}

fn unpushed_head(repo: &Path) -> Option<String> {
    let head = git_run(repo, &["rev-parse", "--verify", "HEAD"]).ok()?;
    let sha = head.trim().to_string();
    let contained = git_run(
        repo,
        &[
            "for-each-ref",
            "--contains",
            &sha,
            "--format=%(refname)",
            "refs/remotes/",
        ],
    )
    .unwrap_or_default();
    if contained.lines().any(|line| !line.trim().is_empty()) {
        return None;
    }
    Some(sha)
}

fn salvage_repo(
    control: &Path,
    slot_type: &str,
    slot: &str,
    repo: &Path,
    head: &str,
) -> Result<PathBuf> {
    let directory = control
        .join("wrkslots-salvage")
        .join("state-home")
        .join(format!("{slot_type}-{slot}"));
    std::fs::create_dir_all(&directory)
        .map_err(|e| format!("cannot create salvage directory: {e}"))?;
    let parts: Vec<String> = repo
        .components()
        .rev()
        .take(2)
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let label = parts.into_iter().rev().collect::<Vec<_>>().join("-");
    let bundle = directory.join(format!("{label}-{}.bundle", &head[..12]));
    if !bundle.exists() {
        let temp = directory.join(format!(
            ".{}.tmp.{}",
            bundle.file_name().unwrap().to_string_lossy(),
            std::process::id()
        ));
        let made = git_run(repo, &["bundle", "create", &temp.to_string_lossy(), "HEAD"]);
        if made.is_err() {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "cannot bundle unpushed git in slot state home {}",
                repo.display()
            ));
        }
        let heads =
            git_run(repo, &["bundle", "list-heads", &temp.to_string_lossy()]).unwrap_or_default();
        let verified = git_run(repo, &["bundle", "verify", &temp.to_string_lossy()]);
        if verified.is_err() || !heads.contains(head) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "salvage bundle for state home {} did not verify for {head}",
                repo.display()
            ));
        }
        std::fs::rename(&temp, &bundle).map_err(|e| format!("cannot store salvage bundle: {e}"))?;
    }
    Ok(bundle)
}

/// Liveness gate + unpushed-git salvage for one state home.
pub(crate) fn prepare_reclaim(control: &Path, slot_type: &str, slot: &str) -> Result<Vec<PathBuf>> {
    let home = state_home(control, slot_type, slot);
    if !home.exists() {
        return Ok(Vec::new());
    }
    if let Some(found) = live_use(&home) {
        return Err(format!(
            "slot state home {} is in use by a live process ({found}); refusing to reclaim it",
            home.display()
        ));
    }
    let mut bundles = Vec::new();
    for repo in git_repos(&home) {
        if let Some(head) = unpushed_head(&repo) {
            bundles.push(salvage_repo(control, slot_type, slot, &repo, &head)?);
        }
    }
    Ok(bundles)
}

pub(crate) fn reclaim_home(control: &Path, slot_type: &str, slot: &str) -> bool {
    let home = state_home(control, slot_type, slot);
    if !home.exists() {
        return false;
    }
    std::fs::remove_dir_all(&home).is_ok()
}

pub(crate) fn reclaim_orphan_state(
    root: Option<&Path>,
    machine_override: Option<&str>,
    apply: bool,
    format_json: bool,
) -> Result<String> {
    let (cfg, _selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    if apply {
        assert_not_frozen(&cfg, &config, "reclaim-orphan-state")?;
    }
    let mut active: BTreeSet<(String, String)> = BTreeSet::new();
    let mut archived: BTreeSet<(String, String)> = BTreeSet::new();
    for machine in discover_machines(&cfg.control)? {
        for (doc_name, key, target) in [
            (format!("ACTIVE.{machine}.json"), "slots", &mut active),
            (format!("ARCHIVED.{machine}.json"), "records", &mut archived),
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
                target.insert((slot_type, slot));
            }
        }
    }
    let base = cfg.control.join("slot-state");
    let mut rows: Vec<(String, String, PathBuf)> = Vec::new();
    if base.is_dir() {
        let mut types: Vec<_> = std::fs::read_dir(&base)
            .map_err(|e| format!("cannot list state homes: {e}"))?
            .flatten()
            .collect();
        types.sort_by_key(|e| e.file_name());
        for type_entry in types {
            let slot_type = type_entry.file_name().to_string_lossy().into_owned();
            let type_dir = type_entry.path();
            if !type_dir.is_dir() {
                continue;
            }
            let mut slots: Vec<_> = std::fs::read_dir(&type_dir)
                .map_err(|e| format!("cannot list state homes: {e}"))?
                .flatten()
                .collect();
            slots.sort_by_key(|e| e.file_name());
            for slot_entry in slots {
                let slot = slot_entry.file_name().to_string_lossy().into_owned();
                let home = slot_entry.path();
                if !home.is_dir()
                    || active.contains(&(slot_type.clone(), slot.clone()))
                    || !archived.contains(&(slot_type.clone(), slot.clone()))
                {
                    continue;
                }
                rows.push((slot, slot_type.clone(), home));
            }
        }
    }
    let mut outcomes: Vec<Value> = Vec::new();
    for (slot, slot_type, home) in &rows {
        let row = json!({
            "slot": slot,
            "slot_type": slot_type,
            "home": home.to_string_lossy(),
        });
        if !apply {
            let uses: Vec<String> = live_use(home).into_iter().collect();
            outcomes.push(json!({
                "slot": row["slot"], "slot_type": row["slot_type"], "home": row["home"],
                "outcome": if uses.is_empty() { "planned" } else { "retained-live" },
                "live_uses": uses,
            }));
            continue;
        }
        match prepare_reclaim(&cfg.control, slot_type, slot) {
            Ok(bundles) => {
                reclaim_home(&cfg.control, slot_type, slot);
                outcomes.push(json!({
                    "slot": row["slot"], "slot_type": row["slot_type"], "home": row["home"],
                    "outcome": "reclaimed",
                    "salvage_bundles": bundles.iter().map(|b| b.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                }));
            }
            Err(error) => outcomes.push(json!({
                "slot": row["slot"], "slot_type": row["slot_type"], "home": row["home"],
                "outcome": "retained", "reason": error,
            })),
        }
    }
    if format_json {
        let payload = json!({"applied": apply, "orphans": outcomes});
        return Ok(format!(
            "{}\n",
            serde_json::to_string_pretty(&sort_keys(&payload)).unwrap_or_default()
        ));
    }
    let mut out = String::new();
    for item in &outcomes {
        out.push_str(&format!(
            "slot={} slot_type={} outcome={} home={}\n",
            item.get("slot").and_then(Value::as_str).unwrap_or_default(),
            item.get("slot_type")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            item.get("outcome")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            item.get("home").and_then(Value::as_str).unwrap_or_default(),
        ));
    }
    out.push_str(&format!(
        "orphan-state-homes={} applied={}\n",
        outcomes.len(),
        if apply { "True" } else { "False" }
    ));
    Ok(out)
}
