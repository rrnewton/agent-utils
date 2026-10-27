//! Where an agent registry keeps each agent (#593 registry-agents-subdir).
//!
//! A registry holds a few fixed items and several collections that grow with the number of
//! agents. Every such collection has a subdirectory of its own, so the fixed items stay in plain
//! sight however many agents have come and gone (layout 2):
//!
//! ```text
//! REGISTRY/
//!   profiles.json, coordinator.json, restore.json, landing-queue.json    fixed
//!   .identity.lock, .coordinator.lock, .restore.lock                     fixed, registry-wide
//!   agents/<name>/            one directory per active agent: agent.json, queue/, logs
//!   agents/agent.json         the layout tripwire (below); never an agent
//!   locks/<name>.lock         the agent's lifecycle lock
//!   locks/<name>.rank.lock    its rank companion
//!   archive/<name>-<token>/   retired agents
//!   .progress/, .waiting/, .renames/, .revives/, .inbox/                 other collections
//! ```
//!
//! **Layout 1**, written by every build before this module, kept each agent directory at
//! `REGISTRY/<name>/` and its two locks at `REGISTRY/.<name>.lock` and
//! `REGISTRY/.<name>.rank.lock`, beside the fixed items. Readers accept both layouts until
//! [`LEGACY_SUPPORT_ENDS`]; new agents are always created in layout 2, and the registry owner's
//! layout migration (also run automatically by commands that change an agent) moves the rest.
//!
//! **One location per agent.** An agent lives in exactly one layout. [`locate`] finds it, and
//! refuses a name with a directory in both, which only an old build creating an agent that a new
//! build had already moved can produce: neither copy is chosen silently.
//!
//! **Locks follow the directory.** A layout-1 agent is locked through its layout-1 lock files,
//! the ones older builds use, so old and new builds still exclude each other on it; a layout-2
//! agent is locked through `locks/`. A caller that takes a lock re-locates the agent afterwards
//! and starts again if a migration moved it meanwhile.
//!
//! **The tripwires.** Two files make builds that know only layout 1 refuse a layout-2 registry
//! instead of misreading it; they never write into `agents/`.
//!
//! - Such a build treats every directory entry whose name looks like an agent name as an agent,
//!   including `agents`, and reads `agents/agent.json` as its record. That file holds a JSON
//!   string instead of an object, which it refuses as an unreadable record: its pane-uniqueness
//!   and session-uniqueness proofs fail, and a slot manager's liveness probe reports the registry
//!   unverifiable instead of calling a live agent dead.
//! - Before it starts, adopts, loads, or renames an agent, such a build lists the rename journals
//!   in `.renames/`, and refuses any entry that is neither a `.json` journal nor a dot-file. The
//!   file [`RENAMES_TRIPWIRE`] there stops it before it has done anything, with a message naming
//!   that file. Without it an old `start` would launch a harness before its pane check refused.
//!
//! A program that writes into an agent's queue by path without reading the registry (a team
//! router built before layout 2) is stopped by neither; what it leaves behind is a stray (see
//! [`locate`]).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::{io_error, Error, Result};
use crate::fsio;

/// The directory holding one subdirectory per active agent.
pub const AGENTS: &str = "agents";
/// The directory holding the agents' lifecycle and rank lock files.
pub const LOCKS: &str = "locks";
/// The tripwire file inside [`AGENTS`]; its name is what a layout-1 build reads as a record.
pub const TRIPWIRE: &str = "agent.json";
/// The tripwire's content: a JSON string, so a layout-1 build cannot read it as a record.
pub const TRIPWIRE_TEXT: &str = "agentctl registry layout 2: each agent lives in \
agents/<name>/ and its locks in locks/. This file is not an agent; it makes agentctl builds \
older than layout 2 refuse this registry instead of misreading it. Upgrade agentctl.";
/// The tripwire in the rename-journal directory `.renames/`; readers of rename journals skip it.
pub const RENAMES_TRIPWIRE: &str = "upgrade-agentctl-registry-layout-2";
/// Its content, for whoever opens it.
pub const RENAMES_TRIPWIRE_TEXT: &str = "This registry uses agentctl registry layout 2. agentctl \
builds older than layout 2 stop at this file; upgrade agentctl. Readers of rename journals skip it.\n";
/// Names no agent may take: each is a directory of the registry itself.
pub const RESERVED_NAMES: [&str; 3] = ["archive", AGENTS, LOCKS];
/// The registry-wide lock files, `REGISTRY/.<stem>.lock`, that are fixed items, not agents'
/// layout-1 locks. Layout 1 shared `.coordinator.lock` between the designation and an agent
/// named `coordinator`; it stays, as the designation lock.
pub const FIXED_LOCK_STEMS: [&str; 3] = ["identity", "coordinator", "restore"];
/// The last day on which readers accept layout 1 (#593 registry-agents-subdir).
pub const LEGACY_SUPPORT_ENDS: &str = "2026-11-26";
/// The issue that removes layout-1 support after [`LEGACY_SUPPORT_ENDS`].
pub const REMOVAL_ISSUE: &str = "#597 registry-old-layout-removal";

