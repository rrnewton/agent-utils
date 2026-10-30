//! How an agent's `agentctl chat` command finds the bridge that serves it (#707
//! chat-prompt-anatomy).
//!
//! A request prompt used to print `chat reply --bridge-state DIR --request KEY --reply-id ID`,
//! with the service's absolute executable path. Now it prints `agentctl chat reply ID --file
//! FILE`, and agentctl finds the bridge the way `git` finds its repository:
//!
//! 1. `--bridge-state DIR`, or `--bridge NAME` for the registry's `bridges/NAME`, when given;
//! 2. else `AGENTCTL_CHAT_BRIDGE`, a state directory, when set;
//! 3. else every bridge registered for discovery: the registry's `bridges/<name>/` entries and the
//!    per-user index `$XDG_STATE_HOME/agentctl/chat-bridges/` (one JSON file per state directory,
//!    which `chat run` writes when it starts). Of those, the bridges whose `bridge.json` targets
//!    the calling agent are kept (the registered agent whose pinned harness is the caller's
//!    nearest ancestor, else `AGENTCTL_AGENT`), and with a reply ID only those that gave it.
//!
//! Exactly one bridge must remain. The index only says where to look: each state's own
//! `bridge.json` and its registry decide, and an entry whose state is gone is skipped.

use std::fs;
use std::io::{self, Read as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::chat_runtime::BridgeState;

/// The registry's directory of bridges, one subdirectory (or link to a state directory) each.
pub const BRIDGES: &str = "bridges";
/// The environment variable naming one bridge state directory for every `chat` command.
pub const BRIDGE_VARIABLE: &str = "AGENTCTL_CHAT_BRIDGE";
/// The environment variable naming the index directory in place of the default; `off` turns
/// registration and index lookup off.
pub const INDEX_VARIABLE: &str = "AGENTCTL_CHAT_BRIDGE_INDEX";
/// The schema of an index entry.
pub const INDEX_SCHEMA: &str = "agentctl-chat-bridge-index/v1";
/// Index entries read at most, so a flooded directory cannot stall a command.
const MAX_INDEX_ENTRIES: usize = 256;
/// Largest index entry read, in bytes.
const MAX_INDEX_ENTRY_BYTES: u64 = 16 * 1_024;

/// One index entry: where a bridge's state is, and the registry its service runs with.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// [`INDEX_SCHEMA`].
    pub schema: String,
    /// The bridge's state directory, canonical.
    pub state: PathBuf,
    /// The registry its `chat run` runs with, canonical when it exists.
    pub registry: PathBuf,
    /// When `chat run` last wrote the entry, in Unix milliseconds.
    pub updated_at_millis: u64,
}

/// The index directory: `AGENTCTL_CHAT_BRIDGE_INDEX`, else `$XDG_STATE_HOME/agentctl/chat-bridges`,
/// else `$HOME/.local/state/agentctl/chat-bridges`; `None` when it is turned off or no home is
/// known.
#[must_use]
pub fn index_directory() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os(INDEX_VARIABLE) {
        if value == "off" || value.is_empty() {
            return None;
        }
        return Some(PathBuf::from(value));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("agentctl").join("chat-bridges"))
}

fn entry_name(state: &Path) -> String {
    let digest = Sha256::digest(state.as_os_str().as_encoded_bytes());
    let hex = digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{hex}.json")
}

/// Record in the index that the bridge whose state is `state` runs with `registry`, and remove
/// entries whose state directory is gone. Both paths are made absolute and canonical first.
///
/// # Errors
///
/// The index is turned off, or its directory or entry cannot be written.
pub fn register(state: &Path, registry: &Path) -> io::Result<PathBuf> {
    let directory = index_directory().ok_or_else(|| {
        io::Error::other(format!("the bridge index is turned off ({INDEX_VARIABLE})"))
    })?;
    register_in(&directory, state, registry)
}

/// [`register`] in the index directory `directory`.
fn register_in(directory: &Path, state: &Path, registry: &Path) -> io::Result<PathBuf> {
    let state = fs::canonicalize(state)?;
    let registry = fs::canonicalize(registry).or_else(|_| std::path::absolute(registry))?;
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    prune(directory);
    let entry = IndexEntry {
        schema: INDEX_SCHEMA.to_owned(),
        state: state.clone(),
        registry,
        updated_at_millis: crate::chat_runtime::unix_millis(),
    };
    let path = directory.join(entry_name(&state));
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        entry_name(&state),
        std::process::id()
    ));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, &entry).map_err(io::Error::other)?;
        io::Write::write_all(&mut file, b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&temporary, &path)?;
    Ok(path)
}

