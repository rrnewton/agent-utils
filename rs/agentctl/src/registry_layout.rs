//! Moving a registry from layout 1 to layout 2 (#593 registry-agents-subdir).
//!
//! The layouts themselves are described in [`agent_queue::registry`]. This module moves agents
//! between them: `agentctl migrate-layout` on request, and [`auto_migrate`] before any command that
//! writes.
//!
//! **One agent at a time, atomically.** Each agent moves by one no-replace `renameat2` of its
//! directory from `REGISTRY/<name>` to `REGISTRY/agents/<name>`, while this process holds the
//! agent's layout-1 lifecycle lock (the one older builds take), its layout-2 lock, and the
//! registry's identity lock, so no build that honours either lock sees it half-moved. A crash
//! leaves each agent wholly in one layout, and a rerun continues.
//!
//! **Agents in use stay.** A directory that another process has open, or works in (a headless
//! wrapper serving its socket, a drain holding its queue lock, an older build's long-running
//! process), is left in layout 1 and reported: moving it would leave that process writing to
//! paths that no longer exist. A later run moves it once it is idle.
//!
//! **Lock files.** After an agent moves, its layout-1 lock files are deleted while they are held,
//! and so are layout-1 lock files no agent uses any more. A lock file someone holds is kept. The
//! registry-wide `.coordinator.lock` is always kept: layout 1 also used it for an agent named
//! `coordinator`, but it is the designation lock.
//!
//! **Older builds** cannot see a moved agent, and the tripwire `agents/agent.json` makes them
//! refuse to start, adopt, or deliver in the registry (see [`agent_queue::registry`]): they fail
//! closed, and never write into `agents/`. Upgrade every agentctl that uses a registry (the chat
//! bridge, the agent watcher, restore units, the wrkslots liveness probe) before migrating it.

use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use agent_queue::registry::{self, Location};
use fs2::FileExt;
use serde_json::{json, Value};

use crate::agent::{self, AgentError};

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// The environment variable that turns [`auto_migrate`] off (`0`, `false`, `no` or `off`).
pub const AUTO_MIGRATE_VARIABLE: &str = "AGENTCTL_LAYOUT_AUTO_MIGRATE";

/// How [`migrate`] runs.
#[derive(Clone, Copy, Debug)]
pub struct MigrateOptions {
    /// Move agents and delete lock files; without it, only report what would happen.
    pub apply: bool,
    /// How long to wait for a busy lock before leaving that agent where it is.
    pub wait: Duration,
}

/// After taking `location`'s lifecycle lock `file` at `path` for `agent`: is that still the lock
/// that guards the agent? Not when a migration moved the agent meanwhile, or deleted the lock
/// file this process was waiting on; the caller then takes the lock again.
///
/// # Errors
///
/// The agent cannot be located, or the lock cannot be inspected.
pub(crate) fn lock_still_current(
    registry: &Path,
    agent: &str,
    location: Location,
    file: &File,
    path: &Path,
) -> Result<bool> {
    if registry::locate(registry, agent)? != location {
        return Ok(false);
    }
    let held = file
        .metadata()
        .map_err(|error| fail(format!("cannot inspect agent lifecycle lock: {error}")))?;
    Ok(match fs::symlink_metadata(path) {
        Ok(named) => named.dev() == held.dev() && named.ino() == held.ino(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(fail(format!(
                "cannot inspect agent lifecycle lock {}: {error}",
                path.display()
            )))
        }
    })
}

