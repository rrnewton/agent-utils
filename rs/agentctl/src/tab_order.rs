//! Coordinator tabs come first in their Herdr workspace (#849 coordinator-tab-first).
//!
//! A coordinator is the agent people look for first, so agentctl moves its tab to the front of
//! its workspace when it starts or revives one, and when it designates one. Herdr's command line
//! has no verb for moving a tab, so this speaks Herdr's socket API: `tab.list` for the
//! workspace's order, then `tab.move` with the new index. Coordinators already at the front keep
//! their order, and a new one goes after them. The move is best-effort: a failure is reported
//! and never fails the command that asked for it.
//!
//! An agent counts as a coordinator when it is the registry's designated coordinator, or when it
//! was started with a launch profile named `coordinator` or ending in `-coordinator`.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};

/// How long one request to the Herdr socket may take, connect to reply.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether a launch profile named `profile` is a coordinator's.
#[must_use]
pub fn is_coordinator_profile(profile: Option<&str>) -> bool {
    profile.is_some_and(|name| name == "coordinator" || name.ends_with("-coordinator"))
}

/// The Herdr socket: `$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock`.
#[must_use]
pub fn socket_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("HERDR_SOCKET_PATH").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(Path::new(&home).join(".config/herdr/herdr.sock"))
}

/// The Herdr socket to move tabs through when agentctl drives the Herdr program `executable`:
/// [`socket_path`], when it exists and `executable` is the Herdr that serves it. The socket
/// belongs to the running Herdr server, not to the program agentctl was told to call, so a
/// stand-in Herdr (a test's script, run from a shell inside a real Herdr pane) must never move
/// the real server's tabs. Inside a Herdr pane `executable` must be the program `HERDR_BIN_PATH`
/// names; elsewhere it must be a compiled program, not a script. A bare name is looked up on
/// `PATH`.
#[must_use]
pub fn socket_for(executable: &Path) -> Option<PathBuf> {
    let socket = socket_path().filter(|path| path.exists())?;
    let program = resolve(executable)?.canonicalize().ok()?;
    let serving = match std::env::var_os("HERDR_BIN_PATH").filter(|path| !path.is_empty()) {
        Some(running) => Path::new(&running).canonicalize().ok().as_ref() == Some(&program),
        None => is_compiled(&program),
    };
    serving.then_some(socket)
}

/// `executable` as given when it names a path, else the first match on `PATH`.
fn resolve(executable: &Path) -> Option<PathBuf> {
    if executable.components().count() > 1 {
        return Some(executable.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(executable))
        .find(|candidate| candidate.is_file())
}

/// Whether `program` is an ELF executable rather than a script.
fn is_compiled(program: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0_u8; 4];
    std::fs::File::open(program)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok_and(|()| magic == *b"\x7fELF")
}

/// Where `tab` should move in a workspace whose tabs are `order`, front first: just after the
/// leading run of other coordinators' tabs (`coordinators`). `None` when it is already within
/// that run or right after it, or not in the workspace at all.
#[must_use]
pub fn target_index(order: &[String], tab: &str, coordinators: &BTreeSet<String>) -> Option<usize> {
    let position = order.iter().position(|candidate| candidate == tab)?;
    let leading = order
        .iter()
        .take_while(|candidate| *candidate != tab && coordinators.contains(*candidate))
        .count();
    (position > leading).then_some(leading)
}

/// Send one request to the Herdr socket at `socket` and return its `result`.
fn request(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("cannot connect to {}: {error}", socket.display()))?;
    stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(SOCKET_TIMEOUT)))
        .map_err(|error| error.to_string())?;
    let id = format!("agentctl:tab-order:{}", std::process::id());
    let mut line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
        .map_err(|error| error.to_string())?;
    line.push(b'\n');
    stream
        .write_all(&line)
        .map_err(|error| format!("{method}: {error}"))?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut reply = String::new();
        let read = reader
            .read_line(&mut reply)
            .map_err(|error| format!("{method}: {error}"))?;
        if read == 0 {
            return Err(format!("{method}: Herdr closed the socket without a reply"));
        }
        let Ok(document) = serde_json::from_str::<Value>(&reply) else {
            return Err(format!("{method}: Herdr sent a reply that is not JSON"));
        };
        if document["id"] != json!(id) {
            continue;
        }
        if let Some(error) = document.get("error") {
            return Err(format!(
                "{method}: {}: {}",
                error["code"].as_str().unwrap_or("error"),
                error["message"].as_str().unwrap_or("")
            ));
        }
        return Ok(document["result"].clone());
    }
}

/// Tab roles in canonical order:
/// liaison/coordinators first, then managed lanes, then unmanaged tabs.
pub const ROLE_FRONT: u8 = 0;
/// A managed lane's tab.
pub const ROLE_LANE: u8 = 1;
/// A tab no registered agent claims.
pub const ROLE_UNMANAGED: u8 = 2;

/// The canonical order of `tabs` (`(tab_id, sort_name, role)`): by role,
/// then by name, stable within equal keys. Idempotent by construction.
#[must_use]
pub fn canonical_tabs(tabs: &[(String, String, u8)]) -> Vec<String> {
    let mut indexed: Vec<(usize, &(String, String, u8))> = tabs.iter().enumerate().collect();
    indexed.sort_by(|(left_index, left), (right_index, right)| {
        (left.2, left.1.as_str(), *left_index).cmp(&(right.2, right.1.as_str(), *right_index))
    });
    indexed
        .into_iter()
        .map(|(_, (tab, _, _))| tab.clone())
        .collect()
}