/// Which layout an agent's directory is in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    /// Layout 2: `REGISTRY/agents/<name>/`, locked through `REGISTRY/locks/`.
    Current,
    /// Layout 1: `REGISTRY/<name>/`, locked through `REGISTRY/.<name>.lock`.
    Legacy,
}

/// Is `value` an agent name of the shared on-disk formats, `[a-z][a-z0-9-]{0,31}`, and not one
/// of the [`RESERVED_NAMES`]?
#[must_use]
pub fn is_agent_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !RESERVED_NAMES.contains(&value)
}

/// `REGISTRY/agents`.
#[must_use]
pub fn agents_directory(registry: &Path) -> PathBuf {
    registry.join(AGENTS)
}

/// `REGISTRY/locks`.
#[must_use]
pub fn locks_directory(registry: &Path) -> PathBuf {
    registry.join(LOCKS)
}

/// An agent's directory in layout 2, whether or not it exists.
#[must_use]
pub fn current_directory(registry: &Path, name: &str) -> PathBuf {
    agents_directory(registry).join(name)
}

/// An agent's directory in layout 1, whether or not it exists.
#[must_use]
pub fn legacy_directory(registry: &Path, name: &str) -> PathBuf {
    registry.join(name)
}

/// The directory of agent `name` at `location`.
#[must_use]
pub fn directory_at(registry: &Path, name: &str, location: Location) -> PathBuf {
    match location {
        Location::Current => current_directory(registry, name),
        Location::Legacy => legacy_directory(registry, name),
    }
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error("inspect agent directory", path, error)),
    }
}

/// Is this layout-1 entry a stray: a directory with no record, beside a layout-2 agent of
/// the same name? An older program that writes into an agent's queue by path (a team router
/// built before layout 2) recreates `REGISTRY/<name>/queue/` that way after a migration. A stray
/// is not an agent: [`locate`] answers layout 2, and the layout migration reports it.
fn stray(legacy: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(legacy) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect agent directory", legacy, error)),
    };
    Ok(metadata.file_type().is_dir() && !present(&legacy.join("agent.json"))?)
}

/// Where agent `name` lives: layout 1 when anything is at `REGISTRY/<name>`, otherwise layout 2
/// (also the answer for an agent that does not exist yet, so new agents go there). A name with an
/// agent in both places is refused; a layout-1 [`stray`] beside a layout-2 agent is ignored.
///
/// # Errors
///
/// A reserved name, an agent in both layouts, or a directory that cannot be inspected.
pub fn locate(registry: &Path, name: &str) -> Result<Location> {
    if RESERVED_NAMES.contains(&name) {
        return Err(Error::invalid(format!(
            "'{name}' is reserved and cannot name an agent"
        )));
    }
    let legacy = legacy_directory(registry, name);
    let current = current_directory(registry, name);
    match (present(&legacy)?, present(&current)?) {
        (true, true) if stray(&legacy)? => Ok(Location::Current),
        (true, true) => Err(Error::invalid(format!(
            "agent '{name}' has a directory in both registry layouts, {} and {}; an agentctl build \
             older than layout 2 probably created the first after the second was migrated. Keep \
             the one you want and move the other aside (see `agentctl migrate-layout --help`)",
            legacy.display(),
            current.display()
        ))),
        (true, false) => Ok(Location::Legacy),
        _ => Ok(Location::Current),
    }
}

