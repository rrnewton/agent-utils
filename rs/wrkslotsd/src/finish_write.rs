//! Writer: record a slot's finish handoff in its active record.
//!
//! Reproduces the existing `finish` mutation: owner authorization,
//! the seal and cleanup-journal guards, the handoff preconditions
//! (record paths, slot contents, and per-checkout Git safety:
//! worktree identity, branch, unfinished operations, ordinary
//! history and index, cache policy, clean status, remote
//! authority, fetch, containment, and landed ancestry), and the
//! single `handoff-recorded` active-state event. The remote fetch
//! updates the checkout's tracking refs in place to the advertised
//! inventory rather than through an isolated repository, and
//! slot-image records stay with the Python edition: their presence
//! refuses here. Fixture registries only until the edition cutover
//! (see `ensure_states::require_fixture`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{landed_ref_for_remote, load_cfg, slot_directory, Cfg};
use crate::canonical::canonical_sha256;
use crate::config_validate::{load_config, validate_name};
use crate::ensure_states::{
    append_event, atomic_write_json, ensure_event_log, refuse_partial_state_allow_seals,
    require_fixture, utc_now,
};
use crate::heartbeat_write::{assert_caller, refuse_seal_overlap_action, seal_paths};
use crate::hold_write::{
    acquire_mutation_locks, active_document, refuse_target_storage, validate_global,
};
use crate::journals_state::parse_journals;
use crate::status_state::{process_state, read_process_identity, Identity};
use crate::storage_state::py_repr;

type Result<T> = std::result::Result<T, String>;

const OPERATION_STATE_NAMES: [&str; 7] = [
    "MERGE_HEAD",
    "CHERRY_PICK_HEAD",
    "REVERT_HEAD",
    "BISECT_LOG",
    "rebase-apply",
    "rebase-merge",
    "sequencer",
];

struct GitOutput {
    code: i32,
    stdout: String,
    stderr: String,
}

fn git_run(repo: &Path, args: &[&str], dev_null_dir: bool) -> Result<GitOutput> {
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
    if dev_null_dir {
        command.env("GIT_DIR", "/dev/null");
    }
    let output = command
        .output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    Ok(GitOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = git_run(repo, args, false)?;
    if output.code != 0 {
        let detail = output.stderr.trim();
        return Err(format!(
            "Git refused in {}: git {}{}",
            repo.display(),
            args.join(" "),
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    Ok(output.stdout)
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn ensure_no_symlink_components(root: &Path, path: &Path, label: &str) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| format!("{label} escapes the project root: {}", path.display()))?;
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(format!("{label} crosses a symlink: {}", current.display()));
                }
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect {label} path {}: {error}",
                    current.display()
                ));
            }
        }
    }
    Ok(())
}

fn relative_inside(root: &Path, raw: &str, label: &str) -> Result<(String, PathBuf)> {
    let candidate = Path::new(raw);
    let parts: Vec<String> = candidate
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if candidate.is_absolute() || parts.is_empty() || parts.iter().any(|part| part == "..") {
        return Err(format!(
            "{label} must be a relative path inside {}: {}",
            root.display(),
            py_repr(raw)
        ));
    }
    let kept: Vec<&str> = parts
        .iter()
        .map(String::as_str)
        .filter(|part| *part != ".")
        .collect();
    if kept.is_empty() {
        return Err(format!("{label} must not be the project root"));
    }
    let lexical = kept.join("/");
    let mut absolute = root.to_path_buf();
    for part in &kept {
        absolute.push(part);
    }
    Ok((lexical, absolute))
}

pub(crate) fn stored_path(cfg: &Cfg, raw: &str, label: &str) -> Result<PathBuf> {
    let (_lexical, absolute) = relative_inside(&cfg.root, raw, label)?;
    ensure_no_symlink_components(&cfg.root, &absolute, label)?;
    Ok(absolute)
}

fn managed_worktrees_root(cfg: &Cfg) -> PathBuf {
    if cfg
        .worktrees
        .file_name()
        .map(|name| name == "slots")
        .unwrap_or(false)
    {
        cfg.worktrees
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cfg.worktrees.clone())
    } else {
        cfg.worktrees.clone()
    }
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

