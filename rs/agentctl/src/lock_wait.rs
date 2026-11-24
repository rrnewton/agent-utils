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
//!   `comm`, and the agentctl subcommand and target agent where derivable), and removes it once
//!   it holds the lock or gives up. A record whose process is gone is ignored by readers.
//! - The holder needs no record: Linux lists every `flock` holder in `/proc/locks`, which also
//!   finds holders started by older agentctl builds.
//!
//! Reports name a process by `pid`, `comm` and, for an agentctl process, its subcommand and
//! target agent (#2217 agentctl-lock-contention-no-argv) — never by its raw command line, which
//! can hold private paths or credentials passed as arguments. The argument vector is read only
//! to derive those two fields and is never itself reported.
//!
//! [`contention`] reads both, and [`describe`] turns the result into one sentence for a status
//! line or an alert.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Longest process name (`comm`) a report carries: the kernel's own limit
/// (`TASK_COMM_LEN` − 1 visible bytes).
const COMM_CHARS: usize = 15;

/// Subcommands whose first positional argument is the target agent's name. Only for these is
/// an agent name derived from a process's arguments (#2217); any other subcommand's arguments
/// stay unreported.
const AGENT_SUBCOMMANDS: &[&str] = &[
    "adopt",
    "approvals",
    "console",
    "drain",
    "handback",
    "rename",
    "revive",
    "send",
    "start",
    "status",
    "stop",
    "tail",
    "takeover",
];

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64())
}

/// Where `agent`'s locks are: beside its directory's layout (#593 registry-agents-subdir). A
/// name that cannot be located (one in both layouts) answers layout 2, whose lock no older build
/// takes; whoever uses that name will meet the conflict.
fn lock_location(registry: &Path, agent: &str) -> agent_queue::registry::Location {
    agent_queue::registry::locate(registry, agent)
        .unwrap_or(agent_queue::registry::Location::Current)
}

/// The lifecycle lock file of `agent` in `registry`: `locks/<agent>.lock`, or `.<agent>.lock` for
/// an agent still in layout 1.
#[must_use]
pub fn lock_path(registry: &Path, agent: &str) -> PathBuf {
    agent_queue::registry::lock_paths(registry, agent, lock_location(registry, agent)).0
}

/// The companion lock that ranks waiters for `agent`'s lifecycle lock: see
/// the queue library's ranked lock (`lock_exclusive_ranked`).
#[must_use]
pub fn rank_path(registry: &Path, agent: &str) -> PathBuf {
    agent_queue::registry::lock_paths(registry, agent, lock_location(registry, agent)).1
}

fn waiting_directory(registry: &Path, agent: &str) -> PathBuf {
    registry.join(".waiting").join(agent)
}

/// The name of process `pid` (`/proc/PID/comm`, at most [`COMM_CHARS`] bytes), or `None` when
/// it cannot be read (the process is gone, or belongs to another user).
fn comm_of_process(pid: u64) -> Option<String> {
    let raw = fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = raw.trim_end_matches(['\n', '\r']);
    (!name.is_empty()).then(|| name.chars().take(COMM_CHARS).collect())
}

/// The name of process `pid`, for a report, or `None` when it cannot be read.
#[must_use]
pub(crate) fn comm_of(pid: u64) -> Option<String> {
    comm_of_process(pid)
}

/// The argument vector of process `pid`, read only to derive the fields
/// [`agentctl_action`] returns; it is never itself put in a report (#2217).
fn arguments_of(pid: u64) -> Option<Vec<String>> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let words: Vec<String> = raw
        .split(|byte| *byte == 0)
        .filter(|word| !word.is_empty())
        .map(|word| String::from_utf8_lossy(word).into_owned())
        .collect();
    (!words.is_empty()).then_some(words)
}

/// The agentctl subcommand and target agent a process's arguments name, without exposing the
/// arguments themselves (#2217): `("send", Some("liaison"))` for `agentctl send liaison …`.
/// `None` when the process is not an agentctl process or names no subcommand. An agent is
/// derived only for [`AGENT_SUBCOMMANDS`], whose first positional argument is the agent's
/// name; global options before the subcommand (`--registry DIR`, `--flag=value`) are skipped,
/// and a would-be agent containing `/` or whitespace is not an agent name and is not derived.
fn agentctl_action(arguments: &[String]) -> Option<(String, Option<String>)> {
    let program = arguments.first()?;
    if Path::new(program).file_name()?.to_str()? != "agentctl" {
        return None;
    }
    let mut rest = arguments[1..].iter().map(String::as_str);
    let subcommand = loop {
        match rest.next()? {
            // The one global option that takes a separate value; every other leading
            // `--name` or `--name=value` is skipped as a single token.
            "--registry" => {
                rest.next()?;
            }
            token if token.starts_with('-') => {}
            token => break token,
        }
    };
    let agent = AGENT_SUBCOMMANDS
        .contains(&subcommand)
        .then(|| {
            rest.find(|token| !token.starts_with('-'))
                .filter(|token| !token.contains(['/', ' ', '\t']))
        })
        .flatten()
        .map(str::to_owned);
    Some((subcommand.to_owned(), agent))
}

