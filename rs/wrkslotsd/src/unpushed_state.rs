//! Reader: unpushed diagnosis (`unpushed`).
//!
//! Reproduces the Python edition's read-only diagnosis: for every
//! checkout of every record on the selected machine, verify the
//! worktree and branch, refresh the remote through the isolated
//! fetch, and report whether any remote-tracking ref contains HEAD,
//! with the two-readings evidence (same-named remote ref and the
//! diagnostic diff command) when none does. Read-only apart from the
//! remote-tracking ref refresh the fetch performs, exactly like the
//! oracle.

use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::active_state::{load_cfg, Cfg};
use crate::config_validate::validate_name;
use crate::ensure_states::refuse_partial_state_allow_seals;
use crate::finish_write::{
    fetch_remote, remote_authority, stored_path, stored_repository_path, verify_existing_worktree,
};
use crate::hold_write::{active_document, validate_global};

type Result<T> = std::result::Result<T, String>;

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-replace-objects",
            "-c",
            "core.useReplaceRefs=false",
            "-C",
        ])
        .arg(repo)
        .args(args)
        .env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            repo.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("Git printed invalid UTF-8 in {}: {error}", repo.display()))
}

fn git_check(repo: &Path, args: &[&str]) -> Result<(i32, String, String)> {
    let mut command = Command::new("git");
    command
        .args([
            "--no-replace-objects",
            "-c",
            "core.useReplaceRefs=false",
            "-C",
        ])
        .arg(repo)
        .args(args)
        .env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with("GIT_") {
            command.env(key, value);
        }
    }
    command
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

/// The registry-storage gate the oracle's `_cmd_unpushed` applies:
/// a storage finding naming the target slot (or any row finding when
/// no slot is named) refuses; other slots' defects are warned about
/// by name and retained.
fn assert_registry_storage(
    cfg: &Cfg,
    root: Option<&Path>,
    slot_filter: Option<&str>,
) -> Result<()> {
    let mut findings: Vec<Value> = Vec::new();
    let fs = crate::storage_state::parse_storage(root)?;
    let git = crate::git_storage_state::parse_storage_git(root)?;
    for doc in [&fs, &git] {
        if let Some(items) = doc.get("findings").and_then(Value::as_array) {
            findings.extend(items.iter().cloned());
        }
    }
    let _ = cfg;
    let mut stale: Vec<String> = Vec::new();
    let mut unexpected: Vec<String> = Vec::new();
    let mut registrations: Vec<String> = Vec::new();
    for finding in &findings {
        let kind = finding
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scope = finding
            .get("scope")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let slot = finding.get("slot").and_then(Value::as_str);
        let slot_type = finding
            .get("slot_type")
            .and_then(Value::as_str)
            .unwrap_or("agent");
        let detail = finding
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if scope == "row" {
            match slot_filter {
                Some(target) if slot == Some(target) => return Err(detail),
                Some(_) => stale.push(format!("{slot_type}:{}", slot.unwrap_or_default())),
                None => return Err(detail),
            }
        } else if kind == "git-registration-without-row" {
            match slot_filter {
                Some(target) if slot == Some(target) => return Err(detail),
                Some(_) => registrations.push(format!("{slot_type}:{}", slot.unwrap_or_default())),
                None => {
                    if slot.is_some() {
                        registrations.push(format!("{slot_type}:{}", slot.unwrap_or_default()));
                    } else {
                        return Err(detail);
                    }
                }
            }
        } else if kind == "directory-without-row" {
            if let Some(name) = slot {
                unexpected.push(format!("{slot_type}:{name}"));
            }
        }
    }
    stale.sort();
    stale.dedup();
    unexpected.sort();
    unexpected.dedup();
    registrations.sort();
    registrations.dedup();
    if !stale.is_empty() {
        let shown: Vec<&str> = stale.iter().take(12).map(String::as_str).collect();
        let more = if stale.len() > 12 {
            format!(" (+{} more)", stale.len() - 12)
        } else {
            String::new()
        };
        eprintln!(
            "WARNING: {} registered slot(s) have missing or inconsistent storage. state: RETAINED -- this command targets different storage, changed none of these rows, and did not inspect them: {}{more}. remedy: these rows outlived their directories; retire each one through its own lifecycle command rather than deleting rows in bulk, because a row is also what protects a slot holding an unread HANDOFF.md",
            stale.len(),
            shown.join(", ")
        );
    }
    if !unexpected.is_empty() {
        let shown: Vec<&str> = unexpected.iter().take(12).map(String::as_str).collect();
        let more = if unexpected.len() > 12 {
            format!(" (+{} more)", unexpected.len() - 12)
        } else {
            String::new()
        };
        eprintln!(
            "WARNING: {} existing unregistered worktree directories remain outside wrkslots records. state: RETAINED -- this command will not inspect, select, or remove them: {}{more}. remedy: run 'wrkslots audit --format json' and import each live slot from verified process evidence",
            unexpected.len(),
            shown.join(", ")
        );
    }
    if !registrations.is_empty() {
        let shown: Vec<&str> = registrations.iter().take(12).map(String::as_str).collect();
        let more = if registrations.len() > 12 {
            format!(" (+{} more)", registrations.len() - 12)
        } else {
            String::new()
        };
        eprintln!(
            "WARNING: {} Git-registered path(s) inside the managed root have no active row. state: RETAINED -- this command named a different target, changed none of them, and did not inspect them: {}{more}. remedy: run 'wrkslots audit --format json', then restore and register each live slot, or retire the stale Git registration through its own lifecycle command; do not delete a directory to repair bookkeeping, because the directory may hold the only reference to unpushed work",
            registrations.len(),
            shown.join(", ")
        );
    }
    Ok(())
}

