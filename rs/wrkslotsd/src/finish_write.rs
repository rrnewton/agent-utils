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
use std::time::{Duration, Instant};

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

// When the GitHub credential helper's pacing bucket is full it tells Git
// to quit; a network Git command then waits, bounded, for a credential
// slot instead of refusing at once (#894).  On expiry the refusal says
// "retry later: credential pacing".
const CREDENTIAL_PACING_WAIT_SECONDS: u64 = 120;
const CREDENTIAL_PACING_POLL_SECONDS: u64 = 5;

fn credential_pacing_refused(output: &str) -> bool {
    let text = output.to_lowercase();
    text.contains("credential pacing") || (text.contains("credential") && text.contains("quit"))
}

fn network_operation(args: &[&str]) -> bool {
    matches!(
        args.first(),
        Some(&"fetch") | Some(&"push") | Some(&"ls-remote")
    )
}

/// Run one Git command, waiting out credential pacing on network commands.
fn git_run_paced(repo: &Path, args: &[&str], dev_null_dir: bool) -> Result<GitOutput> {
    let deadline = Instant::now() + Duration::from_secs(CREDENTIAL_PACING_WAIT_SECONDS);
    loop {
        let output = git_run(repo, args, dev_null_dir)?;
        if output.code == 0
            || !network_operation(args)
            || !credential_pacing_refused(&format!("{}\n{}", output.stderr, output.stdout))
        {
            return Ok(output);
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "retry later: credential pacing: the GitHub credential helper is pacing requests and no credential slot freed within {CREDENTIAL_PACING_WAIT_SECONDS} seconds for git {} in {}",
                args.join(" "),
                repo.display()
            ));
        }
        std::thread::sleep(Duration::from_secs(CREDENTIAL_PACING_POLL_SECONDS).min(deadline - now));
    }
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
    let output = git_run_paced(repo, args, false)?;
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
        if current.is_symlink() {
            return Err(format!("{label} crosses a symlink: {}", current.display()));
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

/// Git pathspecs listing every path the globs can contain, when the
/// glob set is all-literal: the oracle's `_literal_cache_pathspecs`.
/// A wildcard or `**` component, an empty glob, or a component Git
/// would read as pathspec syntax declines, and the caller lists
/// everything, as before (#1284).
pub(crate) fn literal_cache_pathspecs(globs: &[String]) -> Option<Vec<String>> {
    let mut pathspecs: Vec<String> = Vec::new();
    for pattern in globs {
        let parts = glob_parts(pattern);
        if parts.is_empty()
            || parts
                .iter()
                .any(|part| part.chars().any(|ch| matches!(ch, '*' | '?' | '[')))
            || parts
                .iter()
                .any(|part| part == "." || part == ".." || part.starts_with(':'))
        {
            return None;
        }
        let pathspec = parts.join("/");
        if !pathspecs.contains(&pathspec) {
            pathspecs.push(pathspec);
        }
    }
    Some(pathspecs)
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
    let pathspecs = literal_cache_pathspecs(globs);
    let (indexed, committed) = match &pathspecs {
        None => (
            git(path, &["ls-files", "--recurse-submodules", "-z"])?,
            git(path, &["ls-tree", "-r", "--name-only", "-z", "HEAD"])?,
        ),
        Some(pathspecs) => {
            let mut indexed_args: Vec<String> = ["ls-files", "--recurse-submodules", "-z", "--"]
                .iter()
                .map(|item| item.to_string())
                .collect();
            indexed_args.extend(pathspecs.iter().cloned());
            let indexed_refs: Vec<&str> = indexed_args.iter().map(String::as_str).collect();
            let mut committed_args: Vec<String> =
                ["ls-tree", "-r", "--name-only", "-z", "HEAD", "--"]
                    .iter()
                    .map(|item| item.to_string())
                    .collect();
            committed_args.extend(pathspecs.iter().cloned());
            let committed_refs: Vec<&str> = committed_args.iter().map(String::as_str).collect();
            (git(path, &indexed_refs)?, git(path, &committed_refs)?)
        }
    };
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
    let listed = git_run_paced(
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

/// One checkout's (or nested worktree's) commit accounting (#1364).
pub(crate) struct AccountingEntry {
    pub(crate) checkout: String,
    pub(crate) landed: Vec<(String, String)>,
    pub(crate) opened: Vec<(String, String)>,
    pub(crate) unaccounted: Vec<(String, String)>,
    pub(crate) bundle_ref: Option<String>,
}

fn listed_worktrees(repository: &Path) -> Result<Vec<PathBuf>> {
    let listing = git(repository, &["worktree", "list", "--porcelain"])?;
    let mut paths = Vec::new();
    for line in listing.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            paths.push(PathBuf::from(path.trim()));
        }
    }
    Ok(paths)
}

fn is_ancestor(path: &Path, ancestor: &str, descendant: &str) -> Result<bool> {
    let result = git_run(
        path,
        &["merge-base", "--is-ancestor", ancestor, descendant],
        false,
    )?;
    if result.code != 0 && result.code != 1 {
        return Err(format!(
            "cannot compare {ancestor} with {descendant}: {}",
            result.stderr.trim()
        ));
    }
    Ok(result.code == 0)
}

fn patch_equivalents(
    path: &Path,
    upstream: &str,
    head: &str,
) -> Result<std::collections::HashSet<String>> {
    // Git's own patch-id equivalence (`git cherry`), as in the Python edition.
    let output = git(path, &["cherry", upstream, head])?;
    let mut found = std::collections::HashSet::new();
    for line in output.lines() {
        if let Some(sha) = line.strip_prefix("- ") {
            let sha = sha.trim();
            if is_sha(sha) {
                found.insert(sha.to_string());
            }
        }
    }
    Ok(found)
}

fn classify_commits(
    name: &str,
    path: &Path,
    remote: &str,
    landed_ref: &str,
    head: &str,
    base: Option<&str>,
) -> Result<AccountingEntry> {
    let candidates: Vec<String> = match base {
        Some(start) if is_sha(start) && is_ancestor(path, start, head)? => {
            git(path, &["rev-list", &format!("{start}..{head}")])?
                .lines()
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        }
        _ => {
            let merge_base = git_run(path, &["merge-base", head, landed_ref], false)?;
            let base_sha = merge_base.stdout.trim().to_string();
            if merge_base.code == 0 && is_sha(&base_sha) {
                git(path, &["rev-list", &format!("{base_sha}..{head}")])?
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            } else {
                git(path, &["rev-list", head])?
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            }
        }
    };
    let landed_eq = patch_equivalents(path, landed_ref, head)?;
    let published = git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            &format!("refs/remotes/{remote}/"),
        ],
    )?;
    let mut open_eq = std::collections::HashSet::new();
    for reference in published.lines().filter(|line| !line.is_empty()) {
        open_eq.extend(patch_equivalents(path, reference.trim(), head)?);
    }
    let mut entry = AccountingEntry {
        checkout: name.to_string(),
        landed: Vec::new(),
        opened: Vec::new(),
        unaccounted: Vec::new(),
        bundle_ref: None,
    };
    for commit in candidates {
        let subject = git(path, &["show", "-s", "--format=%s", &commit])?
            .trim()
            .to_string();
        let containing = git(
            path,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                &commit,
                &format!("refs/remotes/{remote}/"),
            ],
        )?;
        if is_ancestor(path, &commit, landed_ref)? || landed_eq.contains(&commit) {
            entry.landed.push((commit, subject));
        } else if containing.lines().any(|line| !line.is_empty()) || open_eq.contains(&commit) {
            entry.opened.push((commit, subject));
        } else {
            entry.unaccounted.push((commit, subject));
        }
    }
    Ok(entry)
}