/// How a report names process `pid`: `{"pid", "comm", "subcommand", "agent"}`, with `comm`
/// absent when the process cannot be read and `subcommand`/`agent` present only for an
/// agentctl process whose arguments name them. The raw command line never appears (#2217).
fn process_report(pid: u64) -> Value {
    report_for(pid, comm_of_process(pid), arguments_of(pid).as_deref())
}

/// [`process_report`] from its read pieces, so tests can plant arguments.
fn report_for(pid: u64, comm: Option<String>, arguments: Option<&[String]>) -> Value {
    let mut report = json!({"pid": pid});
    if let Some(comm) = comm {
        report["comm"] = json!(comm);
    }
    if let Some((subcommand, agent)) = arguments.and_then(agentctl_action) {
        report["subcommand"] = json!(subcommand);
        if let Some(agent) = agent {
            report["agent"] = json!(agent);
        }
    }
    report
}

/// A waiter record, removed when dropped.
#[derive(Debug)]
pub struct Waiting {
    path: PathBuf,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        // Leave no directories behind either: a caller that only waited (a
        // refused revive, a send that never got the lock) must not change the
        // registry tree. `remove_dir` only succeeds when the directory is
        // empty, so a concurrent waiter's record is never disturbed.
        if let Some(agent_directory) = self.path.parent() {
            let _ = fs::remove_dir(agent_directory);
            if let Some(waiting_root) = agent_directory.parent() {
                let _ = fs::remove_dir(waiting_root);
            }
        }
    }
}