/// Remove index entries whose state directory no longer holds a `bridge.json`.
fn prune(directory: &Path) {
    for (path, entry) in index_entries_in(directory) {
        if !entry.state.join("bridge.json").is_file() {
            let _ = fs::remove_file(path);
        }
    }
}

fn index_entries_in(directory: &Path) -> Vec<(PathBuf, IndexEntry)> {
    let Ok(listing) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for item in listing.flatten().take(MAX_INDEX_ENTRIES) {
        let path = item.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let mut text = String::new();
        if file
            .take(MAX_INDEX_ENTRY_BYTES)
            .read_to_string(&mut text)
            .is_err()
        {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<IndexEntry>(&text) {
            if entry.schema == INDEX_SCHEMA {
                entries.push((path, entry));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// The registry a `chat` command uses: `given` when it exists; otherwise, when it is the relative
/// default `.agentctl`, the nearest `.agentctl` directory in the working directory's ancestors,
/// as `git` finds `.git`; otherwise `given`.
#[must_use]
pub fn find_registry(given: &Path) -> PathBuf {
    if given.exists() || given != Path::new(".agentctl") {
        return given.to_path_buf();
    }
    let Ok(start) = std::env::current_dir() else {
        return given.to_path_buf();
    };
    start
        .ancestors()
        .map(|directory| directory.join(".agentctl"))
        .find(|candidate| candidate.is_dir())
        .unwrap_or_else(|| given.to_path_buf())
}

/// A bridge that a lookup considered: its state directory, the registry it serves, where it was
/// found, and the agent its `bridge.json` targets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Candidate {
    /// The state directory, canonical.
    pub state: PathBuf,
    /// The registry whose agent the bridge targets.
    pub registry: PathBuf,
    /// Where it was found: `registry` or `index`.
    pub source: &'static str,
    /// The agent its `bridge.json` targets.
    pub agent: String,
}

/// Every bridge registered for discovery: the registry's `bridges/` entries, then the index's,
/// each state directory once (by canonical path), skipping any without a readable `bridge.json`.
#[must_use]
pub fn candidates(registry: &Path) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut add = |state: PathBuf, registry: PathBuf, source: &'static str| {
        let Ok(state) = fs::canonicalize(&state) else {
            return;
        };
        if found.iter().any(|candidate| candidate.state == state) {
            return;
        }
        let Ok(bridge) = BridgeState::inspect(&state) else {
            return;
        };
        let agent = bridge.config().agent_name.clone();
        found.push(Candidate {
            state,
            registry,
            source,
            agent,
        });
    };
    if let Ok(listing) = fs::read_dir(registry.join(BRIDGES)) {
        let mut names = listing
            .flatten()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        names.sort();
        for path in names {
            add(path, registry.to_path_buf(), "registry");
        }
    }
    if let Some(directory) = index_directory() {
        for (_, entry) in index_entries_in(&directory) {
            add(entry.state, entry.registry, "index");
        }
    }
    found
}

/// What a lookup settled on: the state directory and, when a reply ID was given, the key of the
/// request that gave it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    /// The bridge's state directory.
    pub state: PathBuf,
    /// The key of the request that gave the reply ID, when one was given.
    pub request: Option<String>,
}

/// What a lookup was asked.
pub struct Query<'a> {
    /// `--bridge-state`.
    pub state: Option<&'a Path>,
    /// `--bridge NAME`.
    pub name: Option<&'a str>,
    /// The reply ID, when the command names one.
    pub reply_id: Option<&'a str>,
    /// `--registry`, before [`find_registry`].
    pub registry: &'a Path,
}