pub(crate) fn stored_repository_path(cfg: &Cfg, raw: &str) -> Result<(String, PathBuf)> {
    let candidate = Path::new(raw);
    let parts: Vec<String> = candidate
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let (stored, unresolved, allowed_root) = if candidate.is_absolute() {
        let normalized = parts.join("/");
        let normalized = if normalized.is_empty() {
            normalized
        } else {
            format!("/{normalized}")
        };
        if raw != normalized {
            return Err(format!(
                "stored absolute repository path is not canonical: {}",
                py_repr(raw)
            ));
        }
        let parent = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
        let name = parts.last().cloned().unwrap_or_default();
        if candidate == cfg.root
            || candidate == parent
            || name.is_empty()
            || candidate.parent() != Some(parent.as_path())
        {
            return Err(format!(
                "stored absolute repository path must name one direct sibling of the project root, not the project root or another external path: {}",
                py_repr(raw)
            ));
        }
        (format!("../{name}"), candidate.to_path_buf(), parent)
    } else {
        let sibling = parts.len() >= 2
            && parts[0] == ".."
            && parts[1..].iter().all(|part| part != "." && part != "..");
        if parts.iter().any(|part| part == "..") && !sibling {
            return Err(format!(
                "stored repository path must stay inside {} or one direct sibling, with path components '../NAME' or '../NAME/PATH': {}",
                cfg.root.display(),
                py_repr(raw)
            ));
        }
        let kept: Vec<&str> = parts
            .iter()
            .map(String::as_str)
            .filter(|part| *part != ".")
            .collect();
        if sibling {
            let parent = cfg.root.parent().map(Path::to_path_buf).unwrap_or_default();
            if parent.join(&parts[1]) == cfg.root {
                return Err(format!(
                    "stored repository path {} aliases the project root; use a path inside it instead",
                    py_repr(raw)
                ));
            }
            let mut unresolved = parent.clone();
            for part in &parts[1..] {
                unresolved.push(part);
            }
            (kept.join("/"), unresolved, parent)
        } else {
            let stored = if kept.is_empty() {
                ".".to_string()
            } else {
                kept.join("/")
            };
            let mut unresolved = cfg.root.clone();
            for part in &kept {
                unresolved.push(part);
            }
            (stored, unresolved, cfg.root.clone())
        }
    };
    ensure_no_symlink_components(&allowed_root, &unresolved, "repository")?;
    if path_is_within(&unresolved, &managed_worktrees_root(cfg)) {
        return Err(format!(
            "source repository must be outside the managed worktrees directory: {}",
            unresolved.display()
        ));
    }
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
    if path_is_within(&absolute, &managed_worktrees_root(cfg)) {
        return Err(format!(
            "source repository must be outside the managed worktrees directory: {}",
            absolute.display()
        ));
    }
    Ok((stored, absolute))
}

