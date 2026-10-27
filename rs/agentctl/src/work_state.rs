//! What a "working" agent's processes are doing (#570 agent-progress-monitoring).
//!
//! Herdr's `working` says only that the harness is inside a turn. A turn can be the model
//! streaming, a tool that is burning CPU, a tool that has waited forty minutes on a lock another
//! process holds, or a turn that has stalled with nothing running at all. This module looks
//! under the agent's pinned harness process in `/proc`: it finds the tool commands the harness
//! started (the shells it runs commands in) and the gate steps a step runner among them is
//! running (from the evidence journal it keeps open, or else its child processes), and measures
//! each one's CPU time and I/O. Between two watcher passes a tool that used no CPU and moved no
//! bytes is not progressing, and the kernel says why: the wait channel and open files of its
//! processes name a file lock and its holder (`/proc/locks`), terminal input, a network socket, a
//! pipe, or a child that is itself idle.
//!
//! Everything is read through [`ProcView`], so tests describe fake process trees instead of
//! starting real ones. Only command lines, wait channels, file-descriptor targets and counters
//! are read, never a process's environment.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Characters kept of a command line quoted in a notice.
const COMMAND_CHARS: usize = 80;
/// Bytes read from the end of a gate's evidence journal.
const JOURNAL_TAIL_BYTES: u64 = 8 << 20;
/// Recorded runs of a step needed before its p95 is trusted.
pub const MIN_HISTORY_SAMPLES: usize = 5;

/// One process as `/proc/PID/stat` describes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcInfo {
    /// Process ID.
    pub pid: u32,
    /// Parent process ID.
    pub ppid: u32,
    /// Scheduler state letter (`R`, `S`, `D`, `Z`, ...).
    pub state: char,
    /// User plus system CPU time, in clock ticks.
    pub cpu_ticks: u64,
    /// Start time after boot, in clock ticks: with the PID, a process generation.
    pub start_ticks: u64,
}

/// One open file descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FdTarget {
    /// Descriptor number.
    pub fd: u32,
    /// Link target, such as `/dev/pts/3`, `socket:[123]`, `pipe:[456]`, or a path.
    pub target: String,
    /// The file's identity as `/proc/locks` spells it (`MAJ:MIN:INODE`, hexadecimal device).
    pub lock_id: Option<String>,
}

/// Where process facts come from; the production view reads `/proc`.
pub trait ProcView {
    /// Every visible process.
    fn processes(&self) -> Vec<ProcInfo>;
    /// Bytes read plus bytes written (`rchar + wchar`), when readable.
    fn io_bytes(&self, pid: u32) -> Option<u64>;
    /// The kernel function the process sleeps in, when readable and not `0`.
    fn wchan(&self, pid: u32) -> Option<String>;
    /// The command line, arguments joined by spaces.
    fn cmdline(&self, pid: u32) -> Option<String>;
    /// Open file descriptors.
    fn fds(&self, pid: u32) -> Vec<FdTarget>;
    /// The text of `/proc/locks`.
    fn locks(&self) -> String;
    /// At most the last `limit` bytes of a file.
    fn read_tail(&self, path: &Path, limit: u64) -> Option<String>;
    /// Unix seconds of a process start time given in clock ticks after boot.
    fn started_at(&self, _start_ticks: u64) -> Option<f64> {
        None
    }
}

/// The production [`ProcView`].
pub struct LinuxProc;

/// [`LinuxProc`] for one pass over many agents (#571 watch-agents-scale): the process table
/// and `/proc/locks` are read once, when first asked for, and every agent's sample shares them;
/// per-process facts are still read when asked for.
#[derive(Default)]
pub struct PassProc {
    processes: std::sync::OnceLock<Vec<ProcInfo>>,
    locks: std::sync::OnceLock<String>,
}

impl ProcView for PassProc {
    fn processes(&self) -> Vec<ProcInfo> {
        self.processes.get_or_init(|| LinuxProc.processes()).clone()
    }
    fn io_bytes(&self, pid: u32) -> Option<u64> {
        LinuxProc.io_bytes(pid)
    }
    fn wchan(&self, pid: u32) -> Option<String> {
        LinuxProc.wchan(pid)
    }
    fn cmdline(&self, pid: u32) -> Option<String> {
        LinuxProc.cmdline(pid)
    }
    fn fds(&self, pid: u32) -> Vec<FdTarget> {
        LinuxProc.fds(pid)
    }
    fn locks(&self) -> String {
        self.locks.get_or_init(|| LinuxProc.locks()).clone()
    }
    fn read_tail(&self, path: &Path, limit: u64) -> Option<String> {
        LinuxProc.read_tail(path, limit)
    }
    fn started_at(&self, start_ticks: u64) -> Option<f64> {
        LinuxProc.started_at(start_ticks)
    }
}