/// Find the bridge for `query`: see the module documentation. `caller` names the calling agent
/// in a registry, or `None` when it cannot tell; `environment` is `AGENTCTL_CHAT_BRIDGE`.
///
/// # Errors
///
/// No bridge, or more than one, fits; or the reply ID names no request of the bridge named.
pub fn resolve(
    query: &Query<'_>,
    environment: Option<PathBuf>,
    caller: &dyn Fn(&Path) -> Option<String>,
) -> Result<Resolved, String> {
    let registry = find_registry(query.registry);
    if let Some(name) = query.name {
        if name.is_empty() || name.starts_with('.') || name.contains('/') {
            return Err(format!(
                "--bridge {name:?} must name an entry of {}, without `/`",
                registry.join(BRIDGES).display()
            ));
        }
    }
    let named = query
        .state
        .map(Path::to_path_buf)
        .or_else(|| {
            // A registry entry may be a link to a state directory kept elsewhere.
            query.name.map(|name| {
                let entry = registry.join(BRIDGES).join(name);
                fs::canonicalize(&entry).unwrap_or(entry)
            })
        })
        .or(environment);
    if let Some(state) = named {
        let request = match query.reply_id {
            Some(reply_id) => Some(request_in(&state, reply_id)?.ok_or_else(|| {
                format!(
                    "bridge state {} gave no request the reply ID {reply_id:?}",
                    state.display()
                )
            })?),
            None => None,
        };
        return Ok(Resolved { state, request });
    }
    let all = candidates(&registry);
    if all.is_empty() {
        return Err(format!(
            "no chat bridge is registered for discovery: none under {} and none in the index {}; \
             pass --bridge-state DIR (the --bridge-state of the bridge's `chat run`)",
            registry.join(BRIDGES).display(),
            index_directory().map_or_else(|| "(off)".to_owned(), |path| path.display().to_string()),
        ));
    }
    let mut callers = std::collections::BTreeMap::new();
    let mut caller_of = |registry: &Path| {
        callers
            .entry(registry.to_path_buf())
            .or_insert_with(|| caller(registry))
            .clone()
    };
    let targeted = all
        .iter()
        .filter(|candidate| {
            caller_of(&candidate.registry).as_deref() == Some(candidate.agent.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    let pool = if targeted.is_empty() {
        all.clone()
    } else {
        targeted.clone()
    };
    let mut fitting = Vec::new();
    for candidate in pool {
        let request = match query.reply_id {
            Some(reply_id) => match request_in(&candidate.state, reply_id) {
                Ok(Some(key)) => Some(key),
                Ok(None) | Err(_) => continue,
            },
            None => None,
        };
        fitting.push((candidate, request));
    }
    let describe = |candidates: &[Candidate]| {
        candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{} (agent {}, from the {})",
                    candidate.state.display(),
                    candidate.agent,
                    candidate.source
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    match fitting.len() {
        1 => {
            let (candidate, request) = fitting.remove(0);
            Ok(Resolved {
                state: candidate.state,
                request,
            })
        }
        0 => Err(match query.reply_id {
            Some(reply_id) => format!(
                "no chat bridge {}gave the reply ID {reply_id:?}; bridges considered: {}",
                if targeted.is_empty() {
                    ""
                } else {
                    "serving you "
                },
                describe(if targeted.is_empty() { &all } else { &targeted })
            ),
            None => "no chat bridge fits".to_owned(),
        }),
        _ => Err(format!(
            "{} chat bridges fit{}; pass --bridge NAME or --bridge-state DIR: {}",
            fitting.len(),
            if targeted.is_empty() {
                " and agentctl cannot tell which agent you are"
            } else {
                ""
            },
            describe(
                &fitting
                    .into_iter()
                    .map(|(candidate, _)| candidate)
                    .collect::<Vec<_>>()
            )
        )),
    }
}

fn request_in(state: &Path, reply_id: &str) -> Result<Option<String>, String> {
    let bridge = BridgeState::inspect(state)
        .map_err(|error| format!("cannot read bridge state {}: {error}", state.display()))?;
    bridge
        .request_for_reply_id(reply_id)
        .map_err(|error| format!("cannot read bridge state {}: {error}", state.display()))
}

/// Whether the `agentctl` that this process's `PATH` finds first is this build: the same file, or
/// a program (such as a wrapper) whose `--version` output is this build's, when this build names
/// its commit. A request prompt prints plain `agentctl` only then, so an older build on `PATH`
/// never receives syntax it does not know.
#[must_use]
pub fn agentctl_on_path_is_this_build() -> bool {
    let Some(found) = on_path("agentctl") else {
        return false;
    };
    let this = std::env::current_exe().and_then(fs::canonicalize).ok();
    if this.is_some() && fs::canonicalize(&found).ok() == this {
        return true;
    }
    if crate::build_info::SOURCE_COMMIT.is_none() {
        return false;
    }
    version_output(&found, Duration::from_secs(3)).is_some_and(|output| {
        output.trim_end() == format!("agentctl {}", crate::build_info::version_text()).trim_end()
    })
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(program))
        .find(|candidate| {
            fs::metadata(candidate).is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

fn version_output(program: &Path, timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut output = String::new();
    child
        .stdout
        .take()?
        .take(64 * 1_024)
        .read_to_string(&mut output)
        .ok()?;
    Some(output)
}

/// The page `agentctl chat help` prints without a request to describe: with no reply ID, or when
/// the bridge for one cannot be found (`problem` says why).
#[must_use]
pub fn generic_help(reply_id: Option<&str>, problem: Option<&str>) -> String {
    let mut page = String::new();
    if let (Some(reply_id), Some(problem)) = (reply_id, problem) {
        page.push_str(&format!(
            "agentctl could not find the bridge for reply ID {reply_id}: {problem}\n\n"
        ));
    }
    let id = reply_id.unwrap_or("ID");
    page.push_str(&format!(
        "Answering a chat request. A request prompt opens with a header line, `[from SENDER via \
PROVIDER CHANNEL · sent: TIME · reply: ...]`, then the message between its tag lines, \
`<chat_message>` and `</chat_message>`, then how to reply. ID below is the request's reply ID.\n\n\
The normal way to reply: print the reply as ordinary text between two marker lines, each alone \
on its line and outside code fences. The opening line is `<CHAT_REPLY_`, then the reply ID, then \
`>`; the closing line is `</CHAT_REPLY_`, then the reply ID, then `>`. The text between them \
reaches the chat and stays in your terminal. Use the same two lines for every reply (for a long \
ID ending in a number, add one to that number for each later reply), make no tool call between \
them, and give each reply new wording.\n\n\
When the marker lines cannot do the job (a reply before your turn ends, or one that did not reach \
the chat), write the reply to a file and run:\n  agentctl chat reply {id} --file FILE\n\
Send each reply one way only. If the command fails twice, print the reply between the lines at \
the end of your turn instead.\n\n\
What reached the chat:            agentctl chat sent {id}\n\
Earlier messages in the thread:   agentctl chat thread {id}\n\
This page for one request:        agentctl chat help {id}\n\n\
These find the bridge that serves you by caller lookup; when that fails, add --bridge-state DIR \
(and --request KEY to `chat reply`). The whole protocol: agentctl chat userguide\n"
    ));
    page
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registering_writes_one_private_entry_per_state_and_prunes_gone_ones() {
        let root =
            std::env::temp_dir().join(format!("agentctl-chat-lookup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (index, state, gone) = (root.join("index"), root.join("state"), root.join("gone"));
        for directory in [&state, &gone] {
            fs::create_dir_all(directory).expect("state directory");
            fs::write(directory.join("bridge.json"), b"{}").expect("bridge.json");
        }
        let first = register_in(&index, &state, &root.join("registry")).expect("register");
        let again = register_in(&index, &state, &root.join("registry")).expect("register again");
        assert_eq!(first, again, "one entry per state directory");
        register_in(&index, &gone, &root.join("registry")).expect("register another");
        assert_eq!(index_entries_in(&index).len(), 2);
        let mode = fs::metadata(&index).expect("index").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        let mode = fs::metadata(&first).expect("entry").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let (_, entry) = &index_entries_in(&index)
            .into_iter()
            .find(|(path, _)| *path == first)
            .expect("the entry");
        assert_eq!(entry.state, fs::canonicalize(&state).expect("canonical"));
        assert_eq!(entry.schema, INDEX_SCHEMA);
        // The next registration removes the entry of a state directory that is gone.
        fs::remove_dir_all(&gone).expect("remove a state");
        register_in(&index, &state, &root.join("registry")).expect("register once more");
        assert_eq!(index_entries_in(&index).len(), 1);
        fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn a_bridge_name_is_one_entry_of_the_registrys_bridges() {
        for name in ["", ".", "..", "a/b", ".hidden"] {
            let query = Query {
                state: None,
                name: Some(name),
                reply_id: None,
                registry: Path::new("/nonexistent-registry"),
            };
            assert!(resolve(&query, None, &|_| None).is_err(), "{name:?}");
        }
    }
}