/// Agent `name`'s directory: see [`locate`].
///
/// # Errors
///
/// As for [`locate`].
pub fn directory(registry: &Path, name: &str) -> Result<PathBuf> {
    Ok(directory_at(registry, name, locate(registry, name)?))
}

/// The lifecycle lock and rank lock of agent `name` at `location`.
#[must_use]
pub fn lock_paths(registry: &Path, name: &str, location: Location) -> (PathBuf, PathBuf) {
    match location {
        Location::Current => {
            let locks = locks_directory(registry);
            (
                locks.join(format!("{name}.lock")),
                locks.join(format!("{name}.rank.lock")),
            )
        }
        Location::Legacy => (
            registry.join(format!(".{name}.lock")),
            registry.join(format!(".{name}.rank.lock")),
        ),
    }
}

fn entry_names(directory: &Path) -> Result<Vec<String>> {
    match fs::read_dir(directory) {
        Ok(entries) => {
            let mut names = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|error| io_error("list", directory, error))?;
                if let Some(name) = entry.file_name().to_str() {
                    names.push(name.to_owned());
                }
            }
            Ok(names)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(io_error("list", directory, error)),
    }
}

/// The agent names with an entry in layout 1, sorted.
///
/// # Errors
///
/// The registry exists but cannot be listed.
pub fn legacy_names(registry: &Path) -> Result<Vec<String>> {
    let mut names = entry_names(registry)?;
    names.retain(|name| is_agent_name(name));
    names.sort();
    Ok(names)
}

/// The agent names with an entry in layout 2, sorted.
///
/// # Errors
///
/// `REGISTRY/agents` exists but cannot be listed.
pub fn current_names(registry: &Path) -> Result<Vec<String>> {
    let mut names = entry_names(&agents_directory(registry))?;
    names.retain(|name| is_agent_name(name));
    names.sort();
    Ok(names)
}

/// The names whose layout-1 entry is a [`stray`] beside a layout-2 agent, sorted.
///
/// # Errors
///
/// A directory that exists but cannot be listed or inspected.
pub fn stray_names(registry: &Path) -> Result<Vec<String>> {
    let current = current_names(registry)?;
    let mut strays = Vec::new();
    for name in legacy_names(registry)? {
        if current.contains(&name) && stray(&legacy_directory(registry, &name))? {
            strays.push(name);
        }
    }
    Ok(strays)
}

/// Every agent directory that holds `agent.json` under `name`, in either layout: one path
/// normally, two for a name in both layouts. A check that must count every claim (pane
/// ownership) reads them all instead of refusing the whole registry over one conflicted name.
///
/// # Errors
///
/// A directory that cannot be inspected.
pub fn record_directories(registry: &Path, name: &str) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for directory in [
        current_directory(registry, name),
        legacy_directory(registry, name),
    ] {
        if present(&directory.join("agent.json"))? {
            found.push(directory);
        }
    }
    Ok(found)
}

/// `path`, a file or directory inside a layout-1 agent directory (`REGISTRY/<name>/queue`, say)
/// recorded before a migration, where it is now: inside `REGISTRY/agents/<name>/` when the
/// layout-1 path is gone and the layout-2 one exists, otherwise `path` unchanged.
#[must_use]
pub fn relocated(path: &Path) -> PathBuf {
    if path.exists() {
        return path.to_path_buf();
    }
    let mut components: Vec<&std::ffi::OsStr> = Vec::new();
    let mut agent_directory = path;
    // Walk up to the first ancestor whose parent holds a layout-2 directory of the same name.
    while let (Some(name), Some(parent)) = (agent_directory.file_name(), agent_directory.parent()) {
        if name.to_str().is_some_and(is_agent_name) {
            let mut candidate = parent.join(AGENTS).join(name);
            for component in components.iter().rev() {
                candidate.push(component);
            }
            if candidate.exists() && is_tripwire(&parent.join(AGENTS).join(TRIPWIRE)) {
                return candidate;
            }
        }
        components.push(name);
        agent_directory = parent;
    }
    path.to_path_buf()
}

/// Every agent name with an entry in either layout, sorted and without repeats. A name in both
/// appears once; [`locate`] reports the conflict when it is used.
///
/// # Errors
///
/// A directory that exists but cannot be listed.
pub fn names(registry: &Path) -> Result<Vec<String>> {
    let mut names = legacy_names(registry)?;
    names.extend(current_names(registry)?);
    names.sort();
    names.dedup();
    Ok(names)
}