fn parse_stat(pid: u32, text: &str) -> Option<ProcInfo> {
    let close = text.rfind(')')?;
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let number = |index: usize| fields.get(index)?.parse::<u64>().ok();
    Some(ProcInfo {
        pid,
        state: fields.first()?.chars().next()?,
        ppid: u32::try_from(number(1)?).ok()?,
        cpu_ticks: number(11)?.saturating_add(number(12)?),
        start_ticks: number(19)?,
    })
}

/// A device and inode as `/proc/locks` writes them.
fn lock_id(dev: u64, ino: u64) -> String {
    format!("{:02x}:{:02x}:{ino}", libc::major(dev), libc::minor(dev))
}

impl ProcView for LinuxProc {
    fn processes(&self) -> Vec<ProcInfo> {
        let Ok(entries) = fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
                let text = fs::read_to_string(entry.path().join("stat")).ok()?;
                parse_stat(pid, &text)
            })
            .collect()
    }

    fn io_bytes(&self, pid: u32) -> Option<u64> {
        let text = fs::read_to_string(format!("/proc/{pid}/io")).ok()?;
        let field = |name: &str| {
            text.lines()
                .find_map(|line| line.strip_prefix(name)?.trim().parse::<u64>().ok())
        };
        Some(field("rchar:")?.saturating_add(field("wchar:")?))
    }

    fn wchan(&self, pid: u32) -> Option<String> {
        let text = fs::read_to_string(format!("/proc/{pid}/wchan")).ok()?;
        let text = text.trim();
        (!text.is_empty() && text != "0").then(|| text.to_owned())
    }

    fn cmdline(&self, pid: u32) -> Option<String> {
        let bytes = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let joined = text.trim_end_matches('\0').replace('\0', " ");
        (!joined.is_empty()).then_some(joined)
    }

    fn fds(&self, pid: u32) -> Vec<FdTarget> {
        let directory = PathBuf::from(format!("/proc/{pid}/fd"));
        let Ok(entries) = fs::read_dir(&directory) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let fd: u32 = entry.file_name().to_str()?.parse().ok()?;
                let target = fs::read_link(entry.path()).ok()?;
                let lock = fs::metadata(entry.path())
                    .ok()
                    .filter(std::fs::Metadata::is_file)
                    .map(|meta| lock_id(meta.dev(), meta.ino()));
                Some(FdTarget {
                    fd,
                    target: target.to_string_lossy().into_owned(),
                    lock_id: lock,
                })
            })
            .collect()
    }

    fn locks(&self) -> String {
        fs::read_to_string("/proc/locks").unwrap_or_default()
    }

    fn started_at(&self, start_ticks: u64) -> Option<f64> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let boot: f64 = stat
            .lines()
            .find_map(|line| line.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()?;
        // SAFETY: sysconf has no preconditions.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        (ticks > 0).then(|| boot + start_ticks as f64 / ticks as f64)
    }

    fn read_tail(&self, path: &Path, limit: u64) -> Option<String> {
        let mut file = fs::File::open(path).ok()?;
        let length = file.metadata().ok()?.len();
        if length > limit {
            file.seek(SeekFrom::Start(length - limit)).ok()?;
        }
        let mut bytes = Vec::new();
        file.take(limit).read_to_end(&mut bytes).ok()?;
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// One thing the agent is running: a tool command it started, or a gate step under it.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkItem {
    /// Stable identity across passes: `tool:PID:START` or `step:NAME:PID`.
    pub key: String,
    /// The gate step's name, for a step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    /// The item's root process, when it is alive.
    pub root: Option<u32>,
    /// The root's command line, bounded.
    pub command: String,
    /// Live processes in the item's tree.
    pub processes: usize,
    /// CPU ticks summed over the tree.
    pub cpu_ticks: u64,
    /// Bytes read and written, summed over the tree.
    pub io_bytes: u64,
    /// Unix seconds the step started, from the gate's journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<f64>,
    /// What the tree waits on, when it sleeps: one clause naming the process and the cause.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waits_on: Option<String>,
}