/// Deduplicated (landed, open, unaccounted) totals, as in the Python edition.
pub(crate) fn accounting_totals(
    entries: &[AccountingEntry],
) -> (usize, usize, Vec<(String, String)>) {
    let mut landed = std::collections::HashSet::new();
    let mut opened = std::collections::HashSet::new();
    let mut unaccounted: Vec<(String, String)> = Vec::new();
    for entry in entries {
        for (sha, _) in &entry.landed {
            landed.insert(sha.clone());
        }
        for (sha, _) in &entry.opened {
            opened.insert(sha.clone());
        }
        for pair in &entry.unaccounted {
            if !unaccounted.iter().any(|(sha, _)| sha == &pair.0) {
                unaccounted.push(pair.clone());
            }
        }
    }
    for sha in landed.union(&opened).collect::<Vec<_>>() {
        unaccounted.retain(|(item, _)| item != sha);
    }
    let opened_only = opened.difference(&landed).count();
    unaccounted.sort();
    (landed.len(), opened_only, unaccounted)
}

pub(crate) fn accounting_refusal(slot: &str, entries: &[AccountingEntry]) -> String {
    let (landed, opened, unaccounted) = accounting_totals(entries);
    let mut text = format!(
        "slot {slot} has unaccounted commits: accounting landed={landed} open={opened} unaccounted={}",
        unaccounted.len()
    );
    for (sha, subject) in &unaccounted {
        text.push_str(&format!("\nunaccounted {sha} {subject}"));
    }
    text.push_str("\npublish or land every commit, or rerun finish with --discard-unaccounted to record and discard them");
    text
}