/// Lock `file` exclusively, trying until `wait` has passed; `false` when it stayed busy.
fn lock_within(file: &File, wait: Duration) -> bool {
    let deadline = Instant::now() + wait;
    loop {
        if FileExt::try_lock_exclusive(file).is_ok() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// A process other than this one whose working directory is inside `directory`, as
/// `{pid, command}`. Only working directories are read: listing every process's open files costs
/// seconds on a busy host, and the lock files inside the directory ([`held_lock_inside`]) cover
/// the programs that keep it open. Another user's processes cannot use a private registry.
fn working_inside(directory: &Path) -> Option<Value> {
    let canonical = fs::canonicalize(directory).ok()?;
    let own = std::process::id();
    for process in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|pid| pid.parse::<u64>().ok())
        else {
            continue;
        };
        if pid == u64::from(own) {
            continue;
        }
        if fs::read_link(process.path().join("cwd")).is_ok_and(|cwd| cwd.starts_with(&canonical)) {
            return Some(json!({
                "pid": pid,
                "command": crate::lock_wait::command_of(pid).unwrap_or_default(),
            }));
        }
    }
    None
}

/// A process holding a lock file inside agent directory `directory` (its top level and one
/// level down): a headless wrapper's `headless.lock`, a drain's `queue/.delivery.lock`.
fn held_lock_inside(directory: &Path) -> Option<Value> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(directory).ok()?.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {
                if let Ok(inner) = fs::read_dir(&path) {
                    candidates.extend(inner.flatten().map(|entry| entry.path()));
                }
            }
            Ok(_) => candidates.push(path),
            Err(_) => {}
        }
    }
    candidates
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "lock")
        })
        .find_map(|path| {
            let pid = crate::lock_wait::holder_of(&path)?;
            Some(json!({
                "pid": pid,
                "command": crate::lock_wait::command_of(pid).unwrap_or_default(),
            }))
        })
}

/// Why agent directory `directory` must stay where it is, when its record names a path inside it
/// (a turn runner's runtime home, say): moving the directory would leave that path dangling.
fn record_names_its_directory(directory: &Path) -> Option<String> {
    let text = fs::read_to_string(directory.join("agent.json")).ok()?;
    let mut spellings = vec![directory.display().to_string()];
    if let Ok(canonical) = fs::canonicalize(directory) {
        spellings.push(canonical.display().to_string());
    }
    spellings
        .iter()
        .any(|spelling| {
            text.contains(&format!("{spelling}/")) || text.contains(&format!("{spelling}\""))
        })
        .then(|| {
            "its record names paths inside its directory (a turn runner's runtime home); retire \
             or restart it to move it"
                .to_owned()
        })
}

/// Why the layout-1 directory `legacy` cannot move now, if it cannot: a process works in it or
/// holds a lock file in it, or its record names paths inside it.
fn unmovable(legacy: &Path) -> Option<String> {
    if let Some(user) = working_inside(legacy).or_else(|| held_lock_inside(legacy)) {
        return Some(format!(
            "in use by process {} ({})",
            user["pid"],
            user["command"].as_str().unwrap_or("")
        ));
    }
    record_names_its_directory(legacy)
}