/// What one pass saw under the harness.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct WorkSample {
    /// Tool commands and gate steps, in a stable order.
    pub items: Vec<WorkItem>,
}

impl WorkSample {
    /// The tool commands (not gate steps).
    pub fn tools(&self) -> impl Iterator<Item = &WorkItem> {
        self.items.iter().filter(|item| item.step.is_none())
    }

    /// The running gate steps.
    pub fn steps(&self) -> impl Iterator<Item = &WorkItem> {
        self.items.iter().filter(|item| item.step.is_some())
    }
}

fn bounded(text: &str) -> String {
    let text: String = text.chars().take(COMMAND_CHARS).collect();
    text.trim().to_owned()
}

/// Whether a command line is a shell running a command string: how Codex, Claude Code and Muse
/// run a tool command (`bash -lc '...'`, `/bin/bash -c ...`, `sh -c ...`).
fn tool_shell(command: &str) -> bool {
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return false;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    matches!(name, "sh" | "bash" | "zsh" | "dash")
        && words
            .take(4)
            .any(|word| word.starts_with('-') && !word.starts_with("--") && word[1..].contains('c'))
}

struct Table {
    by_pid: BTreeMap<u32, ProcInfo>,
    children: BTreeMap<u32, Vec<u32>>,
}

impl Table {
    fn new(processes: Vec<ProcInfo>) -> Self {
        let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        for process in &processes {
            children.entry(process.ppid).or_default().push(process.pid);
        }
        Self {
            by_pid: processes.into_iter().map(|row| (row.pid, row)).collect(),
            children,
        }
    }

    fn subtree(&self, root: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            if !self.by_pid.contains_key(&pid) || out.contains(&pid) {
                continue;
            }
            out.push(pid);
            if let Some(children) = self.children.get(&pid) {
                stack.extend(children.iter().rev());
            }
        }
        out
    }
}

/// One `/proc/locks` row: its index, whether it waits (`->`), the owner PID, and the file.
fn lock_rows(text: &str) -> Vec<(String, bool, u32, String)> {
    text.lines()
        .filter_map(|line| {
            let (index, rest) = line.split_once(':')?;
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            let waiting = words.first() == Some(&"->");
            if waiting {
                words.remove(0);
            }
            // CLASS MODE ACCESS PID MAJ:MIN:INODE START END
            let pid = words.get(3)?.parse().ok()?;
            Some((
                index.trim().to_owned(),
                waiting,
                pid,
                (*words.get(4)?).to_owned(),
            ))
        })
        .collect()
}

