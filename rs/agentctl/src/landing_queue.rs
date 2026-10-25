//! The landing queue (`agentctl hold`): which agent lands after which.
//!
//! A coordinator that serializes landings tells a ready child to hold until another child has
//! landed. `agentctl hold NAME --after OTHER` writes that down in `REGISTRY/landing-queue.json`,
//! and `agentctl watch-agents` reads it when OTHER finishes its turn with a verdict: the notice
//! that reports the finish also says NAME is next, and the hold is marked released by that turn.
//! A chain (B after A, C after B) advances one step per finish. The watcher still types only
//! into the coordinator; giving NAME the go stays the coordinator's decision.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::AgentError;

/// Schema of `landing-queue.json`.
pub const SCHEMA: &str = "agentctl-landing-queue/v1";
/// Seconds a released hold stays listed before a later write prunes it.
const RELEASED_KEEP_SECONDS: f64 = 24.0 * 3600.0;
/// Largest queue file read.
const MAX_BYTES: u64 = 1 << 20;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// One holder: `name` lands after `after` has finished.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Hold {
    /// The agent that waits.
    pub name: String,
    /// The agent whose finish releases it.
    pub after: String,
    /// Free text repeated in the release notice, such as what the holder is ready to land.
    #[serde(default)]
    pub note: String,
    /// Seconds since the Unix epoch when the hold was recorded.
    pub since: f64,
    /// The finished turn that released the hold (`AGENT:TURN`), once released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_by: Option<String>,
    /// Seconds since the Unix epoch when the hold was released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_at: Option<f64>,
}

/// The whole queue file.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LandingQueue {
    /// Always [`SCHEMA`].
    pub schema: String,
    /// Holds in the order they were recorded.
    pub holds: Vec<Hold>,
}

impl Default for LandingQueue {
    fn default() -> Self {
        Self {
            schema: SCHEMA.to_owned(),
            holds: Vec::new(),
        }
    }
}

/// The queue file of `registry`.
#[must_use]
pub fn path(registry: &Path) -> PathBuf {
    registry.join("landing-queue.json")
}

/// Whether `value` is a registered-agent name: 1-32 lowercase letters, digits, and hyphens,
/// starting with a letter.
#[must_use]
pub fn valid_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

/// Read the queue; a missing file is an empty queue.
pub fn load(path: &Path) -> Result<LandingQueue> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LandingQueue::default());
        }
        Err(error) => return Err(fail(format!("cannot read {}: {error}", path.display()))),
    };
    let mut text = String::new();
    file.take(MAX_BYTES)
        .read_to_string(&mut text)
        .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
    let queue: LandingQueue = serde_json::from_str(&text).map_err(|error| {
        fail(format!(
            "{} is not a landing queue: {error}",
            path.display()
        ))
    })?;
    if queue.schema != SCHEMA {
        return Err(fail(format!(
            "{} has schema {:?}, expected {SCHEMA}",
            path.display(),
            queue.schema
        )));
    }
    Ok(queue)
}