/// Delete the lock file at `path` if nobody holds it, holding it while it goes so no other
/// process can be granted it in between; `Ok(false)` when it was already gone. `already_held` is
/// this process's own locked handle on it.
fn remove_lock_file(path: &Path, already_held: Option<&File>) -> std::result::Result<bool, String> {
    let opened;
    let _held = match already_held {
        Some(file) => file,
        None => {
            if !path.exists() {
                return Ok(false);
            }
            opened = agent::open_private_lock(path, "layout-1 lock file")
                .map_err(|error| error.to_string())?;
            if FileExt::try_lock_exclusive(&opened).is_err() {
                return Err("held by another process".to_owned());
            }
            &opened
        }
    };
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Move one layout-1 agent; `Ok(None)` when it moved, `Ok(Some(reason))` when it was left.
fn move_agent(
    registry: &Path,
    name: &str,
    wait: Duration,
    removed: &mut Vec<String>,
    kept: &mut Vec<Value>,
) -> Result<Option<String>> {
    let legacy = registry::legacy_directory(registry, name);
    let (legacy_lock_path, legacy_rank_path) =
        registry::lock_paths(registry, name, Location::Legacy);
    let legacy_lock = agent::open_private_lock(&legacy_lock_path, "agent lifecycle lock")?;
    if !lock_within(&legacy_lock, wait) {
        return Ok(Some("its lifecycle lock is busy".to_owned()));
    }
    let (current_lock_path, _) = registry::lock_paths(registry, name, Location::Current);
    let current_lock = agent::open_private_lock(&current_lock_path, "agent lifecycle lock")?;
    if !lock_within(&current_lock, wait) {
        return Ok(Some("its lifecycle lock is busy".to_owned()));
    }
    // Layout 1 gave an agent named `identity` the registry identity lock as its own lock, which
    // this process already holds.
    let identity_path = registry.join(".identity.lock");
    let _identity = if legacy_lock_path == identity_path {
        None
    } else {
        let identity = agent::open_private_lock(&identity_path, "agent identity lock")?;
        if !lock_within(&identity, wait) {
            return Ok(Some("the registry identity lock is busy".to_owned()));
        }
        Some(identity)
    };
    // Everything is held: check again what is where, and whether anything uses it now.
    let metadata = match fs::symlink_metadata(&legacy) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Some("it disappeared before it could be moved".to_owned()))
        }
        Err(error) => {
            return Err(fail(format!(
                "cannot inspect {}: {error}",
                legacy.display()
            )))
        }
    };
    if !metadata.file_type().is_dir() {
        return Ok(Some("it is not a directory".to_owned()));
    }
    if let Err(error) = agent::validate_private_directory(&legacy, "agent directory", false) {
        return Ok(Some(error.to_string()));
    }
    if let Some(reason) = unmovable(&legacy) {
        return Ok(Some(reason));
    }
    let current = registry::current_directory(registry, name);
    if fs::symlink_metadata(&current).is_ok() {
        return Ok(Some(format!(
            "{} already exists; keep one of the two directories",
            current.display()
        )));
    }
    let agents = registry::agents_directory(registry);
    let source = open_directory(registry)?;
    let destination = open_directory(&agents)?;
    crate::subagents::rename_agent_directory(&source, name, &destination, name).map_err(
        |error| {
            fail(format!(
                "cannot move agent directory {} to {}: {error}",
                legacy.display(),
                current.display()
            ))
        },
    )?;
    for parent in [&destination, &source] {
        parent
            .sync_all()
            .map_err(|error| fail(format!("cannot sync agent registry: {error}")))?;
    }
    // The layout-1 locks guard nothing now. The designation lock shares the name of agent
    // `coordinator`'s layout-1 lock and stays, like the identity and restore locks.
    for (path, held) in [
        (legacy_rank_path, None),
        (legacy_lock_path, Some(&legacy_lock)),
    ] {
        if is_fixed_lock(&path) {
            continue;
        }
        match remove_lock_file(&path, held) {
            Ok(true) => removed.push(path.display().to_string()),
            Ok(false) => {}
            Err(reason) => kept.push(json!({"path": path.display().to_string(), "reason": reason})),
        }
    }
    Ok(None)
}

fn is_fixed_lock(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix('.'))
        .and_then(|name| name.strip_suffix(".lock"))
        .is_some_and(|stem| registry::FIXED_LOCK_STEMS.contains(&stem))
}

fn open_directory(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| fail(format!("cannot open {}: {error}", path.display())))
}

/// The layout a registry is in: `"empty"`, `"1"`, `"2"` or `"mixed"`.
fn layout_of(legacy: usize, current: usize, legacy_locks: usize) -> &'static str {
    match (legacy > 0 || legacy_locks > 0, current > 0) {
        (false, false) => "empty",
        (true, false) => "1",
        (false, true) => "2",
        (true, true) => "mixed",
    }
}

/// The layout-1 names that are agents (not strays) and the strays, both sorted.
fn legacy_agents(registry: &Path) -> Result<(Vec<String>, Vec<String>)> {
    let strays = registry::stray_names(registry)?;
    let agents = registry::legacy_names(registry)?
        .into_iter()
        .filter(|name| !strays.contains(name))
        .collect();
    Ok((agents, strays))
}

