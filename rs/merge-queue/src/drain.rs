//! Draining the `serve` daemon before an upgrade (`merge-queue drain-for-upgrade`). Stopping the
//! daemon cancels the validations in flight, and the daemon that replaces it starts them again
//! from scratch. A drain lets the running daemon finish the validations already running and act
//! on their verdicts (a green batch lands), start no new validation (no new batch, no culprit
//! search step, no rerun), and exit 0 once no run is in progress. The changes it did not decide
//! stay queued for the next daemon.
//!
//! The request is one file, `<state>/serve.drain.json`, naming the daemon's pid. A daemon only
//! honours a request that names it, and removes any other when it starts, so a request left
//! behind by a daemon that died never drains the daemon that replaces it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::now;
use crate::state::{write_json, State};

/// [`Request::schema`].
pub const DRAIN_SCHEMA: &str = "merge-queue-drain/v1";

/// How a run that a drain stopped says so (the start of its `stopped` reason).
pub const DRAINED: &str = "drained for upgrade";

/// A drain request.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    /// [`DRAIN_SCHEMA`].
    pub schema: String,
    /// The daemon asked to drain.
    pub pid: u32,
    /// RFC 3339 UTC time of the request.
    pub requested_at: String,
    /// Why, as given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl Request {
    /// `requested at <time>` and the reason, for a log line.
    pub fn describe(&self) -> String {
        if self.reason.is_empty() {
            format!("requested at {}", self.requested_at)
        } else {
            format!("requested at {}: {}", self.requested_at, self.reason)
        }
    }
}

fn path(state: &State) -> PathBuf {
    state.root.join("serve.drain.json")
}

/// Ask the daemon `pid` to drain.
pub fn request(state: &State, pid: u32, reason: &str) -> Result<Request, String> {
    let request = Request {
        schema: DRAIN_SCHEMA.to_owned(),
        pid,
        requested_at: now(),
        reason: reason.to_owned(),
    };
    write_json(&path(state), &request)?;
    Ok(request)
}

/// The request on file, whichever daemon it names (an unreadable one counts as none).
pub fn read(state: &State) -> Option<Request> {
    let text = std::fs::read_to_string(path(state)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The request asking the daemon `pid` to drain, if there is one.
pub fn requested_for(state: &State, pid: u32) -> Option<Request> {
    if !path(state).exists() {
        return None;
    }
    read(state).filter(|request| request.pid == pid)
}

/// Remove the request, whichever daemon it names; returns whether there was one.
pub fn clear(state: &State) -> Result<bool, String> {
    match std::fs::remove_file(path(state)) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("cannot remove {}: {error}", path(state).display())),
    }
}
