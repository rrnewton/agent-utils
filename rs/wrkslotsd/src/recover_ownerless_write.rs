//! Writer: recover one ownerless agent worktree
//! (`recover-ownerless-agent-worktree`), plan outcomes.
//!
//! Reproduces the read-only outcomes of the Python command: the
//! local-machine check, interrupted-journal resume planning,
//! registry collision refusal, target/repository binding, and the
//! worktree facts proof (directory and Git identity, HEAD/branch,
//! HANDOFF digest binding, ordinary history/index, remote URL
//! digest, detached-HEAD containment). The destructive apply path
//! (salvage, fencing, registration removal) stays with the Python
//! edition and refuses explicitly. Fixture registries only until
//! the edition cutover (see `ensure_states::require_fixture`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::active_state::{load_cfg, slot_directory};
use crate::config_validate::validate_name;
use crate::ensure_states::{refuse_partial_state, require_fixture};
use crate::finish_write::remote_authority;
use crate::hold_write::{acquire_mutation_locks, active_document, validate_global};
use crate::status_state::read_process_identity;

type Result<T> = std::result::Result<T, String>;

fn short_hostname() -> Result<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map_err(|error| format!("cannot read the machine hostname: {error}"))?;
    let name = raw.trim().split('.').next().unwrap_or_default().to_string();
    validate_name(&name, "machine")?;
    Ok(name)
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn repository_path(cfg: &crate::active_state::Cfg, raw: &str) -> Result<PathBuf> {
    // Mirrors _repository_path(..., allow_managed=True): relative
    // spellings inside the project root or one direct sibling
    // (../NAME[/PATH]); managed-worktree repositories are allowed.
    if Path::new(raw).is_absolute() {
        return Err(format!(
            "repository path must be relative to {}: {raw:?}; use a path inside the project root or ../NAME[/PATH] components inside one direct sibling",
            cfg.root.display()
        ));
    }
    let parts: Vec<String> = Path::new(raw)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    let sibling = parts.len() >= 2
        && parts[0] == ".."
        && parts[1..].iter().all(|part| part != "." && part != "..");
    if parts.iter().any(|part| part == "..") && !sibling {
        return Err(format!(
            "repository path must stay inside {} or one direct sibling, with path components '../NAME' or '../NAME/PATH'; other parent traversal is refused: {raw:?}",
            cfg.root.display()
        ));
    }
    let unresolved = if sibling {
        let mut base = cfg.root.clone();
        base.pop();
        parts[1..].iter().fold(base, |mut path, part| {
            path.push(part);
            path
        })
    } else {
        parts.iter().fold(cfg.root.clone(), |mut path, part| {
            if part != "." {
                path.push(part);
            }
            path
        })
    };
    // No symlink components below the allowed root.
    let allowed = if sibling {
        cfg.root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        cfg.root.clone()
    };
    {
        let relative = unresolved
            .strip_prefix(&allowed)
            .map_err(|_| "repository path escapes its allowed root".to_string())?;
        let mut current = allowed.clone();
        for part in relative.components() {
            current.push(part.as_os_str());
            if current.is_symlink() {
                return Err(format!(
                    "repository crosses a symlink: {}",
                    current.display()
                ));
            }
        }
    }
    let resolved = unresolved
        .canonicalize()
        .map_err(|_| format!("source repository {raw:?} does not exist or cannot be resolved"))?;
    if !resolved.is_dir() {
        return Err(format!(
            "repository does not exist or is unsafe: {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn is_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

/// Plan (or explicitly refuse) ownerless agent worktree recovery.
#[allow(clippy::too_many_arguments)]
pub(crate) fn recover_ownerless_agent_worktree(
    root: Option<&Path>,
    machine_override: Option<&str>,
    raw_path: &str,
    repository_arg: &str,
    head: &str,
    branch: Option<&str>,
    detached: bool,
    remote: &str,
    remote_url_sha256: &str,
    handoff_sha256: Option<&str>,
    apply: bool,
    coordinator_authorized: bool,
    coordinator_pid: Option<i64>,
) -> Result<Value> {
    require_fixture()?;
    let (cfg, selected) = load_cfg(root, machine_override)?;
    validate_name(&selected, "machine")?;
    if selected != short_hostname()? {
        return Err("ownerless agent recovery must run on the worktree's machine".to_string());
    }
    if apply {
        if !coordinator_authorized {
            return Err(
                "ownerless agent worktree recovery requires --coordinator-authorized with --apply"
                    .to_string(),
            );
        }
        let pid = coordinator_pid.ok_or("--apply requires --coordinator-pid")?;
        read_process_identity(pid)?;
    }
    // Resolve the target: one direct child of the agent root.
    // Mirrors _relative_inside: relative spelling only, no parent
    // components, normalized, and no symlink components.
    let agent_root = slot_directory(&cfg, "slot", "agent")?
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if Path::new(raw_path).is_absolute()
        || raw_path.is_empty()
        || Path::new(raw_path)
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(format!(
            "ownerless agent worktree must be a relative path inside {}: {raw_path:?}",
            cfg.root.display()
        ));
    }
    let mut lexical = PathBuf::new();
    for part in Path::new(raw_path).components() {
        if let std::path::Component::Normal(name) = part {
            lexical.push(name);
        }
    }
    if lexical.as_os_str().is_empty() {
        return Err("ownerless agent worktree must not be the project root".to_string());
    }
    let target = cfg.root.join(&lexical);
    {
        let mut current = cfg.root.clone();
        for part in lexical.components() {
            current.push(part.as_os_str());
            if current.is_symlink() {
                return Err(format!(
                    "ownerless agent worktree crosses a symlink: {}",
                    current.display()
                ));
            }
        }
    }
    let name = target
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    if target.parent() != Some(agent_root.as_path()) || name == "ignored" {
        return Err(format!(
            "ownerless agent worktree must be one direct child of {}, excluding ignored",
            agent_root.display()
        ));
    }
    validate_name(&name, "slot")?;
    let relative = lexical.to_string_lossy().into_owned();
    let _locks = acquire_mutation_locks(root, machine_override, &cfg, &selected)?;
    refuse_partial_state(&cfg.control)?;
    // Interrupted-journal outcomes.
    {
        let enumerated = crate::journals_state::parse_journals(root)?;
        let names = enumerated
            .get("journals")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(first) = names.first().and_then(Value::as_str) {
            let path = cfg.control.join(first);
            if names.len() == 1 && first == format!("ACTIVE.{selected}.journal") {
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Err(format!(
                            "another interrupted mutation is recorded in {}",
                            path.display()
                        ));
                    }
                    Err(error) => {
                        return Err(format!("cannot read recovery journal: {error}"));
                    }
                };
                let raw: Value = serde_json::from_str(&text)
                    .map_err(|error| format!("cannot read recovery journal: {error}"))?;
                if raw.get("kind").and_then(Value::as_str) != Some("ownerless-agent-remove") {
                    return Err(format!(
                        "another interrupted mutation is recorded in {}",
                        path.display()
                    ));
                }
                let authorization = raw.get("authorization").cloned().unwrap_or(Value::Null);
                let journal_path = authorization
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if journal_path != relative {
                    return Err(
                        "requested ownerless path differs from the interrupted recovery"
                            .to_string(),
                    );
                }
                let auth_keys: Vec<&String> = authorization
                    .as_object()
                    .map(|object| object.keys().collect())
                    .unwrap_or_default();
                let identity_ok = authorization
                    .get("identity")
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.len() == 3 && items.iter().all(Value::is_i64));
                let fenced = raw
                    .get("fenced")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let fenced_ok = Path::new(fenced).parent() == Path::new(&relative).parent()
                    && Path::new(fenced)
                        .file_name()
                        .map(|value| value.to_string_lossy().into_owned())
                        .is_some_and(|value| {
                            value.starts_with(&format!(".{name}.ownerless-agent."))
                        });
                let journal_valid = raw.get("schema").and_then(Value::as_i64) == Some(2)
                    && raw.get("machine").and_then(Value::as_str) == Some(selected.as_str())
                    && raw.get("slot").and_then(Value::as_str) == Some(name.as_str())
                    && matches!(
                        raw.get("phase").and_then(Value::as_str),
                        Some("prepared" | "preserved" | "fenced" | "removed")
                    )
                    && auth_keys.len() == 10
                    && identity_ok
                    && fenced_ok;
                if !journal_valid {
                    return Err("ownerless-agent journal is invalid".to_string());
                }
                if !apply {
                    return Ok(json!({
                        "machine": selected,
                        "path": target.to_string_lossy(),
                        "outcome": "planned",
                        "detail": "resume",
                    }));
                }
                return Err(
                    "resuming an interrupted ownerless-agent recovery stays with the Python edition for now"
                        .to_string(),
                );
            }
            return Err(format!(
                "another interrupted mutation is recorded in {}",
                path.display()
            ));
        }
    }
    validate_global(root, &cfg)?;
    // Registry collision: no active or archived row may claim it.
    let active = active_document(root, &cfg, &selected)?;
    if let Some(slots) = active.get("slots").and_then(Value::as_array) {
        for record in slots {
            let slot = record
                .get("slot")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if slot == name {
                return Err(format!(
                    "ownerless path name collides with active slot {slot}"
                ));
            }
            for checkout in record
                .get("checkouts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
            {
                let recorded = cfg.root.join(
                    checkout
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                );
                if recorded == target
                    || recorded.starts_with(&target)
                    || target.starts_with(&recorded)
                {
                    return Err(format!(
                        "ownerless path overlaps registered checkout {slot}/{}",
                        checkout
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                    ));
                }
            }
        }
    }
    let archives = crate::recover_absent_write::archives_document(&cfg, &selected)?;
    if let Some(records) = archives.get("records").and_then(Value::as_array) {
        for record in records {
            if record.get("slot").and_then(Value::as_str) == Some(name.as_str()) {
                return Err(format!(
                    "ownerless path name collides with archived slot {name}"
                ));
            }
        }
    }
    let repository = repository_path(&cfg, repository_arg)?;
    if repository == target || repository.starts_with(&target) {
        return Err("ownerless agent source repository is inside the removal target".to_string());
    }
    if !is_sha(head) || !is_digest(remote_url_sha256) {
        return Err("--head and --remote-url-sha256 must be full lowercase digests".to_string());
    }
    if let Some(digest) = handoff_sha256 {
        if !is_digest(digest) {
            return Err("--handoff-sha256 must be one lowercase SHA-256 digest".to_string());
        }
    }
    if detached == branch.is_some() {
        return Err("exactly one of --branch or --detached is required".to_string());
    }
    // Worktree facts.
    if target.is_symlink() || !target.is_dir() {
        return Err(format!(
            "ownerless agent worktree is absent or unsafe: {}",
            target.display()
        ));
    }
    let handoff_path = target.join("HANDOFF.md");
    if handoff_path.is_symlink() {
        return Err(format!(
            "ownerless agent HANDOFF.md must not be a symlink: {}",
            handoff_path.display()
        ));
    }
    let observed_handoff = if handoff_path.exists() {
        let contents = std::fs::read(&handoff_path)
            .map_err(|error| format!("cannot read ownerless agent HANDOFF.md: {error}"))?;
        if contents.len() > 1024 * 1024 {
            return Err("ownerless agent HANDOFF.md is too large".to_string());
        }
        Some(format!("{:x}", Sha256::digest(&contents)))
    } else {
        None
    };
    match (&observed_handoff, handoff_sha256) {
        (Some(observed), None) => {
            return Err(format!(
                "ownerless agent worktree contains unread HANDOFF.md; read it and rerun with --handoff-sha256 {observed}"
            ));
        }
        (observed, Some(expected)) if observed.as_deref() != Some(expected) => {
            return Err(
                "ownerless agent HANDOFF.md changed after it was read; preserve the worktree"
                    .to_string(),
            );
        }
        _ => {}
    }
    let top = git(&target, &["rev-parse", "--show-toplevel"])?;
    if Path::new(top.trim()) != target {
        return Err(format!(
            "ownerless agent worktree is not its Git worktree root: {}",
            target.display()
        ));
    }
    let listed = git(&repository, &["worktree", "list", "--porcelain"])?;
    if !listed
        .lines()
        .any(|line| line == format!("worktree {}", target.display()))
    {
        return Err(format!(
            "ownerless agent worktree is not registered by its source repository: {}",
            target.display()
        ));
    }
    let observed_head = git(&target, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let observed_head = observed_head.trim();
    if detached {
        if git(&target, &["symbolic-ref", "--quiet", "--short", "HEAD"]).is_ok() {
            return Err("ownerless agent worktree is not detached".to_string());
        }
        if observed_head != head {
            return Err("ownerless agent worktree HEAD changed".to_string());
        }
    } else {
        let observed_branch = git(&target, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if observed_head != head || observed_branch.trim() != branch.unwrap_or_default() {
            return Err("ownerless agent worktree HEAD or branch changed".to_string());
        }
    }
    for state_name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        let out = git(&target, &["rev-parse", "--git-path", state_name])?;
        let candidate = {
            let raw = out.trim();
            if Path::new(raw).is_absolute() {
                PathBuf::from(raw)
            } else {
                target.join(raw)
            }
        };
        if candidate.exists() || candidate.is_symlink() {
            return Err(format!(
                "ownerless agent worktree has an unfinished Git operation at {}",
                candidate.display()
            ));
        }
    }
    // Ordinary history and index, as in Python's facts proof.
    let common_out = git(&target, &["rev-parse", "--git-common-dir"])?;
    let common = {
        let raw = common_out.trim();
        if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            target.join(raw)
        }
    };
    for marker in ["info/grafts", "shallow"] {
        let candidate = common.join(marker);
        if candidate.exists() || candidate.is_symlink() {
            return Err(format!(
                "ownerless agent worktree uses graft or shallow history: {}",
                candidate.display()
            ));
        }
    }
    let replacements = git(
        &target,
        &["for-each-ref", "--format=%(refname)", "refs/replace/"],
    )?;
    if !replacements.trim().is_empty() {
        return Err("ownerless agent worktree contains replacement refs".to_string());
    }
    let assume = git(&target, &["ls-files", "-v"])?;
    if assume.lines().any(|line| {
        line.chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_lowercase())
    }) {
        return Err("ownerless agent worktree has assume-unchanged index state".to_string());
    }
    let skip = git(&target, &["ls-files", "-t"])?;
    if skip.lines().any(|line| line.starts_with("S ")) {
        return Err("ownerless agent worktree has skip-worktree index state".to_string());
    }
    let (authority_sha, _url) = remote_authority(&target, remote)?;
    if authority_sha != remote_url_sha256 {
        return Err("ownerless agent worktree remote changed".to_string());
    }
    if detached {
        git(
            &target,
            &[
                "fetch",
                "--quiet",
                remote,
                &format!("+refs/heads/*:refs/remotes/{remote}/*"),
            ],
        )?;
        let (after_sha, _after_url) = remote_authority(&target, remote)?;
        if after_sha != authority_sha {
            return Err("ownerless agent worktree remote changed".to_string());
        }
        let containing = git(
            &target,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "--contains",
                head,
                &format!("refs/remotes/{remote}/"),
            ],
        )?;
        if containing.trim().is_empty() {
            return Err(format!(
                "ownerless agent worktree has a detached HEAD {head} that no branch of remote {remote} contains; push it or create a branch for it first"
            ));
        }
    }
    if apply {
        return Err(
            "ownerless agent worktree recovery stays with the Python edition for now".to_string(),
        );
    }
    Ok(json!({
        "machine": selected,
        "path": target.to_string_lossy(),
        "outcome": "planned",
        "detail": if detached { "ownerless-agent-detached" } else { "ownerless-agent" },
    }))
}