/// Move every layout-1 agent of `registry` to layout 2 and delete the layout-1 lock files
/// nobody holds; with `apply` off, only report. Running it again does nothing more.
///
/// The report: `registry`, `apply`, `layout_before` and `layout` (`empty`, `1`, `2` or `mixed`),
/// `moved` (or `would_move`), `left` (`[{name, reason}]`), `conflicts` (names with an agent in
/// both layouts), `strays` (layout-1 directories without a record beside a layout-2 agent, which
/// an older program writing into a queue by path leaves; never moved), `removed_lock_files`
/// (count; `would_remove_lock_files` without `apply`), `kept_lock_files` (`[{path, reason}]`),
/// `legacy_support_ends` and `removal_issue`.
///
/// # Errors
///
/// The registry is unsafe or cannot be listed, or layout 2 cannot be prepared.
pub fn migrate(registry: &Path, options: MigrateOptions) -> Result<Value> {
    let mut report = json!({
        "registry": registry.display().to_string(),
        "apply": options.apply,
        "legacy_support_ends": registry::LEGACY_SUPPORT_ENDS,
        "removal_issue": registry::REMOVAL_ISSUE,
    });
    let (moved_key, removed_key) = if options.apply {
        ("moved", "removed_lock_files")
    } else {
        ("would_move", "would_remove_lock_files")
    };
    if !registry.exists() {
        report["layout_before"] = json!("empty");
        report["layout"] = json!("empty");
        report[moved_key] = json!([]);
        report["left"] = json!([]);
        report["conflicts"] = json!([]);
        report["strays"] = json!([]);
        report[removed_key] = json!(0);
        report["kept_lock_files"] = json!([]);
        return Ok(report);
    }
    agent::validate_private_directory(registry, "agent registry", false)?;
    let (legacy, strays) = legacy_agents(registry)?;
    let current = registry::current_names(registry)?;
    let legacy_locks = registry::legacy_lock_files(registry)?;
    let layout_before = layout_of(legacy.len(), current.len(), legacy_locks.len());
    let conflicts: Vec<String> = legacy
        .iter()
        .filter(|name| current.contains(name))
        .cloned()
        .collect();
    let candidates: Vec<String> = legacy
        .iter()
        .filter(|name| !conflicts.contains(name))
        .cloned()
        .collect();
    let mut moved = Vec::new();
    let mut left = Vec::new();
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    if options.apply {
        if !candidates.is_empty() || !current.is_empty() {
            registry::prepare(registry)?;
        }
        for name in &candidates {
            // One agent's failure leaves that agent where it is, not the rest.
            match move_agent(registry, name, options.wait, &mut removed, &mut kept) {
                Ok(None) => moved.push(name.clone()),
                Ok(Some(reason)) => left.push(json!({"name": name, "reason": reason})),
                Err(error) => left.push(json!({"name": name, "reason": error.to_string()})),
            }
        }
        // Lock files whose agent is no longer in layout 1.
        for (agent_name, path) in registry::legacy_lock_files(registry)? {
            if registry::locate(registry, &agent_name).ok() == Some(Location::Legacy)
                || conflicts.contains(&agent_name)
            {
                continue;
            }
            match remove_lock_file(&path, None) {
                Ok(true) => removed.push(path.display().to_string()),
                Ok(false) => {}
                Err(reason) => {
                    kept.push(json!({"path": path.display().to_string(), "reason": reason}));
                }
            }
        }
        let (after_legacy, _) = legacy_agents(registry)?;
        report["layout"] = json!(layout_of(
            after_legacy.len(),
            registry::current_names(registry)?.len(),
            registry::legacy_lock_files(registry)?.len()
        ));
        report[removed_key] = json!(removed.len());
    } else {
        for name in &candidates {
            match unmovable(&registry::legacy_directory(registry, name)) {
                Some(reason) => left.push(json!({"name": name, "reason": reason})),
                None => moved.push(name.clone()),
            }
        }
        report["layout"] = json!(layout_before);
        report[removed_key] = json!(legacy_locks
            .iter()
            .filter(|(_, path)| !is_fixed_lock(path))
            .count());
    }
    report["layout_before"] = json!(layout_before);
    report[moved_key] = json!(moved);
    report["left"] = json!(left);
    report["conflicts"] = json!(conflicts);
    report["strays"] = json!(strays);
    report["kept_lock_files"] = json!(kept);
    Ok(report)
}