/// List `workspace`'s tab IDs in shown order through the Herdr socket.
///
/// # Errors
///
/// The socket cannot be reached or Herdr refused the request.
pub fn list_tabs(socket: &Path, workspace: &str) -> Result<Vec<String>, String> {
    let listed = request(socket, "tab.list", json!({"workspace_id": workspace}))?;
    Ok(listed["tabs"]
        .as_array()
        .ok_or("tab.list: the reply lists no tabs")?
        .iter()
        .filter(|entry| entry["workspace_id"] == json!(workspace))
        .filter_map(|entry| Some(entry["tab_id"].as_str()?.to_owned()))
        .collect())
}

/// Move `tab` to `index` through the Herdr socket.
///
/// # Errors
///
/// The socket cannot be reached or Herdr refused the request.
pub fn move_tab(socket: &Path, tab: &str, index: usize) -> Result<(), String> {
    request(
        socket,
        "tab.move",
        json!({"tab_id": tab, "insert_index": index}),
    )?;
    Ok(())
}

/// Move `tab` of `workspace` just after the coordinators already at its front, through the
/// Herdr socket at `socket`. Returns the index it moved to, or `None` when it was already in
/// place.
///
/// # Errors
///
/// The socket cannot be reached or Herdr refused a request; nothing was moved.
pub fn bring_forward(
    socket: &Path,
    workspace: &str,
    tab: &str,
    coordinators: &BTreeSet<String>,
) -> Result<Option<usize>, String> {
    let listed = request(socket, "tab.list", json!({"workspace_id": workspace}))?;
    // The reply lists the tabs in the order the workspace shows them, the order `insert_index`
    // counts in; a tab's `number` is its creation number, not its place.
    let order: Vec<String> = listed["tabs"]
        .as_array()
        .ok_or("tab.list: the reply lists no tabs")?
        .iter()
        .filter(|entry| entry["workspace_id"] == json!(workspace))
        .filter_map(|entry| Some(entry["tab_id"].as_str()?.to_owned()))
        .collect();
    let Some(index) = target_index(&order, tab, coordinators) else {
        return Ok(None);
    };
    request(
        socket,
        "tab.move",
        json!({"tab_id": tab, "insert_index": index}),
    )?;
    Ok(Some(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn only_a_compiled_program_counts_as_a_herdr_outside_a_pane() {
        let directory =
            std::env::temp_dir().join(format!("agentctl-tab-order-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("herdr");
        std::fs::write(&script, "#!/bin/sh\necho fake\n").unwrap();
        assert!(!is_compiled(&script));
        assert!(is_compiled(&std::env::current_exe().unwrap()));
        assert_eq!(resolve(&script), Some(script.clone()));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn canonical_tabs_put_front_then_lanes_by_name_then_unmanaged() {
        let entry = |tab: &str, name: &str, role: u8| (tab.to_owned(), name.to_owned(), role);
        let tabs = vec![
            entry("t3", "shell", ROLE_UNMANAGED),
            entry("t2", "worker-b", ROLE_LANE),
            entry("t1", "coord", ROLE_FRONT),
            entry("t4", "worker-a", ROLE_LANE),
        ];
        let want = vec![
            "t1".to_owned(),
            "t4".to_owned(),
            "t2".to_owned(),
            "t3".to_owned(),
        ];
        assert_eq!(canonical_tabs(&tabs), want);
        let ordered = canonical_tabs(&tabs)
            .into_iter()
            .map(|tab| tabs.iter().find(|entry| entry.0 == tab).unwrap().clone())
            .collect::<Vec<_>>();
        assert_eq!(canonical_tabs(&ordered), want);
    }

    #[test]
    fn coordinator_profiles_are_named_coordinator_or_end_in_it() {
        assert!(is_coordinator_profile(Some("coordinator")));
        assert!(is_coordinator_profile(Some("muse-coordinator")));
        assert!(!is_coordinator_profile(Some("worker")));
        assert!(!is_coordinator_profile(Some("coordinator-helper")));
        assert!(!is_coordinator_profile(None));
    }

    #[test]
    fn a_tab_goes_just_after_the_coordinators_already_in_front() {
        let coordinators: BTreeSet<String> = tabs(&["c1", "c2"]).into_iter().collect();
        let order = tabs(&["c1", "w1", "c2", "me"]);
        assert_eq!(target_index(&order, "me", &coordinators), Some(1));
        assert_eq!(
            target_index(&tabs(&["w1", "me"]), "me", &BTreeSet::new()),
            Some(0)
        );
        // Already within or right after the leading run: no move.
        assert_eq!(
            target_index(&tabs(&["me", "w1"]), "me", &coordinators),
            None
        );
        assert_eq!(
            target_index(&tabs(&["c1", "c2", "me"]), "me", &coordinators),
            None
        );
        // Not in the workspace at all.
        assert_eq!(
            target_index(&tabs(&["c1", "w1"]), "me", &coordinators),
            None
        );
    }
}