fn status_outside_paths(status: &str, prefixes: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in status.lines() {
        let path: Option<String> =
            if let Some(rest) = line.strip_prefix("? ").or_else(|| line.strip_prefix("! ")) {
                Some(rest.to_string())
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                line.splitn(9, ' ')
                    .nth(8)
                    .map(|value| value.split('\t').next().unwrap_or_default().to_string())
            } else if line.starts_with("u ") {
                line.splitn(11, ' ')
                    .nth(10)
                    .map(|value| value.split('\t').next().unwrap_or_default().to_string())
            } else {
                None
            };
        let inside = path
            .as_deref()
            .map(|candidate| {
                prefixes.iter().any(|prefix| {
                    candidate == prefix
                        || candidate.starts_with(&format!("{}/", prefix.trim_end_matches('/')))
                })
            })
            .unwrap_or(false);
        if !inside {
            kept.push(line);
        }
    }
    if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    }
}

const PROVISIONED_EXACT_BYTES: u64 = 16 * 1024 * 1024;
const PROVISIONED_EXACT_FILES: u64 = 512;

fn sha256_regular(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    digest.update(&bytes);
    Ok(format!("{:x}", digest.finalize()))
}

fn mtime_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i64)
        .unwrap_or(0)
}

/// Describe one provisioned path exactly as the record stores it (#1369).
pub(crate) fn describe_provisioned(base: &Path, relative: &str) -> Result<Value> {
    let target = base.join(relative);
    let metadata = std::fs::symlink_metadata(&target)
        .map_err(|error| format!("cannot inspect {}: {error}", target.display()))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let dest = std::fs::read_link(&target)
            .map_err(|error| format!("cannot read link {}: {error}", target.display()))?;
        return Ok(json!({"path": relative, "kind": "symlink", "target": dest.to_string_lossy()}));
    }
    if file_type.is_dir() {
        let mut lines: Vec<String> = Vec::new();
        let mut file_count: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut max_mtime: i64 = 0;
        // Match the Python walk: each directory's sorted entries in
        // walk (breadth) order, directories queued in sorted order.
        let mut queue: std::collections::VecDeque<PathBuf> =
            std::collections::VecDeque::from([target.clone()]);
        while let Some(directory) = queue.pop_front() {
            let mut names: Vec<PathBuf> = Vec::new();
            for entry in std::fs::read_dir(&directory)
                .map_err(|error| format!("cannot list {}: {error}", directory.display()))?
            {
                names.push(entry.map_err(|error| error.to_string())?.path());
            }
            names.sort();
            for child in names {
                let child_meta = std::fs::symlink_metadata(&child)
                    .map_err(|error| format!("cannot inspect {}: {error}", child.display()))?;
                max_mtime = max_mtime.max(mtime_ns(&child_meta));
                let rel = child
                    .strip_prefix(base)
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default();
                let kind = child_meta.file_type();
                if kind.is_symlink() {
                    let dest = std::fs::read_link(&child).map_err(|error| {
                        format!("cannot read link {}: {error}", child.display())
                    })?;
                    lines.push(format!("{rel}\nsymlink\n{}", dest.to_string_lossy()));
                } else if kind.is_dir() {
                    lines.push(format!("{rel}\ndirectory"));
                    queue.push_back(child);
                } else if kind.is_file() {
                    file_count += 1;
                    total_bytes += child_meta.len();
                    lines.push(format!("{rel}\nfile\n{}", sha256_regular(&child)?));
                } else {
                    lines.push(format!("{rel}\nother"));
                }
            }
        }
        let mut digest = Sha256::new();
        digest.update(lines.join("\n").as_bytes());
        if file_count <= PROVISIONED_EXACT_FILES && total_bytes <= PROVISIONED_EXACT_BYTES {
            return Ok(
                json!({"path": relative, "kind": "directory", "sha256": format!("{:x}", digest.finalize())}),
            );
        }
        return Ok(json!({
            "path": relative,
            "kind": "directory",
            "file_count": file_count,
            "total_bytes": total_bytes,
            "max_mtime_ns": max_mtime,
        }));
    }
    if file_type.is_file() {
        return Ok(json!({"path": relative, "kind": "file", "sha256": sha256_regular(&target)?}));
    }
    Ok(json!({"path": relative, "kind": "file", "sha256": "other"}))
}