fn auto_migrate_disabled(environment: &dyn Fn(&str) -> Option<String>) -> bool {
    environment(AUTO_MIGRATE_VARIABLE).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        )
    })
}

/// Before a command that writes: move whatever layout-1 agents are idle, without waiting for
/// any lock, and return the one-line notice to print when anything moved.
/// Costs one directory listing when the registry is already in layout 2. Turned off by
/// [`AUTO_MIGRATE_VARIABLE`]; never fails the command.
pub fn auto_migrate(
    registry: &Path,
    environment: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    if auto_migrate_disabled(environment) || !registry.is_dir() {
        return None;
    }
    let pending = legacy_agents(registry).and_then(|(legacy, _)| {
        Ok(!legacy.is_empty() || !registry::legacy_lock_files(registry)?.is_empty())
    });
    match pending {
        Ok(false) => return None,
        Ok(true) => {}
        Err(error) => {
            return Some(format!(
                "agentctl: could not move {} to registry layout 2 ({error}); run `agentctl \
                 migrate-layout` to see why",
                registry.display()
            ))
        }
    }
    let report = match migrate(
        registry,
        MigrateOptions {
            apply: true,
            wait: Duration::ZERO,
        },
    ) {
        Ok(report) => report,
        Err(error) => {
            return Some(format!(
                "agentctl: could not move {} to registry layout 2 ({error}); run `agentctl \
                 migrate-layout` to see why",
                registry.display()
            ))
        }
    };
    notice(&report)
}