pub(crate) fn common_directory(repository: &Path) -> Result<PathBuf> {
    let output = git(
        repository,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let common = PathBuf::from(output.trim());
    let common = if common.is_absolute() {
        common
    } else {
        repository.join(&common)
    };
    if common.is_symlink() || !common.is_dir() {
        return Err(format!(
            "Git reported an unsafe common directory: {}",
            common.display()
        ));
    }
    Ok(common)
}

pub(crate) fn verify_existing_worktree(repository: &Path, checkout: &Path) -> Result<String> {
    let source_common = common_directory(repository)?;
    let root_output = git(checkout, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root_output.trim());
    if root.is_symlink() || !root.is_dir() {
        return Err(format!(
            "Git reported an unsafe repository root: {}",
            root.display()
        ));
    }
    if root != checkout {
        return Err(format!(
            "checkout path is not its Git worktree root: {} (root {})",
            checkout.display(),
            root.display()
        ));
    }
    if common_directory(checkout)? != source_common {
        return Err(format!(
            "checkout {} does not belong to repository {}",
            checkout.display(),
            repository.display()
        ));
    }
    let listing = git(repository, &["worktree", "list", "--porcelain"])?;
    let listed = listing.lines().any(|line| {
        line.strip_prefix("worktree ")
            .map(|path| Path::new(path.trim()) == checkout)
            .unwrap_or(false)
    });
    if !listed {
        return Err(format!(
            "Git does not list {} as a worktree of {}",
            checkout.display(),
            repository.display()
        ));
    }
    let head_output = git(checkout, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = head_output.trim().to_string();
    if !is_sha(&head) {
        return Err(format!(
            "checkout HEAD is not a full commit: {}",
            checkout.display()
        ));
    }
    Ok(head)
}

fn checkout_branch(checkout: &Path) -> Result<String> {
    let output = git_run(
        checkout,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        false,
    )?;
    if output.code == 1 && output.stdout.is_empty() && output.stderr.is_empty() {
        return Err(format!(
            "checkout must be on its registered branch, not detached: {}",
            checkout.display()
        ));
    }
    if output.code != 0 {
        return Err(format!(
            "cannot read the branch of checkout {}: git symbolic-ref failed: {}",
            checkout.display(),
            output.stderr.trim()
        ));
    }
    let branch = output.stdout.trim().to_string();
    let check = git_run(
        checkout,
        &["check-ref-format", "--allow-onelevel", &branch],
        false,
    )?;
    if check.code != 0 || branch.is_empty() {
        return Err(format!("invalid branch {branch:?}"));
    }
    Ok(branch)
}

pub(crate) fn glob_parts(pattern: &str) -> Vec<String> {
    pattern
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn fnmatch_component(pattern: &str, text: &str) -> bool {
    // fnmatch.fnmatchcase for one path component: `*`, `?`, `[...]`.
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    fn inner(pattern: &[char], text: &[char]) -> bool {
        if pattern.is_empty() {
            return text.is_empty();
        }
        match pattern[0] {
            '*' => {
                for skip in 0..=text.len() {
                    if inner(&pattern[1..], &text[skip..]) {
                        return true;
                    }
                }
                false
            }
            '?' => !text.is_empty() && inner(&pattern[1..], &text[1..]),
            '[' => {
                if text.is_empty() {
                    return false;
                }
                let mut index = 1;
                // fnmatch negation is `[!...]` only; `^` is a literal member.
                let negate = matches!(pattern.get(index), Some('!'));
                if negate {
                    index += 1;
                }
                let mut matched = false;
                let mut first = true;
                while index < pattern.len() && (pattern[index] != ']' || first) {
                    first = false;
                    if index + 2 < pattern.len()
                        && pattern[index + 1] == '-'
                        && pattern[index + 2] != ']'
                    {
                        if pattern[index] <= text[0] && text[0] <= pattern[index + 2] {
                            matched = true;
                        }
                        index += 3;
                    } else {
                        if pattern[index] == text[0] {
                            matched = true;
                        }
                        index += 1;
                    }
                }
                if index >= pattern.len() {
                    // Unterminated class: fnmatch treats `[` literally.
                    return text[0] == '[' && inner(&pattern[1..], &text[1..]);
                }
                (matched != negate) && inner(&pattern[index + 1..], &text[1..])
            }
            other => !text.is_empty() && text[0] == other && inner(&pattern[1..], &text[1..]),
        }
    }
    inner(&pattern, &text)
}

pub(crate) fn glob_parts_match(pattern: &[String], path: &[String]) -> bool {
    if !pattern
        .iter()
        .any(|part| part.chars().any(|ch| matches!(ch, '*' | '?' | '[')))
    {
        return pattern == path;
    }
    fn inner(pattern: &[String], path: &[String]) -> bool {
        if pattern.is_empty() {
            return path.is_empty();
        }
        if pattern[0] == "**" {
            return inner(&pattern[1..], path) || (!path.is_empty() && inner(pattern, &path[1..]));
        }
        !path.is_empty()
            && fnmatch_component(&pattern[0], &path[0])
            && inner(&pattern[1..], &path[1..])
    }
    inner(pattern, path)
}

pub(crate) fn cache_glob_contains_path(pattern: &str, path: &str) -> bool {
    let pattern_parts = glob_parts(pattern);
    if pattern_parts.is_empty() {
        return false;
    }
    let path_parts: Vec<String> = path
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if pattern_parts.iter().any(|part| part == "**") {
        return (1..=path_parts.len())
            .any(|length| glob_parts_match(&pattern_parts, &path_parts[..length]));
    }
    if pattern_parts.len() > path_parts.len() {
        return false;
    }
    glob_parts_match(&pattern_parts, &path_parts[..pattern_parts.len()])
}

/// The oracle's `_assert_cache_policy_untracked_path`: configured
/// cache globs must not overlap tracked source or cross a submodule.
pub(crate) fn assert_cache_policy_untracked(
    path: &Path,
    name: &str,
    globs: &[String],
) -> Result<()> {
    if globs.is_empty() {
        return Ok(());
    }
    let indexed = git(path, &["ls-files", "--recurse-submodules", "-z"])?;
    let committed = git(path, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])?;
    let mut tracked: Vec<String> = Vec::new();
    for output in [&indexed, &committed] {
        for candidate in output.split('\0') {
            if !candidate.is_empty()
                && globs
                    .iter()
                    .any(|glob| cache_glob_contains_path(glob, candidate))
                && !tracked.iter().any(|existing| existing == candidate)
            {
                tracked.push(candidate.to_string());
            }
        }
    }
    if let Some(first) = tracked.first() {
        return Err(format!(
            "configured cache policy for checkout {name} overlaps tracked source at {first}; narrow cache_globs before cleanup or finish"
        ));
    }
    let stage = git(path, &["ls-files", "--stage", "-z"])?;
    let tree = git(path, &["ls-tree", "-r", "-z", "HEAD"])?;
    let mut submodules: Vec<String> = Vec::new();
    for output in [&stage, &tree] {
        for record in output.split('\0') {
            if record.is_empty() {
                continue;
            }
            if let Some((metadata, sub_path)) = record.split_once('\t') {
                if metadata.starts_with("160000 ")
                    && !submodules.iter().any(|existing| existing == sub_path)
                {
                    submodules.push(sub_path.to_string());
                }
            }
        }
    }
    for submodule in &submodules {
        let submodule_parts: Vec<String> = submodule
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
        for pattern in globs {
            let pattern_parts = glob_parts(pattern);
            let wildcard_index = pattern_parts
                .iter()
                .position(|part| part.chars().any(|ch| matches!(ch, '*' | '?' | '[')))
                .unwrap_or(pattern_parts.len());
            let overlaps = if wildcard_index == pattern_parts.len() {
                submodule_parts[..pattern_parts.len().min(submodule_parts.len())]
                    == pattern_parts[..pattern_parts.len().min(submodule_parts.len())]
                    && (submodule_parts.len() >= pattern_parts.len()
                        || pattern_parts[..submodule_parts.len()] == submodule_parts[..])
                    || pattern_parts[..submodule_parts.len().min(pattern_parts.len())]
                        == submodule_parts[..submodule_parts.len().min(pattern_parts.len())]
            } else {
                let literal_prefix = &pattern_parts[..wildcard_index];
                submodule_parts[..literal_prefix.len().min(submodule_parts.len())]
                    == literal_prefix[..literal_prefix.len().min(submodule_parts.len())]
                    || literal_prefix[..submodule_parts.len().min(literal_prefix.len())]
                        == submodule_parts[..submodule_parts.len().min(literal_prefix.len())]
            };
            if overlaps {
                return Err(format!(
                    "configured cache policy for checkout {name} may cross Git submodule {submodule}; configure that repository separately"
                ));
            }
        }
    }
    Ok(())
}

pub(crate) fn cache_globs_for(config: &Value, checkout_name: &str) -> Vec<String> {
    let mut selected: Vec<String> = Vec::new();
    if let Some(items) = config.get("cache_globs").and_then(Value::as_array) {
        for item in items {
            if let Some(text) = item.as_str() {
                if !selected.iter().any(|existing| existing == text) {
                    selected.push(text.to_string());
                }
            }
        }
    }
    if let Some(groups) = config.get("repo_cache_globs").and_then(Value::as_array) {
        for group in groups {
            if let Some(pair) = group.as_array() {
                if pair.first().and_then(Value::as_str) == Some(checkout_name) {
                    if let Some(items) = pair.get(1).and_then(Value::as_array) {
                        for item in items {
                            if let Some(text) = item.as_str() {
                                if !selected.iter().any(|existing| existing == text) {
                                    selected.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    selected
}

pub(crate) fn remote_authority(checkout: &Path, remote: &str) -> Result<(String, String)> {
    let push = git_run(
        checkout,
        &[
            "config",
            "--null",
            "--get-all",
            &format!("remote.{remote}.pushurl"),
        ],
        false,
    )?;
    if push.code != 0 && push.code != 1 {
        return Err(format!(
            "cannot determine whether remote {remote:?} has a push URL: {}",
            push.stderr.trim()
        ));
    }
    let values: Vec<&str> = push.stdout.split('\0').collect();
    if values.iter().any(|value| !value.is_empty()) {
        return Err(format!(
            "remote {remote:?} has a configured push URL; remove every non-empty remote.{remote}.pushurl before remote operations"
        ));
    }
    let fetch_output = git(checkout, &["remote", "get-url", "--all", remote])?;
    let urls: Vec<&str> = fetch_output.lines().collect();
    if urls.len() != 1 || urls[0].is_empty() {
        return Err(format!(
            "remote {remote:?} must have exactly one non-empty fetch URL"
        ));
    }
    let push_output = git(checkout, &["remote", "get-url", "--push", "--all", remote])?;
    let push_urls: Vec<&str> = push_output.lines().collect();
    if push_urls != urls {
        return Err(format!(
            "remote {remote:?} effective push URL differs from its single fetch URL; remove pushInsteadOf or other push-only redirection"
        ));
    }
    let url = urls[0].to_string();
    if url.starts_with('-') {
        return Err(format!(
            "remote {remote:?} URL cannot begin with '-' for isolated transport"
        ));
    }
    Ok((format!("{:x}", Sha256::digest(url.as_bytes())), url))
}

fn git_stdin(repo: &Path, args: &[&str], input: &str) -> Result<String> {
    use std::io::Write as _;
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
        .env_clear()
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
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
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(input.as_bytes())
            .map_err(|error| format!("cannot write Git input: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot execute Git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Git refused in {}: git {}: {}",
            repo.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn ref_inventory(checkout: &Path, prefix: &str) -> Result<BTreeMap<String, String>> {
    let output = git(
        checkout,
        &[
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(symref)",
            prefix,
        ],
    )?;
    let mut refs: BTreeMap<String, String> = BTreeMap::new();
    for line in output.lines() {
        let fields: Vec<&str> = line.split('\0').collect();
        if fields.len() != 3 {
            return Err("Git returned a malformed remote-ref inventory".to_string());
        }
        if !fields[2].is_empty() {
            continue;
        }
        if !is_sha(fields[1]) || refs.contains_key(fields[0]) {
            return Err("Git returned an ambiguous remote-ref inventory".to_string());
        }
        refs.insert(fields[0].to_string(), fields[1].to_string());
    }
    Ok(refs)
}

pub(crate) fn fetch_remote(
    checkout: &Path,
    remote: &str,
    landed_ref: &str,
    url: &str,
) -> Result<()> {
    let prefix = format!("refs/remotes/{remote}/");
    if !landed_ref.starts_with(&prefix) {
        return Err(format!("landed ref must be under {prefix}: {landed_ref}"));
    }
    let listed = git_run(
        checkout,
        &["ls-remote", "--refs", "--heads", "--", url],
        true,
    )?;
    if listed.code != 0 {
        return Err(format!(
            "Git refused in {}: git ls-remote --refs --heads -- {}: {}",
            checkout.display(),
            url,
            listed.stderr.trim()
        ));
    }
    let mut advertised: BTreeMap<String, String> = BTreeMap::new();
    for line in listed.stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != 2 {
            return Err("Git returned a malformed remote-ref inventory".to_string());
        }
        let (head, reference) = (fields[0], fields[1]);
        if !is_sha(head) || advertised.contains_key(reference) {
            return Err("Git returned an ambiguous remote-ref inventory".to_string());
        }
        advertised.insert(reference.to_string(), head.to_string());
    }
    let expected: BTreeMap<String, String> = advertised
        .iter()
        .map(|(reference, head)| {
            (
                format!(
                    "{prefix}{}",
                    reference.strip_prefix("refs/heads/").unwrap_or(reference)
                ),
                head.clone(),
            )
        })
        .collect();
    let existing = ref_inventory(checkout, &prefix)?;
    // Fetch into a scratch namespace first: a failed or raced fetch
    // must not move the checkout's tracking refs. They change only in
    // one verified update-ref transaction against the old values.
    const SCRATCH: &str = "refs/wrkslotsd-finish-fetch/";
    let scratch_inventory = |checkout: &Path| ref_inventory(checkout, SCRATCH);
    if !advertised.is_empty() {
        let fetched = (|| -> Result<()> {
            git(
                checkout,
                &[
                    "fetch",
                    "--no-tags",
                    "--no-write-fetch-head",
                    "--no-auto-maintenance",
                    "--force",
                    "--no-recurse-submodules",
                    url,
                    &format!("+refs/heads/*:{SCRATCH}*"),
                ],
            )?;
            let transferred = scratch_inventory(checkout)?;
            let transferred_expected: BTreeMap<String, String> = transferred
                .iter()
                .map(|(reference, head)| {
                    (
                        format!(
                            "{prefix}{}",
                            reference.strip_prefix(SCRATCH).unwrap_or(reference)
                        ),
                        head.clone(),
                    )
                })
                .collect();
            if transferred_expected != expected {
                return Err(
                    "Git fetched a different head inventory than the metadata inventory"
                        .to_string(),
                );
            }
            Ok(())
        })();
        if let Err(error) = fetched {
            for reference in scratch_inventory(checkout).unwrap_or_default().keys() {
                let _ = git(checkout, &["update-ref", "-d", reference]);
            }
            return Err(error);
        }
    }
    let zero = "0".repeat(40);
    let mut updates: Vec<String> = vec!["start".to_string()];
    for (reference, new_head) in &expected {
        let old_head = existing.get(reference).unwrap_or(&zero);
        if old_head != new_head {
            updates.push(format!("update {reference} {new_head} {old_head}"));
        }
    }
    for (reference, old_head) in &existing {
        if !expected.contains_key(reference) {
            updates.push(format!("delete {reference} {old_head}"));
        }
    }
    if updates.len() > 1 {
        updates.push("prepare".to_string());
        updates.push("commit".to_string());
        updates.push(String::new());
        git_stdin(checkout, &["update-ref", "--stdin"], &updates.join("\n"))?;
    }
    for reference in scratch_inventory(checkout).unwrap_or_default().keys() {
        let _ = git(checkout, &["update-ref", "-d", reference]);
    }
    if ref_inventory(checkout, &prefix)? != expected {
        return Err(
            "Git fetched a different head inventory than the metadata inventory".to_string(),
        );
    }
    Ok(())
}

fn unpushed_refusal(checkout_path: &Path, checkout: &Value, head: &str) -> Result<String> {
    let name = checkout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let remote = checkout
        .get("remote")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let branch = checkout
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let same_named = format!("refs/remotes/{remote}/{branch}");
    let shown = git_run(
        checkout_path,
        &["show-ref", "--verify", "--quiet", &same_named],
        false,
    )?;
    if shown.code != 0 && shown.code != 1 {
        return Err(format!(
            "cannot determine whether ref {same_named} exists: {}",
            shown.stderr.trim()
        ));
    }
    let base = format!(
        "checkout {name} HEAD {head} is not reachable from any refs/remotes/{remote}/* ref"
    );
    if shown.code == 1 {
        return Ok(format!(
            "{base}. A same-named remote ref does not exist; publish the branch before finishing"
        ));
    }
    let touched_output = git(
        checkout_path,
        &[
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-m",
            "-z",
            head,
        ],
    )?;
    let mut touched: Vec<String> = Vec::new();
    for path in touched_output.split('\0') {
        if !path.is_empty() && !touched.iter().any(|existing| existing == path) {
            touched.push(path.to_string());
        }
    }
    let raw_checkout = checkout_path.to_string_lossy().into_owned();
    let shown_checkout = if raw_checkout.chars().all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '_' | '@' | '%' | '+' | '=' | ':' | ',' | '.' | '/' | '-'
            )
    }) {
        raw_checkout
    } else {
        format!("'{}'", raw_checkout.replace('\'', "'\\''"))
    };
    let mut command =
        format!("git --literal-pathspecs -C {shown_checkout} diff {head} {same_named} --");
    for path in &touched {
        command.push(' ');
        if path.chars().any(|ch| ch.is_whitespace()) {
            command.push_str(&format!("'{}'", path.replace('\'', "'\\''")));
        } else {
            command.push_str(path);
        }
    }
    Ok(format!(
        "{base}. Same-named remote ref {same_named} exists, so there are two opposite readings: this commit is genuinely unpushed, or the remote branch was rebased and this local tip is stale. Do not force-push based on this refusal. Run `{command}`; empty output means the touched content is already present at the remote tip"
    ))
}

pub(crate) fn assert_checkout_safe(
    cfg: &Cfg,
    config: &Value,
    checkout: &Value,
    repository: &Path,
) -> Result<Value> {
    let name = checkout
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let raw_path = checkout
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let path = stored_path(cfg, raw_path, "checkout path")?;
    let head = verify_existing_worktree(repository, &path)?;
    let branch = checkout_branch(&path)?;
    let recorded_branch = checkout
        .get("branch")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if branch != recorded_branch {
        return Err(format!(
            "checkout {name} branch changed: expected {recorded_branch}, found {branch}"
        ));
    }
    let mut operation_args: Vec<&str> = vec!["rev-parse"];
    for name_ref in OPERATION_STATE_NAMES {
        operation_args.push("--git-path");
        operation_args.push(name_ref);
    }
    let operations = git(&path, &operation_args)?;
    let operation_lines: Vec<&str> = operations.lines().collect();
    if operation_lines.len() != OPERATION_STATE_NAMES.len() {
        return Err("Git returned an incomplete operation-path inventory".to_string());
    }
    for raw in &operation_lines {
        let candidate = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            path.join(raw)
        };
        if candidate.exists() || candidate.is_symlink() {
            return Err(format!(
                "checkout {name} has an unfinished Git operation: {}",
                candidate.display()
            ));
        }
    }
    let common = common_directory(&path)?;
    let grafts = common.join("info").join("grafts");
    if grafts.exists() || grafts.is_symlink() {
        return Err(format!("checkout uses graft history: {}", grafts.display()));
    }
    let shallow = common.join("shallow");
    if shallow.exists() || shallow.is_symlink() {
        return Err(format!(
            "checkout uses shallow history: {}",
            shallow.display()
        ));
    }
    let replacements = git(
        &path,
        &["for-each-ref", "--format=%(refname)", "refs/replace/"],
    )?;
    if !replacements.trim().is_empty() {
        return Err(format!(
            "checkout contains replacement refs: {}",
            replacements.lines().next().unwrap_or_default()
        ));
    }
    let assume = git(&path, &["ls-files", "-v"])?;
    for line in assume.lines() {
        if line
            .chars()
            .next()
            .map(|ch| ch.is_ascii_lowercase())
            .unwrap_or(false)
        {
            return Err(format!(
                "checkout has assume-unchanged index state: {}",
                line.get(2..).unwrap_or_default()
            ));
        }
    }
    let skip = git(&path, &["ls-files", "-t"])?;
    for line in skip.lines() {
        if let Some(rest) = line.strip_prefix("S ") {
            return Err(format!("checkout has skip-worktree index state: {rest}"));
        }
    }
    let globs = cache_globs_for(config, &name);
    assert_cache_policy_untracked(&path, &name, &globs)?;
    let mut status_args: Vec<String> = vec![
        "status".to_string(),
        "--porcelain=v2".to_string(),
        "--untracked-files=all".to_string(),
        "--ignored=matching".to_string(),
        "--".to_string(),
        ".".to_string(),
    ];
    for pattern in &globs {
        status_args.push(format!(":(exclude,glob){pattern}"));
        status_args.push(format!(":(exclude,glob){pattern}/**"));
    }
    let status_refs: Vec<&str> = status_args.iter().map(String::as_str).collect();
    let status = git(&path, &status_refs)?;
    if !status.is_empty() {
        let first = status.lines().next().unwrap_or_default();
        return Err(format!(
            "checkout {name} is dirty or has untracked/ignored files ({first}); commit intended work and publish it before finishing; keep a handoff out of commits (use write-handoff)"
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
        return Err(format!(
            "checkout {name} remote {remote} URL changed; preserve the checkout for inspection"
        ));
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
    let refs_output = git(
        &path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "--contains",
            &head,
            &format!("refs/remotes/{remote}/"),
        ],
    )?;
    let remote_refs: Vec<String> = refs_output
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if remote_refs.is_empty() {
        return Err(unpushed_refusal(&path, checkout, &head)?);
    }
    let verify = git_run(
        &path,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{landed_ref}^{{commit}}"),
        ],
        false,
    )?;
    if verify.code != 0 {
        return Err(format!(
            "cannot verify landed ref {landed_ref} in checkout {name}"
        ));
    }
    let ancestor = git_run(
        &path,
        &["merge-base", "--is-ancestor", &head, &landed_ref],
        false,
    )?;
    if ancestor.code != 0 && ancestor.code != 1 {
        return Err(format!(
            "cannot compare checkout HEAD {head} with landed ref {landed_ref}: {}",
            ancestor.stderr.trim()
        ));
    }
    if ancestor.code == 1 {
        return Err(format!(
            "checkout {name} HEAD {head} is not an ancestor of configured landed ref {landed_ref}; no PR label or status substitutes for ancestry"
        ));
    }
    let mut updated = checkout.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("head".to_string(), json!(head));
        object.insert("containing_remote_refs".to_string(), json!(remote_refs));
    }
    Ok(updated)
}

pub(crate) fn record_layout(cfg: &Cfg, record: &Value) -> Result<String> {
    let layout = record
        .get("layout")
        .and_then(Value::as_str)
        .unwrap_or(&cfg.layout)
        .to_string();
    if !matches!(layout.as_str(), "nested" | "flat") {
        return Err(format!(
            "invalid layout {}; expected 'nested' or 'flat'",
            py_repr(&layout)
        ));
    }
    Ok(layout)
}

pub(crate) fn assert_record_paths(cfg: &Cfg, record: &Value) -> Result<()> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let slot_dir = slot_directory(cfg, slot, slot_type)?;
    ensure_no_symlink_components(&cfg.root, &slot_dir, "slot")?;
    let layout = record_layout(cfg, record)?;
    let checkouts = record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if layout == "flat" && checkouts.len() != 1 {
        return Err(format!(
            "slot {slot} has multiple checkouts under flat layout"
        ));
    }
    for checkout in &checkouts {
        let name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        validate_name(name, "checkout name")?;
        let remote = checkout
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_landed = landed_ref_for_remote(cfg, remote)?;
        if checkout.get("landed_ref").and_then(Value::as_str) != Some(expected_landed.as_str()) {
            return Err(format!(
                "slot {slot} checkout {name} differs from configured authority"
            ));
        }
        let expected_abs = if layout == "flat" {
            slot_dir.clone()
        } else {
            slot_dir.join(name)
        };
        ensure_no_symlink_components(&cfg.root, &expected_abs, "checkout")?;
        let expected_relative = expected_abs
            .strip_prefix(&cfg.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        let recorded_path = checkout
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if recorded_path != expected_relative {
            return Err(format!(
                "slot {slot} checkout {name} path does not match its slot: {} != {}",
                py_repr(recorded_path),
                py_repr(&expected_relative)
            ));
        }
        let actual = stored_path(cfg, recorded_path, "checkout path")?;
        if actual != expected_abs {
            return Err(format!("slot {slot} checkout path identity changed"));
        }
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        stored_repository_path(cfg, repository_raw)?;
    }
    Ok(())
}

pub(crate) fn assert_slot_contents(cfg: &Cfg, record: &Value) -> Result<PathBuf> {
    let slot = record
        .get("slot")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let slot_type = record
        .get("slot_type")
        .and_then(Value::as_str)
        .unwrap_or("agent");
    let slot_dir = slot_directory(cfg, slot, slot_type)?;
    let metadata = std::fs::symlink_metadata(&slot_dir);
    let unsafe_dir = match &metadata {
        Ok(found) => !found.is_dir() || found.file_type().is_symlink(),
        Err(_) => true,
    };
    if unsafe_dir {
        return Err(format!(
            "slot directory is missing or unsafe: {}",
            slot_dir.display()
        ));
    }
    let image_record = cfg
        .control
        .join("slot-images")
        .join(slot_type)
        .join(slot)
        .join("IMAGE.json");
    if image_record.exists() || image_record.is_symlink() {
        return Err(format!(
            "slot {slot} is image-backed; finish stays with the Python edition for image-backed slots"
        ));
    }
    let layout = record_layout(cfg, record)?;
    let checkouts = record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if layout == "flat" {
        if checkouts.len() != 1 {
            return Err(format!(
                "slot {slot} does not have exactly one checkout under flat layout"
            ));
        }
        let expected_relative = slot_dir
            .strip_prefix(&cfg.root)
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if checkouts[0].get("path").and_then(Value::as_str) != Some(expected_relative.as_str()) {
            return Err(format!(
                "slot {slot} checkout path does not equal the flat slot root"
            ));
        }
        return Ok(slot_dir);
    }
    let expected_names: Vec<String> = checkouts
        .iter()
        .filter_map(|checkout| {
            checkout
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut allowed: Vec<String> = expected_names.clone();
    if slot_type == "agent" {
        allowed.push("HANDOFF.md".to_string());
    }
    let entries = std::fs::read_dir(&slot_dir).map_err(|error| {
        format!(
            "cannot inspect slot directory {}: {error}",
            slot_dir.display()
        )
    })?;
    let mut actual: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "cannot inspect slot directory {}: {error}",
                slot_dir.display()
            )
        })?;
        actual.push(entry.file_name().to_string_lossy().into_owned());
    }
    let extras: Vec<&String> = actual
        .iter()
        .filter(|name| !allowed.iter().any(|item| item == *name))
        .collect();
    let missing: Vec<&String> = expected_names
        .iter()
        .filter(|name| !actual.iter().any(|item| item == *name))
        .collect();
    if !extras.is_empty() || !missing.is_empty() {
        let mut details: Vec<String> = Vec::new();
        if !extras.is_empty() {
            details.push(format!(
                "unexpected entries {}",
                extras
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !missing.is_empty() {
            details.push(format!(
                "missing checkouts {}",
                missing
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        return Err(format!(
            "slot directory does not match its record: {}",
            details.join("; ")
        ));
    }
    Ok(slot_dir)
}

/// Record the finish handoff for the named slot.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish_slot(
    root: Option<&Path>,
    machine_override: Option<&str>,
    slot: &str,
    agent: &str,
    owner_pid: i64,
    expected_generation: i64,
    validation: &[String],
    limitations: &[String],
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    let config = load_config(root, machine_override)?;
    validate_name(&selected, "machine")?;
    validate_name(slot, "slot")?;
    validate_name(agent, "agent")?;
    if validation.is_empty() || validation.iter().any(|item| item.trim().is_empty()) {
        return Err("finish requires at least one non-empty --validation entry".to_string());
    }
    if limitations.iter().any(|item| item.trim().is_empty()) {
        return Err("finish limitations must be non-empty".to_string());
    }
    let supplied = read_process_identity(owner_pid)?;
    assert_caller(&supplied, "owner")?;
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state_allow_seals(&cfg.control)?;
    validate_global(root, &cfg)?;
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
    let record_agent = record
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if record_agent != agent {
        return Err(format!(
            "ownership mismatch for slot {slot}: expected {record_agent}, got {agent}"
        ));
    }
    let owner_value = record.get("owner").cloned().unwrap_or(Value::Null);
    if owner_value.is_null() {
        return Err(format!(
            "slot {slot} has no bound owner process; adopt it first"
        ));
    }
    let recorded_owner = Identity {
        pid: owner_value.get("pid").and_then(Value::as_i64).unwrap_or(0),
        start_ticks: owner_value
            .get("start_ticks")
            .and_then(Value::as_i64)
            .unwrap_or(0),
        boot_id: owner_value
            .get("boot_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        host_id: owner_value
            .get("host_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        cgroup_path: owner_value
            .get("cgroup_path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    };
    if supplied != recorded_owner {
        let (state, detail) = process_state(Some(&recorded_owner));
        return Err(format!(
            "owner process generation mismatch for slot {slot}; recorded owner is {state}: {detail}"
        ));
    }
    assert_caller(&supplied, "owner")?;
    refuse_seal_overlap_action(&cfg, &record, "finish")?;
    let journals = parse_journals(root)?;
    let journal_names: Vec<String> = journals
        .get("journals")
        .and_then(Value::as_array)
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if seal_paths(&cfg).is_empty() {
        if let Some(first) = journal_names.first() {
            return Err(format!(
                "interrupted mutation recorded in {}; run 'wrkslots recover' first",
                cfg.control.join(first).display()
            ));
        }
    } else if journal_names
        .iter()
        .any(|name| name.contains(&format!(".{}.", slot.chars().count())) && name.contains(slot))
    {
        return Err(format!(
            "finish target '{slot}' overlaps private validation finish for '{slot}'"
        ));
    }
    refuse_target_storage(root, slot)?;
    if !record
        .get("handoff")
        .cloned()
        .unwrap_or(Value::Null)
        .is_null()
    {
        return Err(format!("slot {slot} already has a recorded handoff"));
    }
    assert_record_paths(&cfg, &record)?;
    assert_slot_contents(&cfg, &record)?;
    let mut final_checkouts: Vec<Value> = Vec::new();
    for checkout in record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        final_checkouts.push(assert_checkout_safe(&cfg, &config, &checkout, &repository)?);
    }
    let continuation = format!(
        "wrkslots remove {slot} --coordinator-pid <current-coordinator-pid> --expected-generation {generation}"
    );
    let mut updated = record.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("checkouts".to_string(), json!(final_checkouts));
        object.insert(
            "handoff".to_string(),
            json!({
                "recorded_at": utc_now(),
                "validation": validation,
                "limitations": limitations,
                "continuation": continuation,
            }),
        );
    }
    ensure_event_log(root, &cfg, &selected)?;
    let previous_revision = active.get("revision").and_then(Value::as_i64).unwrap_or(0);
    // Reload through the same loader immediately before publication:
    // the revision and the target record must not have moved under
    // the locks since they were first loaded.
    let reloaded = active_document(root, &cfg, &selected)?;
    let reloaded_record = reloaded
        .get("slots")
        .and_then(Value::as_array)
        .and_then(|slots| {
            slots
                .iter()
                .find(|item| item.get("slot").and_then(Value::as_str) == Some(slot))
        });
    if reloaded
        .get("revision")
        .and_then(Value::as_i64)
        .unwrap_or(0)
        != previous_revision
        || reloaded_record != Some(&record)
    {
        return Err(format!(
            "active state changed before finish publication for slot '{slot}'; preserve it and retry"
        ));
    }
    let previous_digest = canonical_sha256(&record).map_err(|error| error.to_string())?;
    append_event(
        &cfg,
        &selected,
        "active-state-recorded",
        json!({
            "action": "handoff-recorded",
            "slot": slot,
            "previous_revision": previous_revision,
            "revision": previous_revision + 1,
            "previous_record_sha256": previous_digest,
            "record": updated,
            "evidence": {"validation": validation},
        }),
    )?;
    let mut slots: Vec<Value> = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in slots.iter_mut() {
        if item.get("slot").and_then(Value::as_str) == Some(slot) {
            *item = updated.clone();
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
    let after = active_document(root, &cfg, &selected)?;
    let before_slots = active
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let after_slots = after
        .get("slots")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let unrelated_before: Vec<&Value> = before_slots
        .iter()
        .filter(|item| item.get("slot").and_then(Value::as_str) != Some(slot))
        .collect();
    let unrelated_after: Vec<&Value> = after_slots
        .iter()
        .filter(|item| item.get("slot").and_then(Value::as_str) != Some(slot))
        .collect();
    if unrelated_before != unrelated_after {
        return Err(format!(
            "mutation of slot '{slot}' changed an unrelated registry row"
        ));
    }
    Ok(json!({
        "machine": selected,
        "slot": slot,
        "generation": generation,
        "outcome": "handoff recorded",
        "continuation": continuation,
    }))
}
