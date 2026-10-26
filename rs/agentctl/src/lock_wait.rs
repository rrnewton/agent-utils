//! Who holds an agent's lifecycle lock, and who is waiting for it (#556 chat-delivery-latency).
//!
//! Every managed send to an agent holds the agent's lifecycle lock from enqueue until the
//! delivery is confirmed, so senders to a busy agent wait for one another, and nothing on screen
//! said so: a chat request could wait half an hour behind other agents' messages while its
//! sender looked idle. This module makes that wait observable without adding work to an
//! uncontended send.
//!
//! - A sender that finds the lock taken writes a waiter record,
//!   `REGISTRY/.waiting/<agent>/<pid>-<n>.json` (`pid`, `starttime_ticks`, `priority`, `since`,
//!   `command`), and removes it once it holds the lock or gives up. A record whose process is
//!   gone is ignored by readers.
//! - The holder needs no record: Linux lists every `flock` holder in `/proc/locks`, which also
//!   finds holders started by older agentctl builds.
//!
//! [`contention`] reads both, and [`describe`] turns the result into one sentence for a status
//! line or an alert.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Longest command line kept in a waiter record or a report, in characters.
const COMMAND_CHARS: usize = 240;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// The lifecycle lock file of `agent` in `registry`.
#[must_use]
pub fn lock_path(registry: &Path, agent: &str) -> PathBuf {
    registry.join(format!(".{agent}.lock"))
}

/// The companion lock that ranks waiters for `agent`'s lifecycle lock: see
/// [`agent_queue::fsio::lock_exclusive_ranked`].
#[must_use]
pub fn rank_path(registry: &Path, agent: &str) -> PathBuf {
    registry.join(format!(".{agent}.rank.lock"))
}

fn waiting_directory(registry: &Path, agent: &str) -> PathBuf {
    registry.join(".waiting").join(agent)
}

fn bounded(text: &str) -> String {
    let mut kept: String = text.chars().take(COMMAND_CHARS).collect();
    if text.chars().count() > COMMAND_CHARS {
        kept.push_str("...");
    }
    kept
}

/// The command line of process `pid`, its arguments joined by spaces, or `None` when it cannot
/// be read (the process is gone, or belongs to another user).
fn command_line(pid: u64) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let words: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    (!words.is_empty()).then(|| bounded(&words.join(" ")))
}