/// The one-line notice for an automatic migration's `report`, or `None` when nothing moved.
fn notice(report: &Value) -> Option<String> {
    let names = |key: &str| -> Vec<String> {
        report[key]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(|value| {
                        value
                            .as_str()
                            .or_else(|| value["name"].as_str())
                            .map(str::to_owned)
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    let moved = names("moved");
    let left = names("left");
    let conflicts = names("conflicts");
    // An agent left behind is retried by the next command that writes, and a conflict stops any
    // command that uses its name with its own explanation, so only a move is news.
    if moved.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !moved.is_empty() {
        parts.push(format!(
            "moved {} agent(s) to {}/agents/ ({})",
            moved.len(),
            report["registry"].as_str().unwrap_or(""),
            moved.join(", ")
        ));
    }
    if !left.is_empty() {
        parts.push(format!(
            "left {} in the old layout for now, in use or locked ({})",
            left.len(),
            left.join(", ")
        ));
    }
    if !conflicts.is_empty() {
        parts.push(format!(
            "found {} in both layouts, which needs a person ({})",
            conflicts.len(),
            conflicts.join(", ")
        ));
    }
    Some(format!(
        "agentctl: registry layout 2 (#593 registry-agents-subdir): {}. Older agentctl builds \
         cannot use this registry; the old layout stays readable until {}. See `agentctl \
         migrate-layout --help`.",
        parts.join("; "),
        registry::LEGACY_SUPPORT_ENDS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agentctl-layout-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    fn legacy_agent(registry: &Path, name: &str) {
        let directory = registry.join(name);
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            directory.join("agent.json"),
            format!("{{\"name\": \"{name}\"}}"),
        )
        .unwrap();
        for lock in [format!(".{name}.lock"), format!(".{name}.rank.lock")] {
            agent::open_private_lock(&registry.join(lock), "lock").unwrap();
        }
    }

    /// A short wait: a child another test forks briefly holds a copy of every open lock until it
    /// execs, so a lock this test just released can still look busy for an instant.
    const APPLY: MigrateOptions = MigrateOptions {
        apply: true,
        wait: Duration::from_secs(5),
    };
    const APPLY_NOW: MigrateOptions = MigrateOptions {
        apply: true,
        wait: Duration::ZERO,
    };

    #[test]
    fn migration_moves_every_idle_agent_and_is_idempotent() {
        let registry = scratch("idempotent");
        legacy_agent(&registry, "alpha");
        legacy_agent(&registry, "beta");
        fs::write(registry.join("profiles.json"), "{}").unwrap();
        agent::open_private_lock(&registry.join(".identity.lock"), "lock").unwrap();
        agent::open_private_lock(&registry.join(".coordinator.lock"), "lock").unwrap();
        agent::open_private_lock(&registry.join(".gone.lock"), "lock").unwrap();

        let dry = migrate(
            &registry,
            MigrateOptions {
                apply: false,
                wait: Duration::ZERO,
            },
        )
        .unwrap();
        assert_eq!(dry["would_move"], json!(["alpha", "beta"]));
        assert_eq!(dry["would_remove_lock_files"], 5);
        assert!(
            registry.join("alpha/agent.json").exists(),
            "a dry run moves nothing"
        );

        let first = migrate(&registry, APPLY).unwrap();
        assert_eq!(first["layout_before"], "1");
        assert_eq!(first["layout"], "2", "{first:#}");
        assert_eq!(first["moved"], json!(["alpha", "beta"]));
        assert_eq!(first["removed_lock_files"], 5);
        assert!(registry.join("agents/alpha/agent.json").exists());
        assert!(registry::is_tripwire(&registry.join("agents/agent.json")));
        let mut top: Vec<String> = fs::read_dir(&registry)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        top.sort();
        assert_eq!(
            top,
            [
                ".coordinator.lock",
                ".identity.lock",
                ".renames",
                "agents",
                "locks",
                "profiles.json"
            ]
        );

        let second = migrate(&registry, APPLY).unwrap();
        assert_eq!(second["moved"], json!([]));
        assert_eq!(second["removed_lock_files"], 0);
        assert_eq!(second["layout"], "2");
        assert!(auto_migrate(&registry, &|_| None).is_none());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_busy_or_conflicting_agent_stays_where_it_is() {
        let registry = scratch("busy");
        legacy_agent(&registry, "busy");
        legacy_agent(&registry, "twice");
        legacy_agent(&registry, "idle");
        registry::prepare(&registry).unwrap();
        fs::create_dir(registry.join("agents/twice")).unwrap();
        let held = agent::open_private_lock(&registry.join(".busy.lock"), "lock").unwrap();
        FileExt::try_lock_exclusive(&held).unwrap();
        // A second open file description conflicts with `held`, as another process would.
        let report = migrate(&registry, APPLY_NOW).unwrap();
        assert_eq!(report["moved"], json!(["idle"]));
        assert_eq!(report["conflicts"], json!(["twice"]));
        assert_eq!(report["left"][0]["name"], "busy");
        assert!(registry.join("busy/agent.json").exists());
        assert!(registry.join(".busy.lock").exists());
        assert!(registry.join("twice/agent.json").exists());
        assert_eq!(report["layout"], "mixed");
        let notice = notice(&report).unwrap();
        assert!(notice.contains("moved 1 agent(s)"), "{notice}");
        assert!(notice.contains("both layouts"), "{notice}");
        assert!(notice.contains("left 1"), "{notice}");
        drop(held);
        let again = migrate(&registry, APPLY).unwrap();
        assert_eq!(again["moved"], json!(["busy"]));
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn an_agent_another_process_works_in_is_left_alone() {
        let registry = scratch("in-use");
        legacy_agent(&registry, "served");
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .current_dir(registry.join("served"))
            .spawn()
            .unwrap();
        // Wait until the child's working directory is visible in /proc.
        let cwd = PathBuf::from(format!("/proc/{}/cwd", child.id()));
        let target = fs::canonicalize(registry.join("served")).unwrap();
        for _ in 0..100 {
            if fs::read_link(&cwd).is_ok_and(|path| path == target) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let report = migrate(&registry, APPLY).unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(report["moved"], json!([]));
        let reason = report["left"][0]["reason"].as_str().unwrap();
        assert!(reason.starts_with("in use by process"), "{reason}");
        assert!(registry.join("served/agent.json").exists());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn an_agent_whose_record_names_its_own_directory_is_left_alone() {
        let registry = scratch("self-named");
        legacy_agent(&registry, "runner");
        let home = registry.join("runner/runtime");
        fs::write(
            registry.join("runner/agent.json"),
            json!({"name": "runner", "runtime_home": home}).to_string(),
        )
        .unwrap();
        let report = migrate(&registry, APPLY).unwrap();
        assert_eq!(report["moved"], json!([]));
        assert!(report["left"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("runtime home"));
        assert!(registry.join("runner/agent.json").exists());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn an_agent_whose_wrapper_holds_its_lock_is_left_and_an_identity_agent_moves() {
        let registry = scratch("held-inside");
        legacy_agent(&registry, "served");
        legacy_agent(&registry, "identity");
        let wrapper =
            agent::open_private_lock(&registry.join("served/headless.lock"), "lock").unwrap();
        FileExt::try_lock_exclusive(&wrapper).unwrap();
        let report = migrate(&registry, APPLY).unwrap();
        assert_eq!(report["moved"], json!(["identity"]), "{report:#}");
        let reason = report["left"][0]["reason"].as_str().unwrap();
        assert!(reason.starts_with("in use by process"), "{reason}");
        // The identity lock is the registry's, so it stays.
        assert!(registry.join(".identity.lock").exists());
        drop(wrapper);
        assert_eq!(
            migrate(&registry, APPLY).unwrap()["moved"],
            json!(["served"])
        );
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_stray_left_by_an_older_queue_writer_is_reported_not_moved() {
        let registry = scratch("stray");
        legacy_agent(&registry, "coordinator");
        migrate(&registry, APPLY).unwrap();
        // An older agent-teams enqueues by path and recreates REGISTRY/coordinator/queue.
        fs::create_dir_all(registry.join("coordinator/queue/inbox")).unwrap();
        assert_eq!(
            registry::locate(&registry, "coordinator").unwrap(),
            Location::Current
        );
        let report = migrate(&registry, APPLY).unwrap();
        assert_eq!(report["strays"], json!(["coordinator"]));
        assert_eq!(report["conflicts"], json!([]));
        assert_eq!(report["moved"], json!([]));
        assert!(auto_migrate(&registry, &|_| None).is_none());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn auto_migration_can_be_turned_off() {
        let registry = scratch("off");
        legacy_agent(&registry, "alpha");
        let off = |name: &str| (name == AUTO_MIGRATE_VARIABLE).then(|| "0".to_owned());
        assert!(auto_migrate(&registry, &off).is_none());
        assert!(registry.join("alpha").exists());
        let notice = auto_migrate(&registry, &|_| None).unwrap();
        assert!(notice.contains("moved 1 agent(s)"), "{notice}");
        assert!(registry.join("agents/alpha").exists());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_lock_taken_before_a_migration_is_known_to_be_stale() {
        let registry = scratch("stale-lock");
        legacy_agent(&registry, "alpha");
        let (path, _) = registry::lock_paths(&registry, "alpha", Location::Legacy);
        let file = agent::open_private_lock(&path, "lock").unwrap();
        assert!(lock_still_current(&registry, "alpha", Location::Legacy, &file, &path).unwrap());
        migrate(&registry, APPLY).unwrap();
        assert!(!lock_still_current(&registry, "alpha", Location::Legacy, &file, &path).unwrap());
        fs::remove_dir_all(registry).unwrap();
    }
}