/// Record that this process is waiting for `agent`'s lifecycle lock, with `priority` when it
/// waits as a priority sender. Best effort: a record that cannot be written only makes the wait
/// less visible, so it never stops the send, and `None` is returned.
pub fn register(registry: &Path, agent: &str, priority: bool) -> Option<Waiting> {
    let directory = waiting_directory(registry, agent);
    crate::agent::create_private_directory(&directory, "lock waiter directory", true, true).ok()?;
    // Records left by senders that died while waiting are removed here, by a writer; readers
    // only skip them.
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().ends_with(".json")
                && live_record(&entry.path()).is_none()
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    let pid = u64::from(std::process::id());
    let starttime_ticks = agent_queue::identity::read_process_stat(pid)
        .ok()??
        .starttime_ticks;
    let path = directory.join(format!(
        "{pid}-{}.json",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    // The record names this process the way a report does (#2217): `comm` plus the derived
    // subcommand and agent — never the raw command line other processes will read back.
    let mut record = json!({
        "pid": pid,
        "starttime_ticks": starttime_ticks,
        "priority": priority,
        "since": now(),
        "comm": comm_of_process(pid),
    });
    let own_arguments: Vec<String> = std::env::args().collect();
    if let Some((subcommand, agent)) = agentctl_action(&own_arguments) {
        record["subcommand"] = json!(subcommand);
        if let Some(agent) = agent {
            record["agent"] = json!(agent);
        }
    }
    crate::agent::atomic_json(&path, &record).ok()?;
    Some(Waiting { path })
}

/// The waiter record at `path`, when it can be read and its process is still the one that
/// wrote it: a record left by a process that is gone, or whose PID was reused, is stale.
fn live_record(path: &Path) -> Option<Value> {
    let record: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let pid = record.get("pid")?.as_u64()?;
    let ticks = record.get("starttime_ticks")?.as_u64()?;
    matches!(
        agent_queue::identity::read_process_stat(pid),
        Ok(Some(stat)) if stat.starttime_ticks == ticks
    )
    .then_some(record)
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
            let record = live_record(&entry.path())?;
            let pid = record.get("pid")?.as_u64()?;
            let since = record.get("since")?.as_f64()?;
            // The identity is re-derived from the live process, so a record written by an
            // older build (which stored the raw command line) never surfaces it (#2217).
            let mut waiter = process_report(pid);
            waiter["priority"] = json!(record
                .get("priority")
                .and_then(Value::as_bool)
                .unwrap_or(false));
            waiter["waiting_seconds"] = json!((at - since).max(0.0).round());
            Some((since, waiter))
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

/// The process holding a `flock` on the file at `path`, if any (reads `/proc/locks`).
#[must_use]
pub(crate) fn holder_of(path: &Path) -> Option<u64> {
    let metadata = fs::metadata(path).ok()?;
    let locks = fs::read_to_string("/proc/locks").ok()?;
    flock_holder(&locks, metadata.dev(), metadata.ino())
}

/// Who holds `agent`'s lifecycle lock in `registry` and who is waiting for it:
/// `{"holder": {"pid", "comm", "subcommand", "agent"} | null, "waiters": [{"pid", "comm",
/// "subcommand", "agent", "priority", "waiting_seconds"}]}`, waiters longest first. Each
/// process is named by pid, `comm` and, for an agentctl process, its subcommand and target
/// agent — never by its raw command line (#2217). Reads only; never takes the lock.
#[must_use]
pub fn contention(registry: &Path, agent: &str) -> Value {
    let at = now();
    let holder = fs::metadata(lock_path(registry, agent))
        .ok()
        .and_then(|metadata| {
            let locks = fs::read_to_string("/proc/locks").ok()?;
            flock_holder(&locks, metadata.dev(), metadata.ino())
        })
        .map(process_report);
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
    let mut name = format!("PID {pid}");
    if let Some(comm) = process["comm"].as_str() {
        name.push_str(&format!(" ({comm}"));
        if let Some(subcommand) = process["subcommand"].as_str() {
            name.push_str(&format!(" {subcommand}"));
            if let Some(agent) = process["agent"].as_str() {
                name.push_str(&format!(" {agent}"));
            }
        }
        name.push(')');
    }
    name
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
        agent_queue::registry::prepare(&registry).unwrap();
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
        // The wait leaves nothing in the registry tree, not even the
        // `.waiting/worker` directories it created to become visible.
        assert!(!registry.join(".waiting").exists());
        let _ = fs::remove_dir_all(&registry);
    }

    fn arguments(words: &[&str]) -> Vec<String> {
        words.iter().map(|word| (*word).to_owned()).collect()
    }

    #[test]
    fn an_agentctl_action_is_derived_without_its_arguments() {
        let sentinel = "SENTINEL-private-brief-path";
        assert_eq!(
            agentctl_action(&arguments(&[
                "/usr/local/bin/agentctl",
                "send",
                "liaison",
                "--file",
                sentinel,
            ])),
            Some(("send".to_owned(), Some("liaison".to_owned())))
        );
        // Global options before the subcommand are skipped, value included.
        assert_eq!(
            agentctl_action(&arguments(&[
                "agentctl",
                "--registry",
                "/some/registry",
                "console",
                "worker",
            ])),
            Some(("console".to_owned(), Some("worker".to_owned())))
        );
        // A subcommand that takes no agent names none, however it is invoked.
        assert_eq!(
            agentctl_action(&arguments(&[
                "agentctl", "chat", "run", "--token", sentinel
            ])),
            Some(("chat".to_owned(), None))
        );
        // Not an agentctl process: nothing is derived, whatever it runs.
        assert_eq!(agentctl_action(&arguments(&["sleep", "30"])), None);
        assert_eq!(agentctl_action(&arguments(&["agentctl"])), None);
    }

    #[test]
    fn a_process_report_names_pid_comm_and_action_never_the_command_line() {
        let sentinel = "SENTINEL-secret-argument";
        let report = report_for(
            4242,
            Some("agentctl".to_owned()),
            Some(&arguments(&[
                "/usr/local/bin/agentctl",
                "send",
                "liaison",
                "--file",
                sentinel,
            ])),
        );
        assert_eq!(report["pid"], json!(4242));
        assert_eq!(report["comm"], json!("agentctl"));
        assert_eq!(report["subcommand"], json!("send"));
        assert_eq!(report["agent"], json!("liaison"));
        let printed = report.to_string();
        assert!(!printed.contains(sentinel), "{printed}");
        assert!(!printed.contains("\"command\""), "{printed}");
        // The watch delivery alarm built from such a report is just as clean.
        let alarm = describe(
            "liaison",
            &json!({"holder": report.clone(), "waiters": [report]}),
        )
        .unwrap();
        assert!(!alarm.contains(sentinel), "{alarm}");
        assert!(
            alarm.contains("PID 4242 (agentctl send liaison)"),
            "{alarm}"
        );
    }

    #[test]
    fn a_legacy_waiter_records_command_line_is_never_reported() {
        let registry = std::env::temp_dir().join(format!(
            "agentctl-lock-wait-legacy-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let directory = waiting_directory(&registry, "worker");
        crate::agent::create_private_directory(&directory, "waiters", true, true).unwrap();
        let own_pid = u64::from(std::process::id());
        let ticks = agent_queue::identity::read_process_stat(own_pid)
            .unwrap()
            .unwrap()
            .starttime_ticks;
        let sentinel = "SENTINEL-legacy-brief";
        crate::agent::atomic_json(
            &directory.join(format!("{own_pid}-0.json")),
            &json!({"pid": own_pid, "starttime_ticks": ticks, "priority": false,
                    "since": now(), "command": format!("agentctl send worker {sentinel}")}),
        )
        .unwrap();
        let report = contention(&registry, "worker");
        assert_eq!(report["waiters"][0]["pid"], json!(own_pid));
        assert!(report["waiters"][0]["comm"].is_string(), "{report}");
        let printed = report.to_string();
        assert!(!printed.contains(sentinel), "{printed}");
        assert!(!printed.contains("\"command\""), "{printed}");
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
        // The next sender that has to wait removes the stale record.
        let _waiting = register(&registry, "worker", false).unwrap();
        assert!(!directory.join("1-0.json").exists());
        let _ = fs::remove_dir_all(&registry);
    }
}