/// A waiter record, removed when dropped.
#[derive(Debug)]
pub struct Waiting {
    path: PathBuf,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Record that this process is waiting for `agent`'s lifecycle lock, with `priority` when it
/// waits as a priority sender. Best effort: a record that cannot be written only makes the wait
/// less visible, so it never stops the send, and `None` is returned.
pub fn register(registry: &Path, agent: &str, priority: bool) -> Option<Waiting> {
    let directory = waiting_directory(registry, agent);
    crate::agent::create_private_directory(&directory, "lock waiter directory", true, true).ok()?;
    let pid = u64::from(std::process::id());
    let starttime_ticks = agent_queue::identity::read_process_stat(pid)
        .ok()?
        .starttime_ticks;
    let path = directory.join(format!(
        "{pid}-{}.json",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let record = json!({
        "pid": pid,
        "starttime_ticks": starttime_ticks,
        "priority": priority,
        "since": now(),
        "command": command_line(pid),
    });
    crate::agent::atomic_json(&path, &record).ok()?;
    Some(Waiting { path })
}

/// The live waiter records for `agent`, the longest waiting first.
fn waiters(registry: &Path, agent: &str, at: f64) -> Vec<Value> {
    let Ok(entries) = fs::read_dir(waiting_directory(registry, agent)) else {
        return Vec::new();
    };
    let mut found: Vec<(f64, Value)> = entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .filter_map(|entry| {
            let record: Value = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
            let pid = record.get("pid")?.as_u64()?;
            let ticks = record.get("starttime_ticks")?.as_u64()?;
            // A record left by a process that is gone, or whose PID was reused, is stale.
            let alive = agent_queue::identity::read_process_stat(pid)
                .is_ok_and(|stat| stat.starttime_ticks == ticks);
            if !alive {
                return None;
            }
            let since = record.get("since")?.as_f64()?;
            Some((
                since,
                json!({
                    "pid": pid,
                    "priority": record.get("priority").and_then(Value::as_bool).unwrap_or(false),
                    "waiting_seconds": (at - since).max(0.0).round(),
                    "command": record.get("command").cloned().unwrap_or(Value::Null),
                }),
            ))
        })
        .collect();
    found.sort_by(|left, right| left.0.total_cmp(&right.0));
    found.into_iter().map(|(_, waiter)| waiter).collect()
}

/// The processes holding a `flock` on a file with inode `inode`, from the text of
/// `/proc/locks`, each with the device numbers printed there. Waiting entries (`->`) are
/// skipped.
fn flock_holders(locks: &str, inode: u64) -> Vec<(u32, u32, u64)> {
    locks
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // "N: FLOCK ADVISORY WRITE PID MAJ:MIN:INODE START END"
            if fields.get(1) != Some(&"FLOCK") {
                return None;
            }
            let mut place = fields.get(5)?.split(':');
            let major = u32::from_str_radix(place.next()?, 16).ok()?;
            let minor = u32::from_str_radix(place.next()?, 16).ok()?;
            let file_inode: u64 = place.next()?.parse().ok()?;
            (file_inode == inode).then_some((major, minor, fields.get(4)?.parse().ok()?))
        })
        .collect()
}

/// Whether process `pid` has a file with device `device` and inode `inode` open.
fn has_open(pid: u64, device: u64, inode: u64) -> bool {
    fs::read_dir(format!("/proc/{pid}/fd")).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            fs::metadata(entry.path())
                .is_ok_and(|metadata| metadata.dev() == device && metadata.ino() == inode)
        })
    })
}

/// The holder of a `flock` on the file with device `device` and inode `inode`. `/proc/locks`
/// prints the filesystem's device, which on some filesystems (a btrfs subvolume, for one)
/// differs from the device `stat` reports, so a holder on another device is accepted only when
/// it has the file itself open.
fn flock_holder(locks: &str, device: u64, inode: u64) -> Option<u64> {
    let (major, minor) = (libc::major(device), libc::minor(device));
    let holders = flock_holders(locks, inode);
    holders
        .iter()
        .find(|(file_major, file_minor, _)| (*file_major, *file_minor) == (major, minor))
        .or_else(|| {
            holders
                .iter()
                .find(|(_, _, pid)| has_open(*pid, device, inode))
        })
        .map(|(_, _, pid)| *pid)
}

/// Who holds `agent`'s lifecycle lock in `registry` and who is waiting for it:
/// `{"holder": {"pid", "command"} | null, "waiters": [{"pid", "priority", "waiting_seconds",
/// "command"}]}`, waiters longest first. Reads only; never takes the lock.
#[must_use]
pub fn contention(registry: &Path, agent: &str) -> Value {
    let at = now();
    let holder = fs::metadata(lock_path(registry, agent))
        .ok()
        .and_then(|metadata| {
            let locks = fs::read_to_string("/proc/locks").ok()?;
            flock_holder(&locks, metadata.dev(), metadata.ino())
        })
        .map(|pid| json!({"pid": pid, "command": command_line(pid)}));
    json!({
        "holder": holder,
        "waiters": waiters(registry, agent, at),
    })
}

/// The longest any sender has been waiting, in seconds, in a [`contention`] report.
#[must_use]
pub fn longest_wait(report: &Value) -> f64 {
    report["waiters"]
        .as_array()
        .and_then(|waiters| waiters.first())
        .and_then(|waiter| waiter["waiting_seconds"].as_f64())
        .unwrap_or(0.0)
}

fn who(process: &Value) -> String {
    let pid = process["pid"].as_u64().unwrap_or_default();
    match process["command"].as_str() {
        Some(command) => format!("PID {pid}: {command}"),
        None => format!("PID {pid}"),
    }
}