/// Why the processes in `pids` sleep, as one clause, or `None` when one of them runs.
fn waits_on(view: &dyn ProcView, table: &Table, pids: &[u32], locks: &str) -> Option<String> {
    if pids.is_empty() {
        return Some("has no live process".to_owned());
    }
    if pids
        .iter()
        .any(|pid| table.by_pid.get(pid).is_some_and(|row| row.state == 'R'))
    {
        return None;
    }
    let named = |pid: u32| {
        format!(
            "`{}` (PID {pid})",
            view.cmdline(pid)
                .map_or_else(|| "?".to_owned(), |text| bounded(&text))
        )
    };
    let rows = lock_rows(locks);
    // Lower rank wins: the most specific cause anywhere in the tree explains it.
    let mut best: Option<(u8, String)> = None;
    let mut offer = |rank: u8, text: String| {
        if best.as_ref().is_none_or(|(old, _)| rank < *old) {
            best = Some((rank, text));
        }
    };
    for &pid in pids {
        let state = table.by_pid.get(&pid).map_or('?', |row| row.state);
        if let Some((index, _, _, file)) = rows
            .iter()
            .find(|(_, waiting, owner, _)| *waiting && *owner == pid)
        {
            let holder = rows
                .iter()
                .find(|(other, waiting, _, other_file)| {
                    other == index && !waiting && other_file == file
                })
                .map(|(_, _, holder, _)| *holder);
            let path = view
                .fds(pid)
                .into_iter()
                .find(|fd| fd.lock_id.as_deref() == Some(file.as_str()))
                .map_or_else(|| file.clone(), |fd| fd.target);
            offer(
                0,
                match holder {
                    Some(holder) => format!(
                        "{} waits on a file lock on {path} held by {}",
                        named(pid),
                        named(holder)
                    ),
                    None => format!("{} waits on a file lock on {path}", named(pid)),
                },
            );
            continue;
        }
        let wchan = view.wchan(pid).unwrap_or_default();
        let fds = || view.fds(pid);
        if state == 'Z' {
            offer(8, format!("{} exited and was not reaped", named(pid)));
        } else if wchan.contains("lock_inode_wait") || wchan.contains("flock") {
            offer(1, format!("{} waits on a file lock", named(pid)));
        } else if wchan.contains("tty_read") || wchan.contains("n_tty") {
            let tty = fds()
                .into_iter()
                .find(|fd| fd.fd == 0)
                .map_or_else(|| "its terminal".to_owned(), |fd| fd.target);
            offer(2, format!("{} waits for input on {tty}", named(pid)));
        } else if [
            "sk_wait_data",
            "inet_csk_accept",
            "tcp_recvmsg",
            "wait_for_connect",
            "unix_stream_read",
            "sk_stream_wait",
        ]
        .iter()
        .any(|name| wchan.contains(name))
        {
            offer(3, format!("{} waits on a network socket", named(pid)));
        } else if ["ep_poll", "do_sys_poll", "do_select", "poll_schedule"]
            .iter()
            .any(|name| wchan.contains(name))
        {
            let sockets = fds()
                .iter()
                .filter(|fd| fd.target.starts_with("socket:"))
                .count();
            if sockets > 0 {
                offer(
                    3,
                    format!("{} waits on a network socket ({sockets} open)", named(pid)),
                );
            } else {
                offer(6, format!("{} is polling for an event", named(pid)));
            }
        } else if wchan.contains("pipe_read")
            || wchan.contains("pipe_write")
            || wchan.contains("pipe_wait")
        {
            offer(4, format!("{} waits on a pipe", named(pid)));
        } else if state == 'D' {
            offer(
                5,
                format!(
                    "{} is in uninterruptible I/O ({})",
                    named(pid),
                    if wchan.is_empty() { "?" } else { &wchan }
                ),
            );
        } else if wchan.contains("futex") {
            offer(7, format!("{} waits on a futex inside itself", named(pid)));
        } else if wchan.contains("nanosleep") || wchan.contains("clock_nanosleep") {
            offer(7, format!("{} is sleeping", named(pid)));
        } else if wchan.contains("do_wait") || wchan.contains("wait4") {
            // A parent waiting on its child explains nothing; the child does.
            offer(9, format!("{} waits for a child process", named(pid)));
        } else {
            offer(
                8,
                format!(
                    "{} sleeps in {}",
                    named(pid),
                    if wchan.is_empty() {
                        "the kernel"
                    } else {
                        &wchan
                    }
                ),
            );
        }
    }
    best.map(|(_, text)| text)
}

/// The steps a gate's journal says started and have not ended: name, PID, start, command.
fn running_steps(journal: &str) -> Vec<(String, Option<u32>, Option<f64>, String)> {
    let mut running: BTreeMap<String, (Option<u32>, Option<f64>, String)> = BTreeMap::new();
    for line in journal.lines() {
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(step) = record["step"].as_str() else {
            continue;
        };
        match record["event"].as_str() {
            Some("step_start") => {
                let number = |key: &str| {
                    record[key]
                        .as_str()
                        .and_then(|text| text.parse::<f64>().ok())
                        .or_else(|| record[key].as_f64())
                };
                running.insert(
                    step.to_owned(),
                    (
                        number("pid").map(|pid| pid as u32),
                        number("ts"),
                        record["cmd"].as_str().unwrap_or_default().to_owned(),
                    ),
                );
            }
            Some("step_end") => {
                running.remove(step);
            }
            _ => {}
        }
    }
    running
        .into_iter()
        .map(|(step, (pid, started, command))| (step, pid, started, command))
        .collect()
}