/// The layout-1 lock files left in the registry's top level, as `(agent name, path)`, sorted by
/// path: `.<name>.lock` and `.<name>.rank.lock`, except the [`FIXED_LOCK_STEMS`] (whose
/// `.rank.lock` companions are agents' locks, and listed).
///
/// # Errors
///
/// The registry exists but cannot be listed.
pub fn legacy_lock_files(registry: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    for entry in entry_names(registry)? {
        let Some(stem) = entry
            .strip_prefix('.')
            .and_then(|rest| rest.strip_suffix(".lock"))
        else {
            continue;
        };
        let (agent, rank) = match stem.strip_suffix(".rank") {
            Some(agent) => (agent, true),
            None => (stem, false),
        };
        if is_agent_name(agent) && (rank || !FIXED_LOCK_STEMS.contains(&agent)) {
            found.push((agent.to_owned(), registry.join(&entry)));
        }
    }
    found.sort_by(|left, right| left.1.cmp(&right.1));
    Ok(found)
}

/// Is `path` the tripwire, or anything else [`TRIPWIRE`] might be?
#[must_use]
pub fn is_tripwire(path: &Path) -> bool {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value.is_string())
}

/// Create `REGISTRY/agents` and `REGISTRY/locks` (mode 0700) and both tripwires, whichever are
/// missing. Every writer calls this before it creates anything in layout 2, so a layout-1 build
/// can never see a layout-2 agent without also meeting the tripwire.
///
/// # Errors
///
/// A directory or the tripwire cannot be created, or exists and is unsafe.
pub fn prepare(registry: &Path) -> Result<()> {
    fsio::create_private_directory(registry, "agent registry", true, true)?;
    // Builds before layout 2 allowed agents named `agents` and `locks`; such an agent's
    // directory is no place for a collection.
    for (collection, record) in [
        (AGENTS, agents_directory(registry).join(TRIPWIRE)),
        (LOCKS, locks_directory(registry).join("agent.json")),
    ] {
        if present(&record)? && (collection == LOCKS || !is_tripwire(&record)) {
            return Err(Error::unsafe_path(format!(
                "{} is the directory of a layout-1 agent named '{collection}', which layout 2 \
                 reserves; rename it with an older agentctl first",
                registry.join(collection).display()
            )));
        }
    }
    let agents = agents_directory(registry);
    fsio::create_private_directory(&agents, "agent directory collection", false, true)?;
    let tripwire = agents.join(TRIPWIRE);
    match fs::symlink_metadata(&tripwire) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(Error::unsafe_path(format!(
                "unsafe registry layout tripwire: {}",
                tripwire.display()
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match fsio::atomic_json_create(&tripwire, &Value::String(TRIPWIRE_TEXT.to_owned())) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(io_error(
                        "create registry layout tripwire",
                        &tripwire,
                        error,
                    ))
                }
            }
        }
        Err(error) => {
            return Err(io_error(
                "inspect registry layout tripwire",
                &tripwire,
                error,
            ))
        }
    }
    let renames = registry.join(".renames");
    fsio::create_private_directory(&renames, "rename journal directory", false, true)?;
    let renames_tripwire = renames.join(RENAMES_TRIPWIRE);
    if fs::symlink_metadata(&renames_tripwire).is_err() {
        create_text_noreplace(&renames_tripwire, RENAMES_TRIPWIRE_TEXT)?;
    }
    fsio::create_private_directory(
        &locks_directory(registry),
        "agent lock collection",
        false,
        true,
    )
}