/// Change the queue under its lock and write it back atomically; returns what `change` returns.
/// Released holds older than a day are pruned on the way.
pub fn update<T>(
    path: &Path,
    now: f64,
    change: impl FnOnce(&mut LandingQueue) -> Result<T>,
) -> Result<T> {
    use fs2::FileExt;
    let directory = path
        .parent()
        .ok_or_else(|| fail("landing queue path has no directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| fail(format!("cannot create {}: {error}", directory.display())))?;
    let lock_path = directory.join("landing-queue.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| fail(format!("cannot open {}: {error}", lock_path.display())))?;
    lock.lock_exclusive()
        .map_err(|error| fail(format!("cannot lock {}: {error}", lock_path.display())))?;
    let mut queue = load(path)?;
    let result = change(&mut queue)?;
    queue.holds.retain(|hold| {
        hold.released_at
            .is_none_or(|at| now - at < RELEASED_KEEP_SECONDS)
    });
    let mut bytes = serde_json::to_vec_pretty(&queue).map_err(|error| fail(error.to_string()))?;
    bytes.push(b'\n');
    let temporary = directory.join(format!(".landing-queue.json.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    fs::rename(&temporary, path)
        .map_err(|error| fail(format!("cannot publish {}: {error}", path.display())))?;
    Ok(result)
}

/// Record that `name` lands after `after`, replacing any hold `name` already had.
pub fn hold(path: &Path, name: &str, after: &str, note: &str, now: f64) -> Result<LandingQueue> {
    for value in [name, after] {
        if !valid_name(value) {
            return Err(fail(format!(
                "{value:?} is not an agent name (1-32 lowercase letters, digits, and hyphens)"
            )));
        }
    }
    if name == after {
        return Err(fail(format!("{name} cannot hold for itself")));
    }
    update(path, now, |queue| {
        queue.holds.retain(|hold| hold.name != name);
        queue.holds.push(Hold {
            name: name.to_owned(),
            after: after.to_owned(),
            note: note.to_owned(),
            since: now,
            released_by: None,
            released_at: None,
        });
        Ok(queue.clone())
    })
}

/// Drop `name`'s hold, released or not; returns whether there was one, and the queue after.
pub fn drop_hold(path: &Path, name: &str, now: f64) -> Result<(bool, LandingQueue)> {
    update(path, now, |queue| {
        let before = queue.holds.len();
        queue.holds.retain(|hold| hold.name != name);
        Ok((queue.holds.len() != before, queue.clone()))
    })
}

/// The holds still waiting on `after`, unreleased, in the order they were recorded. Reads only.
pub fn waiting_on(path: &Path, after: &str) -> Result<Vec<Hold>> {
    Ok(load(path)?
        .holds
        .into_iter()
        .filter(|hold| hold.after == after && hold.released_by.is_none())
        .collect())
}

/// Mark every hold still waiting on `after` released by `by` (the notice that announced it)
/// and return them. A missing queue file is not created.
pub fn release_after(path: &Path, after: &str, by: &str, now: f64) -> Result<Vec<Hold>> {
    if waiting_on(path, after)?.is_empty() {
        return Ok(Vec::new());
    }
    update(path, now, |queue| {
        let mut released = Vec::new();
        for hold in &mut queue.holds {
            if hold.after == after && hold.released_by.is_none() {
                hold.released_by = Some(by.to_owned());
                hold.released_at = Some(now);
                released.push(hold.clone());
            }
        }
        Ok(released)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A queue path in a fresh scratch directory, removed again when the guard drops.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn queue_path(name: &str) -> (Scratch, PathBuf) {
        let directory = std::env::temp_dir().join(format!(
            "agentctl-landing-queue-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        let path = path(&directory);
        (Scratch(directory), path)
    }

    #[test]
    fn a_finish_releases_its_holders_once_and_a_chain_advances() {
        let (_directory, path) = queue_path("chain");
        hold(&path, "second", "first", "gate green", 10.0).unwrap();
        hold(&path, "third", "second", "", 20.0).unwrap();
        assert!(release_after(&path, "nobody", "nobody:1", 30.0)
            .unwrap()
            .is_empty());
        let waiting = waiting_on(&path, "first").unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].note, "gate green");
        assert_eq!(
            waiting_on(&path, "first").unwrap(),
            waiting,
            "reading releases nothing"
        );
        let released = release_after(&path, "first", "first:aa", 30.0).unwrap();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].name, "second");
        assert_eq!(released[0].released_by.as_deref(), Some("first:aa"));
        // A later finish of the same agent names nobody again.
        assert!(waiting_on(&path, "first").unwrap().is_empty());
        assert!(release_after(&path, "first", "first:bb", 50.0)
            .unwrap()
            .is_empty());
        // The chain moves on when the released holder finishes.
        let next = release_after(&path, "second", "second:cc", 60.0).unwrap();
        assert_eq!(next[0].name, "third");
    }

    #[test]
    fn a_new_hold_replaces_the_old_and_released_holds_are_pruned_after_a_day() {
        let (_directory, path) = queue_path("replace");
        hold(&path, "second", "first", "", 10.0).unwrap();
        let queue = hold(&path, "second", "other", "rebased", 20.0).unwrap();
        assert_eq!(queue.holds.len(), 1);
        assert_eq!(queue.holds[0].after, "other");
        release_after(&path, "other", "other:1", 30.0).unwrap();
        hold(&path, "fourth", "third", "", 31.0).unwrap();
        assert_eq!(load(&path).unwrap().holds.len(), 2);
        hold(&path, "fifth", "third", "", 30.0 + RELEASED_KEEP_SECONDS).unwrap();
        let names: Vec<String> = load(&path)
            .unwrap()
            .holds
            .into_iter()
            .map(|hold| hold.name)
            .collect();
        assert_eq!(names, ["fourth", "fifth"]);
        let (dropped, queue) = drop_hold(&path, "fourth", 40.0).unwrap();
        assert!(dropped);
        assert_eq!(queue.holds.len(), 1);
        assert!(!drop_hold(&path, "fourth", 41.0).unwrap().0);
    }

    #[test]
    fn bad_names_and_self_holds_are_refused_and_nothing_is_written() {
        let (_directory, path) = queue_path("refused");
        assert!(hold(&path, "Bad Name", "first", "", 1.0).is_err());
        assert!(hold(&path, "same", "same", "", 1.0).is_err());
        assert!(!path.exists());
        // Releasing from a queue that was never written creates nothing.
        assert!(release_after(&path, "first", "first:1", 2.0)
            .unwrap()
            .is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn a_foreign_file_is_refused_rather_than_overwritten() {
        let (directory, path) = queue_path("foreign");
        fs::create_dir_all(&directory.0).unwrap();
        fs::write(&path, "{\"schema\": \"other/v1\", \"holds\": []}").unwrap();
        assert!(hold(&path, "second", "first", "", 1.0).is_err());
        assert!(fs::read_to_string(&path).unwrap().contains("other/v1"));
    }
}