/// Diagnose unpublished or stale-after-rebase HEADs, printing the
/// oracle's JSON payload (`{"schema": 2, "slots": [...]}`).
pub(crate) fn unpushed(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot_filter: Option<&str>,
) -> Result<Value> {
    if let Some(slot) = slot_filter {
        validate_name(slot, "slot")?;
    }
    let (cfg, selected) = load_cfg(root, machine_override)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    validate_global(root, &cfg)?;
    assert_registry_storage(&cfg, root, slot_filter)?;
    let active = active_document(root, &cfg, &selected)?;
    let records = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut rows: Vec<Value> = Vec::new();
    for record in &records {
        let slot = record
            .get("slot")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(target) = slot_filter {
            if slot != target {
                continue;
            }
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
            let path = stored_path(&cfg, raw_path, "checkout path")?;
            let raw_repository = checkout
                .get("repository")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let (_relative, repository) = stored_repository_path(&cfg, raw_repository)?;
            let head = verify_existing_worktree(&repository, &path)?;
            let (code, stdout, stderr) =
                git_check(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
            let branch = if code == 1 && stdout.is_empty() && stderr.is_empty() {
                return Err(format!(
                    "checkout must be on its registered branch, not detached: {}",
                    path.display()
                ));
            } else if code != 0 {
                return Err(format!(
                    "cannot read the branch of checkout {}: git symbolic-ref failed: {}",
                    path.display(),
                    stderr.trim()
                ));
            } else {
                stdout.trim().to_string()
            };
            let recorded_branch = checkout
                .get("branch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if branch != recorded_branch {
                return Err(format!(
                    "checkout {name} branch changed: expected {recorded_branch}, found {branch}; preserve it for inspection"
                ));
            }
            let remote = checkout
                .get("remote")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let (authority_sha, url) = remote_authority(&path, &remote)?;
            let recorded_sha = checkout
                .get("remote_url_sha256")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if authority_sha != recorded_sha {
                return Err(format!("checkout {name} remote {remote} URL changed"));
            }
            let landed_ref = checkout
                .get("landed_ref")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            fetch_remote(&path, &remote, &landed_ref, &url)?;
            let (after_sha, _after_url) = remote_authority(&path, &remote)?;
            if after_sha != authority_sha {
                return Err(format!(
                    "remote {remote:?} changed after authority was captured; preserve the checkout and restore its recorded remote"
                ));
            }
            let containing_output = git(
                &path,
                &[
                    "for-each-ref",
                    "--format=%(refname)",
                    "--contains",
                    &head,
                    &format!("refs/remotes/{remote}/"),
                ],
            )?;
            let containing: Vec<String> = containing_output
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect();
            let same_named_ref = format!("refs/remotes/{remote}/{branch}");
            let (exists_code, _out, _err) =
                git_check(&path, &["show-ref", "--verify", "--quiet", &same_named_ref])?;
            if exists_code != 0 && exists_code != 1 {
                return Err(format!(
                    "cannot determine whether ref {same_named_ref} exists"
                ));
            }
            let same_named = if exists_code == 0 {
                Value::String(same_named_ref.clone())
            } else {
                Value::Null
            };
            let paths_output = git(
                &path,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    "-m",
                    "-z",
                    &head,
                ],
            )?;
            let mut touched: Vec<String> = Vec::new();
            for candidate in paths_output.split('\0') {
                if !candidate.is_empty() && !touched.iter().any(|item| item == candidate) {
                    touched.push(candidate.to_string());
                }
            }
            let diagnostic = if !containing.is_empty() {
                Value::Null
            } else if exists_code == 0 {
                let mut parts = vec![
                    "git".to_string(),
                    "--literal-pathspecs".to_string(),
                    "-C".to_string(),
                    path.to_string_lossy().into_owned(),
                    "diff".to_string(),
                    head.clone(),
                    same_named_ref,
                    "--".to_string(),
                ];
                parts.extend(touched.iter().cloned());
                Value::String(shell_join(&parts))
            } else {
                Value::Null
            };
            rows.push(json!({
                "slot": slot,
                "checkout": name,
                "head": head,
                "state": if containing.is_empty() { "UNPUSHED-OR-STALE" } else { "PUBLISHED" },
                "containing_remote_refs": containing,
                "same_named_remote_ref": same_named,
                "touched_files": touched,
                "diagnostic_command": diagnostic,
            }));
        }
    }
    Ok(json!({"schema": 2, "slots": rows}))
}

/// `shlex.join` for the diagnostic command: quote only what needs it.
fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| {
            if !part.is_empty()
                && part.chars().all(|ch| {
                    ch.is_ascii_alphanumeric()
                        || matches!(
                            ch,
                            '@' | '%' | '_' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
                        )
                })
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\"'\"'"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