/// Durably create `path` (mode 0600) holding `text`, unless something is already there.
fn create_text_noreplace(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path
        .parent()
        .ok_or_else(|| Error::invalid(format!("path has no parent: {}", path.display())))?;
    let temporary = parent.join(format!(".layout.{}.tmp", std::process::id()));
    let written = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        match fs::hard_link(&temporary, path) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
            _ => Ok(()),
        }
    })();
    let _ = fs::remove_file(&temporary);
    written.map_err(|error| io_error("create registry layout tripwire", path, error))?;
    fsio::sync_directory(parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "agent-queue-registry-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn a_new_agent_goes_to_layout_two_and_an_old_one_stays_found() {
        let registry = scratch("locate");
        assert_eq!(locate(&registry, "fresh").unwrap(), Location::Current);
        assert_eq!(
            directory(&registry, "fresh").unwrap(),
            registry.join("agents/fresh")
        );
        fs::create_dir(registry.join("old")).unwrap();
        assert_eq!(locate(&registry, "old").unwrap(), Location::Legacy);
        assert_eq!(directory(&registry, "old").unwrap(), registry.join("old"));
        assert_eq!(
            lock_paths(&registry, "old", Location::Legacy),
            (registry.join(".old.lock"), registry.join(".old.rank.lock"))
        );
        assert_eq!(
            lock_paths(&registry, "fresh", Location::Current),
            (
                registry.join("locks/fresh.lock"),
                registry.join("locks/fresh.rank.lock")
            )
        );
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_name_in_both_layouts_is_refused_and_listed_once() {
        let registry = scratch("conflict");
        prepare(&registry).unwrap();
        fs::create_dir(registry.join("twice")).unwrap();
        fs::create_dir(registry.join("agents/twice")).unwrap();
        fs::create_dir(registry.join("agents/new")).unwrap();
        fs::create_dir(registry.join("old")).unwrap();
        // Without a record, the layout-1 entry is a stray, not a second agent.
        assert_eq!(locate(&registry, "twice").unwrap(), Location::Current);
        assert_eq!(stray_names(&registry).unwrap(), ["twice"]);
        fs::write(registry.join("twice/agent.json"), "{}").unwrap();
        let error = locate(&registry, "twice").unwrap_err();
        assert!(error.message().contains("both registry layouts"), "{error}");
        assert!(stray_names(&registry).unwrap().is_empty());
        assert_eq!(record_directories(&registry, "twice").unwrap().len(), 1);
        assert_eq!(names(&registry).unwrap(), ["new", "old", "twice"]);
        assert!(locate(&registry, "agents").is_err());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn reserved_entries_and_the_tripwire_are_never_agents() {
        let registry = scratch("reserved");
        prepare(&registry).unwrap();
        fs::create_dir(registry.join("archive")).unwrap();
        assert!(names(&registry).unwrap().is_empty());
        assert!(is_tripwire(&registry.join("agents/agent.json")));
        assert!(registry.join(".renames").join(RENAMES_TRIPWIRE).is_file());
        // A second prepare keeps the tripwire and succeeds.
        prepare(&registry).unwrap();
        let text = fs::read_to_string(registry.join("agents/agent.json")).unwrap();
        assert!(serde_json::from_str::<Value>(&text).unwrap().is_string());
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_layout_one_agent_named_like_a_collection_stops_layout_two() {
        let registry = scratch("collection-name");
        fs::create_dir(registry.join("agents")).unwrap();
        fs::write(registry.join("agents/agent.json"), "{\"name\": \"agents\"}").unwrap();
        let error = prepare(&registry).unwrap_err();
        assert!(error.message().contains("named 'agents'"), "{error}");
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn a_path_recorded_before_a_migration_is_found_after_it() {
        let registry = scratch("relocated");
        let old = registry.join("coordinator/queue");
        assert_eq!(relocated(&old), old);
        prepare(&registry).unwrap();
        fs::create_dir_all(registry.join("agents/coordinator/queue")).unwrap();
        assert_eq!(relocated(&old), registry.join("agents/coordinator/queue"));
        fs::create_dir_all(&old).unwrap();
        assert_eq!(relocated(&old), old);
        fs::remove_dir_all(registry).unwrap();
    }

    #[test]
    fn legacy_lock_files_are_found_by_agent() {
        let registry = scratch("locks");
        for file in [
            ".a.lock",
            ".a.rank.lock",
            ".identity.lock",
            ".coordinator.lock",
            ".coordinator.rank.lock",
            ".b-2.lock",
            "x.lock",
        ] {
            fs::write(registry.join(file), b"").unwrap();
        }
        let found = legacy_lock_files(&registry).unwrap();
        let agents: Vec<&str> = found.iter().map(|(agent, _)| agent.as_str()).collect();
        assert_eq!(agents, ["a", "a", "b-2", "coordinator"]);
        fs::remove_dir_all(registry).unwrap();
    }
}