/// Sample what runs under the harness process `harness` (whose start time must be
/// `harness_start` when given, so a reused PID is not read): every tool command and every
/// running gate step. `None` when the harness is not alive.
#[must_use]
pub fn sample(view: &dyn ProcView, harness: u32, harness_start: Option<u64>) -> Option<WorkSample> {
    let table = Table::new(view.processes());
    let root = table.by_pid.get(&harness)?;
    if harness_start.is_some_and(|start| start != root.start_ticks) {
        return None;
    }
    // Tool roots: the first shells running a command string on each path down from the harness.
    let mut roots = Vec::new();
    let mut stack: Vec<u32> = table.children.get(&harness).cloned().unwrap_or_default();
    while let Some(pid) = stack.pop() {
        if view
            .cmdline(pid)
            .is_some_and(|command| tool_shell(&command))
        {
            roots.push(pid);
        } else if let Some(children) = table.children.get(&pid) {
            stack.extend(children);
        }
    }
    roots.sort_unstable();
    let locks = view.locks();
    let measure = |pids: &[u32]| {
        let cpu = pids
            .iter()
            .filter_map(|pid| table.by_pid.get(pid))
            .map(|row| row.cpu_ticks)
            .sum::<u64>();
        let io = pids
            .iter()
            .filter_map(|pid| view.io_bytes(*pid))
            .sum::<u64>();
        (cpu, io)
    };
    let mut items = Vec::new();
    let mut journals = BTreeSet::new();
    let mut runners = Vec::new();
    for root in roots {
        let pids = table.subtree(root);
        let (cpu_ticks, io_bytes) = measure(&pids);
        for pid in &pids {
            let mut journal = false;
            for fd in view.fds(*pid) {
                if fd.target.ends_with("/journal.jsonl") {
                    journals.insert(fd.target);
                    journal = true;
                }
            }
            // A step runner that keeps no journal (no evidence directory configured) still runs
            // each step as a child of its own.
            if !journal
                && view
                    .cmdline(*pid)
                    .is_some_and(|command| step_runner(&command))
            {
                runners.push(*pid);
            }
        }
        items.push(WorkItem {
            key: format!("tool:{root}:{}", table.by_pid[&root].start_ticks),
            step: None,
            root: Some(root),
            command: view
                .cmdline(root)
                .map_or_else(String::new, |command| bounded(&command)),
            processes: pids.len(),
            cpu_ticks,
            io_bytes,
            started_at: None,
            waits_on: waits_on(view, &table, &pids, &locks),
        });
    }
    for journal in journals {
        let Some(text) = view.read_tail(Path::new(&journal), JOURNAL_TAIL_BYTES) else {
            continue;
        };
        for (step, pid, started_at, command) in running_steps(&text) {
            let pids = pid.map(|pid| table.subtree(pid)).unwrap_or_default();
            let (cpu_ticks, io_bytes) = measure(&pids);
            items.push(WorkItem {
                key: format!("step:{step}:{}", pid.unwrap_or(0)),
                step: Some(step),
                root: pid.filter(|pid| table.by_pid.contains_key(pid)),
                command: bounded(&command),
                processes: pids.len(),
                cpu_ticks,
                io_bytes,
                started_at,
                waits_on: waits_on(view, &table, &pids, &locks),
            });
        }
    }
    for runner in runners {
        for &child in table.children.get(&runner).into_iter().flatten() {
            let pids = table.subtree(child);
            let (cpu_ticks, io_bytes) = measure(&pids);
            let start = table.by_pid[&child].start_ticks;
            let command = view
                .cmdline(child)
                .map_or_else(String::new, |command| bounded(&command));
            items.push(WorkItem {
                key: format!("step:{child}:{start}"),
                step: Some(format!("`{command}` (PID {child})")),
                root: Some(child),
                command,
                processes: pids.len(),
                cpu_ticks,
                io_bytes,
                started_at: view.started_at(start),
                waits_on: waits_on(view, &table, &pids, &locks),
            });
        }
    }
    Some(WorkSample { items })
}

/// Whether a command line runs the suite's step runner, whose steps are its children.
fn step_runner(command: &str) -> bool {
    command
        .split_whitespace()
        .next()
        .and_then(|program| program.rsplit('/').next())
        .is_some_and(|name| name == "dagrun")
}

/// The p95 duration of each gate step in a recorded timing history (#27
/// cross-runner-test-timing): the step runner's per-step profile CSV, `step` and `elapsed_s`
/// columns.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepHistory {
    durations: BTreeMap<String, Vec<f64>>,
}

/// Split one CSV row, honouring double quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                field.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            _ => field.push(character),
        }
    }
    fields.push(field);
    fields
}