/// One sentence for a [`contention`] report, or `None` when nobody is waiting.
#[must_use]
pub fn describe(agent: &str, report: &Value) -> Option<String> {
    let waiters = report["waiters"]
        .as_array()
        .filter(|list| !list.is_empty())?;
    let longest = &waiters[0];
    let minutes = (longest["waiting_seconds"].as_f64().unwrap_or(0.0) / 60.0).floor();
    let mut sentence = format!(
        "{} sender{} waiting for agent {agent}'s lifecycle lock, the longest for {minutes:.0} min ({}{})",
        waiters.len(),
        if waiters.len() == 1 { "" } else { "s" },
        who(longest),
        if longest["priority"].as_bool() == Some(true) {
            ", priority"
        } else {
            ""
        }
    );
    match report["holder"].as_object() {
        Some(_) => sentence.push_str(&format!("; held by {}", who(&report["holder"]))),
        None => sentence.push_str("; no holder found in /proc/locks"),
    }
    Some(sentence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flock_holders_are_found_by_inode_with_their_devices() {
        let locks = "1: FLOCK  ADVISORY  WRITE 4242 00:2b:991 0 EOF\n\
                     1: -> FLOCK  ADVISORY  WRITE 5151 00:2b:991 0 EOF\n\
                     2: POSIX  ADVISORY  WRITE 6262 00:2b:992 0 EOF\n\
                     3: FLOCK  ADVISORY  WRITE 7373 fd:01:993 0 EOF\n";
        assert_eq!(flock_holders(locks, 991), vec![(0, 0x2b, 4242)]);
        assert_eq!(flock_holders(locks, 993), vec![(0xfd, 1, 7373)]);
        assert_eq!(flock_holders(locks, 992), vec![]);
        assert_eq!(flock_holder(locks, libc::makedev(0, 0x2b), 991), Some(4242));
        // On another device, and not open by that process: not the holder.
        assert_eq!(flock_holder(locks, libc::makedev(0, 0x2c), 991), None);
    }

    #[test]
    fn a_waiter_is_reported_while_it_waits_and_the_holder_is_found() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-lock-wait-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        crate::agent::create_private_directory(&registry, "registry", true, true).unwrap();
        let lock =
            crate::agent::open_private_lock(&lock_path(&registry, "worker"), "lock").unwrap();
        fs2::FileExt::try_lock_exclusive(&lock).unwrap();

        assert_eq!(describe("worker", &contention(&registry, "worker")), None);
        let waiting = register(&registry, "worker", true).expect("waiter record");
        let report = contention(&registry, "worker");
        assert_eq!(report["holder"]["pid"], json!(std::process::id()));
        assert_eq!(report["waiters"][0]["pid"], json!(std::process::id()));
        assert_eq!(report["waiters"][0]["priority"], json!(true));
        let sentence = describe("worker", &report).unwrap();
        assert!(sentence.starts_with("1 sender waiting for agent worker's lifecycle lock"));
        assert!(sentence.contains(", priority"), "{sentence}");
        assert!(sentence.contains("; held by PID"), "{sentence}");

        drop(waiting);
        assert_eq!(contention(&registry, "worker")["waiters"], json!([]));
        let _ = fs::remove_dir_all(&registry);
    }

    #[test]
    fn a_record_left_by_a_dead_process_is_ignored() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-lock-wait-stale-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let directory = waiting_directory(&registry, "worker");
        crate::agent::create_private_directory(&directory, "waiters", true, true).unwrap();
        crate::agent::atomic_json(
            &directory.join("1-0.json"),
            &json!({"pid": u64::from(std::process::id()), "starttime_ticks": 1,
                    "priority": false, "since": 1.0, "command": "gone"}),
        )
        .unwrap();
        assert_eq!(contention(&registry, "worker")["waiters"], json!([]));
        let _ = fs::remove_dir_all(&registry);
    }
}