fn gitlink_paths(path: &Path) -> Result<Vec<String>> {
    let mut found: Vec<String> = Vec::new();
    for args in [
        vec!["ls-files", "--stage", "-z"],
        vec!["ls-tree", "-r", "-z", "HEAD"],
    ] {
        let output = git(path, &args)?;
        for record in output.split('\0') {
            if record.is_empty() {
                continue;
            }
            if let Some((metadata, sub_path)) = record.split_once('\t') {
                if metadata.starts_with("160000 ") && !found.iter().any(|item| item == sub_path) {
                    found.push(sub_path.to_string());
                }
            }
        }
    }
    Ok(found)
}

pub(crate) fn initialized_submodules(path: &Path) -> Result<Vec<String>> {
    let mut result: Vec<String> = Vec::new();
    let mut queue: Vec<String> = gitlink_paths(path)?;
    let mut index = 0;
    while index < queue.len() {
        let relative = queue[index].clone();
        index += 1;
        let nested = path.join(&relative);
        if !nested.join(".git").exists() {
            continue;
        }
        result.push(relative.clone());
        for child in gitlink_paths(&nested)? {
            queue.push(format!("{relative}/{child}"));
        }
    }
    Ok(result)
}

/// Record every untracked/ignored path provisioning made (#1369).
pub(crate) fn snapshot_provisioned(path: &Path) -> Result<Vec<Value>> {
    let mut roots: Vec<(String, PathBuf)> = vec![(String::new(), path.to_path_buf())];
    for submodule in initialized_submodules(path)? {
        roots.push((submodule.clone(), path.join(submodule)));
    }
    let mut entries: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (prefix, root) in roots {
        let status = git(
            &root,
            &[
                "status",
                "--porcelain=v2",
                "--untracked-files=all",
                "--ignored=matching",
                "--",
                ".",
            ],
        )?;
        for line in status.lines() {
            if !line.starts_with("? ") && !line.starts_with("! ") {
                continue;
            }
            let Some(found) = status_line_path(line) else {
                continue;
            };
            let relative = if prefix.is_empty() {
                found.trim_end_matches('/').to_string()
            } else {
                format!("{prefix}/{}", found.trim_end_matches('/'))
            };
            if seen.contains(&relative) {
                continue;
            }
            if let Ok(entry) = describe_provisioned(path, &relative) {
                seen.push(relative);
                entries.push(entry);
            }
        }
    }
    entries.sort_by(|left, right| {
        left.get("path")
            .and_then(Value::as_str)
            .cmp(&right.get("path").and_then(Value::as_str))
    });
    Ok(entries)
}

pub(crate) fn provisioned_unchanged(path: &Path, entry: &Value) -> bool {
    let Some(relative) = entry.get("path").and_then(Value::as_str) else {
        return false;
    };
    describe_provisioned(path, relative)
        .map(|current| current == *entry)
        .unwrap_or(false)
}

fn status_line_path(line: &str) -> Option<String> {
    if let Some(rest) = line.strip_prefix("? ").or_else(|| line.strip_prefix("! ")) {
        return Some(rest.to_string());
    }
    if line.starts_with("1 ") || line.starts_with("2 ") {
        return line
            .splitn(9, ' ')
            .nth(8)
            .map(|value| value.split('\t').next().unwrap_or_default().to_string());
    }
    if line.starts_with("u ") {
        return line
            .splitn(11, ' ')
            .nth(10)
            .map(|value| value.split('\t').next().unwrap_or_default().to_string());
    }
    None
}