impl StepHistory {
    /// Add every row of one profile CSV; a file without `step` and `elapsed_s` adds nothing.
    pub fn add_csv(&mut self, text: &str) {
        let mut lines = text.lines();
        let Some(header) = lines.next() else {
            return;
        };
        let header = csv_fields(header);
        let column = |name: &str| header.iter().position(|field| field == name);
        let (Some(step), Some(elapsed)) = (column("step"), column("elapsed_s")) else {
            return;
        };
        for line in lines {
            let fields = csv_fields(line);
            let (Some(name), Some(seconds)) = (fields.get(step), fields.get(elapsed)) else {
                continue;
            };
            if let Ok(seconds) = seconds.parse::<f64>() {
                if seconds.is_finite() && seconds >= 0.0 && !name.is_empty() {
                    self.durations
                        .entry(name.clone())
                        .or_default()
                        .push(seconds);
                }
            }
        }
    }

    /// Load each file in `paths`; one that cannot be read is skipped and reported on stderr.
    #[must_use]
    pub fn load(paths: &[PathBuf]) -> Self {
        let mut history = Self::default();
        for path in paths {
            match fs::read_to_string(path) {
                Ok(text) => history.add_csv(&text),
                Err(error) => eprintln!(
                    "agentctl watch-agents: step history {}: {error}",
                    path.display()
                ),
            }
        }
        history
    }

    /// The nearest-rank p95 of `step` and how many runs it rests on, once there are
    /// [`MIN_HISTORY_SAMPLES`] of them.
    #[must_use]
    pub fn p95(&self, step: &str) -> Option<(f64, usize)> {
        let mut samples = self.durations.get(step)?.clone();
        if samples.len() < MIN_HISTORY_SAMPLES {
            return None;
        }
        samples.sort_by(f64::total_cmp);
        let rank = (samples.len() * 95).div_ceil(100).max(1);
        Some((samples[rank - 1], samples.len()))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A fake `/proc` for tests: processes, wait channels, descriptors, locks and files.
    use super::*;

    #[derive(Clone, Debug, Default)]
    pub struct FakeProc {
        pub processes: Vec<ProcInfo>,
        pub io: BTreeMap<u32, u64>,
        pub wchan: BTreeMap<u32, String>,
        pub cmdline: BTreeMap<u32, String>,
        pub fds: BTreeMap<u32, Vec<FdTarget>>,
        pub locks: String,
        pub files: BTreeMap<String, String>,
    }

    impl FakeProc {
        /// Add a process sleeping (`S`) or in state `state`.
        pub fn spawn(&mut self, pid: u32, ppid: u32, state: char, command: &str) -> &mut Self {
            self.processes.push(ProcInfo {
                pid,
                ppid,
                state,
                cpu_ticks: 0,
                start_ticks: u64::from(pid) * 10,
            });
            self.cmdline.insert(pid, command.to_owned());
            self
        }

        pub fn burn(&mut self, pid: u32, ticks: u64) -> &mut Self {
            if let Some(row) = self.processes.iter_mut().find(|row| row.pid == pid) {
                row.cpu_ticks += ticks;
            }
            self
        }

        pub fn sleep_in(&mut self, pid: u32, wchan: &str) -> &mut Self {
            self.wchan.insert(pid, wchan.to_owned());
            self
        }

        pub fn open(&mut self, pid: u32, fd: u32, target: &str, lock: Option<&str>) -> &mut Self {
            self.fds.entry(pid).or_default().push(FdTarget {
                fd,
                target: target.to_owned(),
                lock_id: lock.map(str::to_owned),
            });
            self
        }
    }

    impl ProcView for FakeProc {
        fn processes(&self) -> Vec<ProcInfo> {
            self.processes.clone()
        }
        fn io_bytes(&self, pid: u32) -> Option<u64> {
            Some(self.io.get(&pid).copied().unwrap_or(0))
        }
        fn wchan(&self, pid: u32) -> Option<String> {
            self.wchan.get(&pid).cloned()
        }
        fn cmdline(&self, pid: u32) -> Option<String> {
            self.cmdline.get(&pid).cloned()
        }
        fn fds(&self, pid: u32) -> Vec<FdTarget> {
            self.fds.get(&pid).cloned().unwrap_or_default()
        }
        fn locks(&self) -> String {
            self.locks.clone()
        }
        fn read_tail(&self, path: &Path, _limit: u64) -> Option<String> {
            self.files.get(path.to_str()?).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeProc;
    use super::*;

    /// A harness (PID 100) with an MCP server child (not a tool) and one tool shell (200).
    fn harness() -> FakeProc {
        let mut proc = FakeProc::default();
        proc.spawn(100, 1, 'S', "claude --model opus")
            .spawn(150, 100, 'S', "node /opt/mcp/server.js")
            .spawn(200, 100, 'S', "/bin/bash -c cargo build --release");
        proc
    }

    #[test]
    fn stat_lines_parse_even_with_parentheses_in_the_name() {
        let line = "4242 (tool (x)) S 1 4242 4242 0 -1 4194304 150 0 0 0 7 3 0 0 20 0 1 0 555 0";
        let info = parse_stat(4242, line).unwrap();
        assert_eq!(
            (info.state, info.ppid, info.cpu_ticks, info.start_ticks),
            ('S', 1, 10, 555)
        );
    }

    #[test]
    fn tool_shells_are_found_and_other_children_are_not() {
        let mut proc = harness();
        proc.spawn(201, 200, 'R', "cargo build --release")
            .burn(201, 500);
        let seen = sample(&proc, 100, Some(1000)).unwrap();
        assert_eq!(seen.items.len(), 1);
        let tool = &seen.items[0];
        assert_eq!(tool.key, "tool:200:2000");
        assert_eq!(tool.command, "/bin/bash -c cargo build --release");
        assert_eq!((tool.processes, tool.cpu_ticks), (2, 500));
        assert_eq!(tool.waits_on, None, "a running process is not waiting");
        assert!(
            sample(&proc, 100, Some(1)).is_none(),
            "a reused harness PID is not read"
        );
        assert!(sample(&proc, 999, None).is_none());
    }

    #[test]
    fn a_tool_blocked_on_a_file_lock_names_the_holder() {
        let mut proc = harness();
        proc.spawn(201, 200, 'S', "cargo build --release")
            .sleep_in(200, "do_wait")
            .sleep_in(201, "locks_lock_inode_wait")
            .open(201, 3, "/work/target/release/.cargo-lock", Some("00:2e:77"))
            .spawn(900, 1, 'S', "cargo test --workspace");
        proc.locks = "7: FLOCK  ADVISORY  WRITE 900 00:2e:77 0 EOF\n7: -> FLOCK  ADVISORY  WRITE 201 00:2e:77 0 EOF\n".to_owned();
        let tool = &sample(&proc, 100, None).unwrap().items[0];
        assert_eq!(
            tool.waits_on.as_deref(),
            Some("`cargo build --release` (PID 201) waits on a file lock on /work/target/release/.cargo-lock held by `cargo test --workspace` (PID 900)")
        );
    }

    #[test]
    fn terminal_network_pipe_and_idle_children_are_told_apart() {
        // (wait channel, an open descriptor, what the notice says)
        type Case<'a> = (&'a str, Option<(u32, &'a str)>, &'a str);
        let cases: [Case; 6] = [
            (
                "n_tty_read",
                Some((0, "/dev/pts/7")),
                "waits for input on /dev/pts/7",
            ),
            ("sk_wait_data", None, "waits on a network socket"),
            (
                "ep_poll",
                Some((5, "socket:[991]")),
                "waits on a network socket (1 open)",
            ),
            ("ep_poll", None, "is polling for an event"),
            ("pipe_read", None, "waits on a pipe"),
            ("hrtimer_nanosleep", None, "is sleeping"),
        ];
        for (wchan, fd, expected) in cases {
            let mut proc = harness();
            proc.spawn(201, 200, 'S', "leaf")
                .sleep_in(200, "do_wait")
                .sleep_in(201, wchan);
            if let Some((number, target)) = fd {
                proc.open(201, number, target, None);
            }
            let tool = &sample(&proc, 100, None).unwrap().items[0];
            assert_eq!(
                tool.waits_on.as_deref(),
                Some(format!("`leaf` (PID 201) {expected}").as_str()),
                "{wchan}"
            );
        }
        let mut proc = harness();
        proc.spawn(201, 200, 'D', "dd").sleep_in(201, "io_schedule");
        assert!(sample(&proc, 100, None).unwrap().items[0]
            .waits_on
            .as_deref()
            .unwrap()
            .ends_with("is in uninterruptible I/O (io_schedule)"));
        let mut proc = harness();
        proc.sleep_in(200, "do_wait");
        assert_eq!(
            sample(&proc, 100, None).unwrap().items[0]
                .waits_on
                .as_deref(),
            Some("`/bin/bash -c cargo build --release` (PID 200) waits for a child process")
        );
    }

    #[test]
    fn gate_steps_come_from_the_journal_a_dagrun_keeps_open() {
        let mut proc = harness();
        proc.spawn(201, 200, 'S', "dagrun run --dag validation.dag.yaml")
            .open(201, 9, "/tmp/run/journal.jsonl", None)
            .spawn(300, 201, 'S', "cargo test -p agentctl")
            .sleep_in(300, "locks_lock_inode_wait")
            .spawn(301, 201, 'R', "pytest")
            .burn(301, 40);
        proc.files.insert(
            "/tmp/run/journal.jsonl".to_owned(),
            [
                r#"{"ts":"1000.5","event":"step_start","step":"rust-agentctl","pid":"300","timeout_s":"3600","cmd":"cargo test -p agentctl"}"#,
                r#"{"ts":"1001.0","event":"step_start","step":"py-unit","pid":"301","timeout_s":"900","cmd":"pytest"}"#,
                r#"{"ts":"1002.0","event":"step_start","step":"lint","pid":"302","cmd":"ruff"}"#,
                r#"{"ts":"1010.0","event":"step_end","step":"lint","ok":true}"#,
                "not json",
            ]
            .join("\n"),
        );
        let seen = sample(&proc, 100, None).unwrap();
        let steps: Vec<&WorkItem> = seen.steps().collect();
        assert_eq!(steps.len(), 2, "the finished step is not running");
        assert_eq!(steps[0].step.as_deref(), Some("py-unit"));
        assert_eq!(steps[0].cpu_ticks, 40);
        assert_eq!(steps[1].step.as_deref(), Some("rust-agentctl"));
        assert_eq!(steps[1].started_at, Some(1000.5));
        assert_eq!(
            steps[1].waits_on.as_deref(),
            Some("`cargo test -p agentctl` (PID 300) waits on a file lock")
        );
        assert_eq!(seen.tools().count(), 1);
    }

    #[test]
    fn a_step_runner_without_a_journal_still_shows_its_steps() {
        let mut proc = harness();
        proc.spawn(
            201,
            200,
            'S',
            "/opt/bin/dagrun run --dag validation.dag.yaml",
        )
        .spawn(300, 201, 'S', "cargo test --release")
        .sleep_in(300, "locks_lock_inode_wait")
        .spawn(301, 201, 'R', "node --test");
        let seen = sample(&proc, 100, None).unwrap();
        let steps: Vec<&WorkItem> = seen.steps().collect();
        assert_eq!(steps.len(), 2);
        assert_eq!(
            steps[0].step.as_deref(),
            Some("`cargo test --release` (PID 300)")
        );
        assert_eq!(steps[0].key, "step:300:3000");
        assert_eq!(
            steps[0].waits_on.as_deref(),
            Some("`cargo test --release` (PID 300) waits on a file lock")
        );
        assert_eq!(steps[1].waits_on, None);
    }

    #[test]
    fn a_step_history_gives_a_p95_once_it_has_enough_runs() {
        let mut history = StepHistory::default();
        let mut csv = "timestamp,machine_id,step,elapsed_s,cmd\n".to_owned();
        for seconds in [100, 110, 120, 130, 140, 150, 160, 170, 180, 1900] {
            csv.push_str(&format!(
                "1,m,rust-agentctl,{seconds},\"cargo test, quoted\"\n"
            ));
        }
        csv.push_str("1,m,rare,10,x\n1,m,rare,bogus,x\n");
        history.add_csv(&csv);
        assert_eq!(history.p95("rust-agentctl"), Some((1900.0, 10)));
        assert_eq!(history.p95("rare"), None, "one usable run is not a history");
        assert_eq!(history.p95("unknown"), None);
        history.add_csv("no,useful,columns\n1,2,3\n");
        assert_eq!(csv_fields(r#"a,"b,c","d""e""#), vec!["a", "b,c", "d\"e"]);
    }
}