/// Drop status lines covered by tolerated provisioned paths (#1369).
pub(crate) fn status_outside_provisioned(
    status: &str,
    tolerated: &[String],
    allowlist: &[String],
) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for line in status.lines() {
        let Some(found) = status_line_path(line) else {
            kept.push(line);
            continue;
        };
        let clean = found.trim_end_matches('/');
        let mut covered = tolerated.iter().any(|item| {
            clean == item
                || clean.starts_with(&format!("{}/", item.trim_end_matches('/')))
                || item.starts_with(&format!("{clean}/"))
        });
        if !covered {
            let parts: Vec<String> = clean
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_string)
                .collect();
            'patterns: for pattern in allowlist {
                let pattern_parts: Vec<String> = pattern
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect();
                for end in 1..=parts.len() {
                    if glob_parts_match(&pattern_parts, &parts[..end]) {
                        covered = true;
                        break 'patterns;
                    }
                }
            }
        }
        if !covered {
            kept.push(line);
        }
    }
    if kept.is_empty() {
        String::new()
    } else {
        format!("{}\n", kept.join("\n"))
    }
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
    assert_checkout_safe_accounting(cfg, config, checkout, repository, None)
}

pub(crate) fn assert_checkout_safe_accounting(
    cfg: &Cfg,
    config: &Value,
    checkout: &Value,
    repository: &Path,
    accounting: Option<&mut Vec<AccountingEntry>>,
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
    // One `-v` listing answers both questions: a lowercase tag is the
    // assume-unchanged form, and `S` is skip-worktree, exactly what the
    // separate `-t` listing reported. Assume-unchanged is reported
    // first, as when the two listings were separate (#1284).
    let listed = git(&path, &["ls-files", "-v"])?;
    for line in listed.lines() {
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
    for line in listed.lines() {
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
    let mut status = git(&path, &status_refs)?;
    if accounting.is_some() && !status.is_empty() {
        // A nested linked worktree is a checkout boundary, not dirt (#1364).
        let mut prefixes: Vec<String> = Vec::new();
        for listed in listed_worktrees(repository)? {
            if listed != path {
                if let Ok(relative) = listed.strip_prefix(&path) {
                    prefixes.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        if !prefixes.is_empty() {
            status = status_outside_paths(&status, &prefixes);
        }
    }
    let provisioned: Vec<Value> = checkout
        .get("provisioned")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let allowlist: Vec<String> = config
        .get("provisioned_allowlist")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !status.is_empty() && (!provisioned.is_empty() || !allowlist.is_empty()) {
        let tolerated: Vec<String> = provisioned
            .iter()
            .filter(|entry| provisioned_unchanged(&path, entry))
            .filter_map(|entry| {
                entry
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        status = status_outside_provisioned(&status, &tolerated, &allowlist);
    }
    for entry in &provisioned {
        if !provisioned_unchanged(&path, entry) {
            return Err(format!(
                "checkout {name} provisioned path changed: {}; restore the provisioning output or remove it before finishing",
                entry.get("path").and_then(Value::as_str).unwrap_or_default()
            ));
        }
    }
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
    if let Some(sink) = accounting {
        // Per-commit accounting (#1364) replaces the HEAD-only gates.
        let start_point = checkout
            .get("start_point")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        sink.push(classify_commits(
            &name,
            &path,
            &remote,
            &landed_ref,
            &head,
            Some(&start_point),
        )?);
        let mut updated = checkout.clone();
        if let Some(object) = updated.as_object_mut() {
            object.insert("head".to_string(), json!(head));
            object.insert("containing_remote_refs".to_string(), json!(remote_refs));
        }
        return Ok(updated);
    }
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
    discard_unaccounted: bool,
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
            return Err(crate::journals_state::journal_blocker_message(
                &cfg.control,
                first,
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
    let slot_path = assert_slot_contents(&cfg, &record)?;
    let mut entries: Vec<AccountingEntry> = Vec::new();
    let mut final_checkouts: Vec<Value> = Vec::new();
    let checkouts_now = record
        .get("checkouts")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let recorded_paths: Vec<PathBuf> = checkouts_now
        .iter()
        .map(|checkout| {
            stored_path(
                &cfg,
                checkout
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "checkout path",
            )
        })
        .collect::<Result<Vec<_>>>()?;
    for checkout in &checkouts_now {
        let repository_raw = checkout
            .get("repository")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (_stored, repository) = stored_repository_path(&cfg, repository_raw)?;
        final_checkouts.push(assert_checkout_safe_accounting(
            &cfg,
            &config,
            checkout,
            &repository,
            Some(&mut entries),
        )?);
        // Nested linked worktrees inside the slot (#1364).
        let remote = checkout
            .get("remote")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let landed_ref = checkout
            .get("landed_ref")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let checkout_name = checkout
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        for listed in listed_worktrees(&repository)? {
            if recorded_paths.contains(&listed) {
                continue;
            }
            if !listed.starts_with(&slot_path) {
                continue;
            }
            let head = git(&listed, &["rev-parse", "--verify", "HEAD^{commit}"])?
                .trim()
                .to_string();
            let branch = checkout_branch(&listed).unwrap_or_else(|_| "HEAD".to_string());
            let mut entry = classify_commits(
                &format!(
                    "{}+nested:{}",
                    checkout_name,
                    listed.file_name().unwrap_or_default().to_string_lossy()
                ),
                &listed,
                &remote,
                &landed_ref,
                &head,
                None,
            )?;
            entry.bundle_ref = Some(if branch == "HEAD" {
                "HEAD".to_string()
            } else {
                format!("refs/heads/{branch}")
            });
            entries.push(entry);
        }
    }
    let (landed_count, open_count, unaccounted) = accounting_totals(&entries);
    if !unaccounted.is_empty() && !discard_unaccounted {
        return Err(accounting_refusal(slot, &entries));
    }
    let mut discarded: Vec<String> = Vec::new();
    let mut bundle_paths: Vec<String> = Vec::new();
    if !unaccounted.is_empty() && discard_unaccounted {
        discarded = unaccounted.iter().map(|(sha, _)| sha.clone()).collect();
        let archive_dir = cfg.control.join("wrkslots-salvage");
        std::fs::create_dir_all(&archive_dir)
            .map_err(|error| format!("cannot create salvage archive: {error}"))?;
        for (index, checkout) in checkouts_now.iter().enumerate() {
            let name = checkout
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let related: Vec<&AccountingEntry> = entries
                .iter()
                .filter(|entry| {
                    entry.checkout == name || entry.checkout.starts_with(&format!("{name}+nested:"))
                })
                .collect();
            let bad = related
                .iter()
                .flat_map(|entry| entry.unaccounted.iter())
                .any(|(sha, _)| discarded.contains(sha));
            if !bad {
                continue;
            }
            let path = &recorded_paths[index];
            let landed_ref = checkout
                .get("landed_ref")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let bundle = archive_dir.join(format!("finish-{slot}-{name}-{generation}.bundle"));
            if bundle.exists() {
                std::fs::remove_file(&bundle)
                    .map_err(|error| format!("cannot replace discard bundle: {error}"))?;
            }
            let mut args: Vec<String> = vec![
                "bundle".to_string(),
                "create".to_string(),
                bundle.to_string_lossy().into_owned(),
                "HEAD".to_string(),
            ];
            for entry in &related {
                if let Some(reference) = &entry.bundle_ref {
                    args.push(reference.clone());
                }
            }
            args.push(format!("^{landed_ref}"));
            let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
            git(path, &arg_refs)?;
            git(path, &["bundle", "verify", &bundle.to_string_lossy()])?;
            bundle_paths.push(bundle.to_string_lossy().into_owned());
        }
    }
    let continuation = format!(
        "wrkslots remove {slot} --coordinator-pid <current-coordinator-pid> --expected-generation {generation}"
    );
    let mut updated = record.clone();
    if let Some(object) = updated.as_object_mut() {
        object.insert("checkouts".to_string(), json!(final_checkouts));
        let mut handoff = json!({
            "recorded_at": utc_now(),
            "validation": validation,
            "limitations": limitations,
            "continuation": continuation,
        });
        if !discarded.is_empty() {
            handoff["discarded_unaccounted"] = json!(discarded);
        }
        object.insert("handoff".to_string(), handoff);
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
            "evidence": {
                "validation": validation,
                "accounting": {
                    "landed": landed_count,
                    "open": open_count,
                    "unaccounted": unaccounted.len(),
                    "discarded_unaccounted": discarded,
                    "discard_bundles": bundle_paths,
                },
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
        "accounting": {
            "landed": landed_count,
            "open": open_count,
            "unaccounted": unaccounted.len(),
            "discarded_unaccounted": discarded,
            "discard_bundles": bundle_paths,
        },
    }))
}
