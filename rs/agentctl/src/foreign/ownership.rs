//! Generation-safe ownership identity types.
//!
//! A process number names a process only until that process is reaped,
//! so every recorded identity is the triple `(boot_id, pid, start ticks)`.
//!
//! This module covers record types and read/validate: [`ProcessIdentity`],
//! [`HarnessRecord`], [`parse_harness_record`], and [`HarnessRecord::document`],
//! plus the kernel evidence those records are checked against:
//! [`current_boot_id`], [`read_stat`], and [`scan_processes`], pidfd
//! pinning ([`pin`], [`PinnedProcess`], [`stop_process`]), and harness-tree
//! cleanup ([`kill_harness_tree`], cgroup v2 helpers, [`tree_alive`]).
//! Spawning an owned harness lives elsewhere.

use std::collections::HashSet;
use std::fmt;
use std::io::Read as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::procstat::{parse_process_stat, ProcessStat};

/// Env var carrying a turn's harness generation into the harness process.
pub const GENERATION_ENV: &str = "AGENTCTL_HARNESS_GENERATION";
/// Schema tag written into `active-harness.json`.
pub const HARNESS_SCHEMA: &str = "agentctl-headless-harness/v1";
/// Schema tag written into `runner-identity.json`.
pub const RUNNER_SCHEMA: &str = "agentctl-headless-runner/v1";
/// Directory-name prefix for per-turn cgroup v2 containment.
pub const CGROUP_PREFIX: &str = "agentctl-harness-";
/// Env var selecting per-turn cgroup containment (`off`, `auto`, `require`).
pub const CGROUP_MODE_ENV: &str = "AGENTCTL_HEADLESS_CGROUP";

const MAX_ENVIRON_BYTES: u64 = 16 * 1024 * 1024;

const MAX_PROCESS_ID: i64 = (1 << 31) - 1;

/// Structured operation error with a machine-stable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignOwnershipError {
    /// Machine-stable error code (e.g. `harness_identity_unknown`).
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl ForeignOwnershipError {
    fn harness_identity_unknown(detail: &str) -> Self {
        Self {
            code: "harness_identity_unknown",
            message: format!("malformed active harness identity: {detail}"),
        }
    }

    fn runner_identity_unknown(message: String) -> Self {
        Self {
            code: "runner_identity_unknown",
            message,
        }
    }

    fn new(code: &'static str, message: String) -> Self {
        Self { code, message }
    }
}

impl fmt::Display for ForeignOwnershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ForeignOwnershipError {}

/// How one turn's harness tree is contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Containment {
    /// Tracked by process/session proofs only.
    Process,
    /// Tracked by a per-turn cgroup v2 directory.
    Cgroup,
    /// Written before boot identities and generations existed.
    Legacy,
}

impl Containment {
    fn as_str(self) -> &'static str {
        match self {
            Containment::Process => "process",
            Containment::Cgroup => "cgroup",
            Containment::Legacy => "legacy",
        }
    }
}

/// A process number bound to its kernel start ticks and, when known, its boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    /// OS process id.
    pub pid: i64,
    /// `/proc/<pid>/stat` starttime as recorded, when known.
    pub started_at: Option<String>,
    /// Boot UUID the process was recorded under, when known.
    pub boot_id: Option<String>,
}

impl ProcessIdentity {
    /// Build an identity from its recorded parts.
    pub fn new(pid: i64, started_at: Option<String>, boot_id: Option<String>) -> Self {
        Self {
            pid,
            started_at,
            boot_id,
        }
    }
}

/// The durable ownership of one turn's harness tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessRecord {
    /// Identity of the runner process owning the turn.
    pub runner: ProcessIdentity,
    /// Identity of the harness leader process.
    pub leader: ProcessIdentity,
    /// Leader's process-group/session id, when recorded.
    pub process_group: Option<i64>,
    /// 32-hex turn generation, when recorded.
    pub generation: Option<String>,
    /// How the tree is contained.
    pub containment: Containment,
    /// Per-turn cgroup directory, for [`Containment::Cgroup`] only.
    pub cgroup: Option<PathBuf>,
}

impl HarnessRecord {
    /// True for a record written before boot identities and generations existed.
    pub fn legacy(&self) -> bool {
        self.containment == Containment::Legacy
    }

    /// Serialize, keeping the keys older readers understand.
    ///
    /// Serialize this record to its document form.
    pub fn document(&self, recorded_at: &str) -> serde_json::Value {
        let mut value = serde_json::json!({
            "schema": HARNESS_SCHEMA,
            "runner_pid": self.runner.pid,
            "runner_started_at": self.runner.started_at,
            "runner_boot_id": self.runner.boot_id,
            "pid": self.leader.pid,
            "started_at": self.leader.started_at,
            "boot_id": self.leader.boot_id,
            "process_group": self.process_group,
            "generation": self.generation,
            "containment": self.containment.as_str(),
            "recorded_at": recorded_at,
        });
        if let Some(cgroup) = &self.cgroup {
            value["cgroup"] = serde_json::Value::String(cgroup.to_string_lossy().into_owned());
        }
        value
    }
}

/// True for a canonical lower-case boot UUID.
pub fn valid_boot_id(value: &str) -> bool {
    // `[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}`
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lens = [8usize, 4, 4, 4, 12];
    parts.iter().zip(lens.iter()).all(|(part, &len)| {
        part.len() == len
            && part
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}

/// True for a 32-char lowercase hex generation token.
pub fn valid_generation(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// True for canonical start-ticks text: `0` or a 1–20 digit number without leading zeros.
pub fn valid_ticks(value: &str) -> bool {
    // `(?:0|[1-9][0-9]{0,19})`
    if value.is_empty() || value.len() > 20 {
        return false;
    }
    if !value.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    value.len() == 1 || !value.starts_with('0')
}

/// The procfs root the kernel-evidence readers use by default.
const PROC: &str = "/proc";
/// A boot identity file longer than this is malformed evidence, as in Python.
const MAX_BOOT_ID_BYTES: usize = 128;

/// Return the canonical Linux boot UUID, refusing unreadable evidence.
pub fn current_boot_id() -> Result<String, ForeignOwnershipError> {
    current_boot_id_at(Path::new(PROC))
}

/// [`current_boot_id`] against an explicit procfs root (for tests).
pub fn current_boot_id_at(proc_root: &Path) -> Result<String, ForeignOwnershipError> {
    let path = proc_root.join("sys/kernel/random/boot_id");
    // Read at most one byte past the limit: anything longer is malformed.
    let mut raw = Vec::new();
    let read = std::fs::File::open(&path).and_then(|file| {
        file.take((MAX_BOOT_ID_BYTES + 1) as u64)
            .read_to_end(&mut raw)
    });
    if let Err(error) = read {
        return Err(ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot read the Linux boot identity: {error}"
        )));
    }
    let value = String::from_utf8_lossy(&raw).trim().to_owned();
    if raw.is_empty() || raw.len() > MAX_BOOT_ID_BYTES || !valid_boot_id(&value) {
        return Err(ForeignOwnershipError::runner_identity_unknown(
            "the Linux boot identity is malformed".to_owned(),
        ));
    }
    Ok(value)
}

/// Return one process's identity fields, or `None` only for positive
/// disappearance. Unreadable or malformed procfs evidence is not a death
/// certificate and must not authorize later process signalling.
pub fn read_stat(pid: i64) -> Result<Option<ProcessStat>, ForeignOwnershipError> {
    read_stat_at(Path::new(PROC), pid)
}

/// [`read_stat`] against an explicit procfs root (for tests).
pub fn read_stat_at(
    proc_root: &Path,
    pid: i64,
) -> Result<Option<ProcessStat>, ForeignOwnershipError> {
    let raw = match std::fs::read(proc_root.join(pid.to_string()).join("stat")) {
        Ok(raw) => raw,
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ESRCH) =>
        {
            return Ok(None);
        }
        Err(error) => {
            return Err(ForeignOwnershipError::runner_identity_unknown(format!(
                "cannot read runner process {pid} identity: {error}"
            )));
        }
    };
    match parse_process_stat(&raw) {
        Some(parsed) if parsed.pid == pid => Ok(Some(parsed)),
        _ => Err(ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot parse runner process {pid} identity"
        ))),
    }
}

fn quiet_stat_at(proc_root: &Path, pid: i64) -> Option<ProcessStat> {
    read_stat_at(proc_root, pid).ok().flatten()
}

/// List every process procfs shows; vanished or unreadable ones are skipped.
pub fn scan_processes() -> Result<Vec<ProcessStat>, ForeignOwnershipError> {
    scan_processes_at(Path::new(PROC))
}

/// [`scan_processes`] against an explicit procfs root (for tests).
pub fn scan_processes_at(proc_root: &Path) -> Result<Vec<ProcessStat>, ForeignOwnershipError> {
    let entries = std::fs::read_dir(proc_root).map_err(|error| {
        ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot list {}: {error}",
            proc_root.display()
        ))
    })?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<i64>() else {
            continue;
        };
        if let Some(parsed) = quiet_stat_at(proc_root, pid) {
            found.push(parsed);
        }
    }
    Ok(found)
}

fn same_user_at(proc_root: &Path, pid: i64) -> bool {
    std::fs::metadata(proc_root.join(pid.to_string()))
        .map(|metadata| metadata.uid() == unsafe { libc::getuid() })
        .unwrap_or(false)
}

/// True when the process's initial environment carries this turn's generation.
pub fn carries_generation(pid: i64, generation: &str) -> bool {
    carries_generation_at(Path::new(PROC), pid, generation)
}

/// [`carries_generation`] against an explicit procfs root (for tests).
pub fn carries_generation_at(proc_root: &Path, pid: i64, generation: &str) -> bool {
    let needle = format!("{GENERATION_ENV}={generation}").into_bytes();
    let Ok(stream) = std::fs::File::open(proc_root.join(pid.to_string()).join("environ")) else {
        return false;
    };
    let mut data = Vec::new();
    if stream
        .take(MAX_ENVIRON_BYTES)
        .read_to_end(&mut data)
        .is_err()
    {
        return false;
    }
    data.split(|byte| *byte == 0).any(|entry| entry == needle)
}

/// Process states that are positive death certificates (`_DEAD_STATES`).
const DEAD_STATES: [&str; 3] = ["Z", "X", "x"];

/// True when this host provides `pidfd_open` and `pidfd_send_signal`.
#[cfg(target_os = "linux")]
pub fn pidfds_supported() -> bool {
    open_pidfd(std::process::id() as i64)
        .map(|descriptor| descriptor.is_some())
        .unwrap_or(false)
}

/// True when this host provides `pidfd_open` and `pidfd_send_signal`.
#[cfg(not(target_os = "linux"))]
pub fn pidfds_supported() -> bool {
    false
}

/// Open a pidfd; `None` when the number names no process.
#[cfg(target_os = "linux")]
fn open_pidfd(pid: i64) -> std::io::Result<Option<OwnedFd>> {
    for _attempt in 0..4 {
        // SAFETY: pidfd_open takes scalar arguments and returns a fresh descriptor.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0_u32) };
        if descriptor >= 0 {
            let descriptor = i32::try_from(descriptor).map_err(|_| {
                std::io::Error::other("pidfd_open returned an out-of-range descriptor")
            })?;
            // SAFETY: the successful syscall returned a fresh descriptor owned here.
            return Ok(Some(unsafe { OwnedFd::from_raw_fd(descriptor) }));
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "pidfd_open was interrupted repeatedly",
    ))
}

#[cfg(not(target_os = "linux"))]
fn open_pidfd(_pid: i64) -> std::io::Result<Option<OwnedFd>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "pidfds are Linux-only",
    ))
}

/// Poll one descriptor for readability; `None` waits without bound.
fn fd_readable(descriptor: &OwnedFd, timeout: Option<Duration>) -> bool {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    loop {
        let millis = match deadline {
            None => -1,
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX)
            }
        };
        let mut poll = libc::pollfd {
            fd: descriptor.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one initialized pollfd, retained for the call.
        let result = unsafe { libc::poll(&raw mut poll, 1, millis) };
        if result > 0 {
            return poll.revents != 0;
        }
        if result == 0 {
            return false;
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return false;
        }
    }
}

/// One exact process generation held by a pidfd.
#[derive(Debug)]
pub struct PinnedProcess {
    pid: i64,
    starttime: u64,
    descriptor: OwnedFd,
}

impl PinnedProcess {
    /// The pinned process number.
    pub fn pid(&self) -> i64 {
        self.pid
    }

    /// The kernel start ticks proved when the process was pinned.
    pub fn starttime(&self) -> u64 {
        self.starttime
    }

    /// The open pidfd.
    pub fn descriptor(&self) -> &OwnedFd {
        &self.descriptor
    }

    /// True once the process has exited (it may still await its parent's reap).
    pub fn exited(&self) -> bool {
        fd_readable(&self.descriptor, Some(Duration::ZERO))
    }

    /// Wait up to `timeout` (`None`: without bound) for exit; true when it exited.
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        fd_readable(&self.descriptor, timeout)
    }

    /// Signal exactly this process; false when it has exited or refuses the
    /// signal. A refusal must not abort a cleanup halfway; the member is
    /// then reported as remaining.
    #[cfg(target_os = "linux")]
    pub fn send(&self, signum: i32) -> bool {
        // SAFETY: the pidfd addresses a stable process identity; null siginfo
        // is the documented ordinary signal form.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.descriptor.as_raw_fd(),
                signum,
                std::ptr::null::<libc::siginfo_t>(),
                0_u32,
            )
        };
        result >= 0
    }

    /// Signal exactly this process; pidfds are Linux-only.
    #[cfg(not(target_os = "linux"))]
    pub fn send(&self, _signum: i32) -> bool {
        false
    }

    /// Release the pidfd.
    pub fn close(self) {}
}

/// An unreaped direct child of this process, for hosts without pidfds.
///
/// Only this process can reap its child, so while `reaped` reports false the
/// number still names the same process and a numeric signal is exact.
pub struct ChildHandle {
    pid: i64,
    starttime: u64,
    reaped: Box<dyn Fn() -> bool + Send + Sync>,
}

impl fmt::Debug for ChildHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildHandle")
            .field("pid", &self.pid)
            .field("starttime", &self.starttime)
            .finish_non_exhaustive()
    }
}

impl ChildHandle {
    /// Hold one unreaped child; `reaped` reports when its owner reaped it.
    pub fn new(
        pid: i64,
        starttime: u64,
        reaped: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            pid,
            starttime,
            reaped: Box::new(reaped),
        }
    }

    /// The held child's process number.
    pub fn pid(&self) -> i64 {
        self.pid
    }

    /// The kernel start ticks proved when the child was held.
    pub fn starttime(&self) -> u64 {
        self.starttime
    }

    /// True once the child has exited; never reaps it.
    pub fn exited(&self) -> bool {
        if (self.reaped)() {
            return true;
        }
        // SAFETY: waitid writes only into the zeroed siginfo owned by this frame.
        let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::waitid(
                libc::P_PID,
                libc::id_t::try_from(self.pid).unwrap_or(0),
                &mut information,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if status != 0 {
            // ECHILD: not our child (or already reaped elsewhere) means gone
            // for the parent-pinned contract, as Python's ChildProcessError.
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD);
        }
        // SAFETY: si_pid is valid for the SIGCHLD information waitid reports.
        (unsafe { information.si_pid() }) != 0
    }

    /// Poll for exit up to `timeout` (`None`: without bound) without reaping.
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        while !self.exited() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// Signal the child while it is provably unreaped.
    pub fn send(&self, signum: i32) -> bool {
        if self.exited() {
            return false;
        }
        // Parent-pinned: an unreaped child keeps its number.
        // SAFETY: kill takes only the checked child pid and signal number.
        (unsafe { libc::kill(self.pid as libc::pid_t, signum) }) >= 0
    }

    /// Nothing to release; present for symmetry with [`PinnedProcess`].
    pub fn close(self) {}
}

/// A held process generation: pidfd-pinned, or a runner's own unreaped child.
#[derive(Debug)]
pub enum ProcessHandle {
    /// Held by a pidfd.
    Pinned(PinnedProcess),
    /// Held as this process's unreaped child.
    Child(ChildHandle),
}

impl ProcessHandle {
    /// The held process number.
    pub fn pid(&self) -> i64 {
        match self {
            Self::Pinned(process) => process.pid(),
            Self::Child(child) => child.pid(),
        }
    }

    /// The kernel start ticks proved when the process was held.
    pub fn starttime(&self) -> u64 {
        match self {
            Self::Pinned(process) => process.starttime(),
            Self::Child(child) => child.starttime(),
        }
    }

    /// True once the held process has exited.
    pub fn exited(&self) -> bool {
        match self {
            Self::Pinned(process) => process.exited(),
            Self::Child(child) => child.exited(),
        }
    }

    /// Wait up to `timeout` (`None`: without bound) for exit.
    pub fn wait(&self, timeout: Option<Duration>) -> bool {
        match self {
            Self::Pinned(process) => process.wait(timeout),
            Self::Child(child) => child.wait(timeout),
        }
    }

    /// Signal exactly the held process; false on exit or refusal.
    pub fn send(&self, signum: i32) -> bool {
        match self {
            Self::Pinned(process) => process.send(signum),
            Self::Child(child) => child.send(signum),
        }
    }

    /// Release the handle.
    pub fn close(self) {
        match self {
            Self::Pinned(process) => process.close(),
            Self::Child(child) => child.close(),
        }
    }
}

/// Whether [`pin`] proved the recorded process alive, gone, or neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinVerdict {
    /// The pidfd holds the recorded process (`ALIVE`).
    Alive,
    /// Positive death certificate (`GONE`).
    Gone,
    /// Neither proved; never authorizes a signal (`UNPROVEN`).
    Unproven,
}

/// Outcome of pinning one recorded identity.
#[derive(Debug)]
pub struct Pin {
    /// The pin verdict.
    pub verdict: PinVerdict,
    /// Why this verdict, for operator-facing errors.
    pub reason: String,
    /// The held process, for [`PinVerdict::Alive`] only.
    pub process: Option<PinnedProcess>,
}

fn gone(reason: String) -> Pin {
    Pin {
        verdict: PinVerdict::Gone,
        reason,
        process: None,
    }
}

fn unproven(reason: String) -> Pin {
    Pin {
        verdict: PinVerdict::Unproven,
        reason,
        process: None,
    }
}

/// Proof callback for a record without a boot identity (`legacy_proof`).
pub type LegacyProof<'a> = &'a dyn Fn(i64, &ProcessStat) -> bool;

/// Pin a recorded process, or prove it gone, or say why neither is possible.
///
/// [`PinVerdict::Gone`] is a positive death certificate: a different boot, no
/// such process, a newer process under the number, or an exited one.
/// [`PinVerdict::Unproven`] never authorizes a signal. A record without a boot
/// identity is pinned only when `legacy_proof` accepts the process.
pub fn pin(
    identity: &ProcessIdentity,
    legacy_proof: Option<LegacyProof<'_>>,
) -> Result<Pin, ForeignOwnershipError> {
    pin_at(Path::new(PROC), identity, legacy_proof)
}

/// [`pin`] against an explicit procfs root (for tests). The pidfd itself
/// always names the real process.
pub fn pin_at(
    proc_root: &Path,
    identity: &ProcessIdentity,
    legacy_proof: Option<LegacyProof<'_>>,
) -> Result<Pin, ForeignOwnershipError> {
    let pid = identity.pid;
    if !(2..=MAX_PROCESS_ID).contains(&pid) {
        return Ok(unproven(format!(
            "recorded pid {pid} is not a valid process number"
        )));
    }
    let boot = current_boot_id_at(proc_root)?;
    if let Some(recorded_boot) = &identity.boot_id {
        if !valid_boot_id(recorded_boot) {
            return Ok(unproven(format!(
                "pid {pid} has a malformed recorded boot identity"
            )));
        }
        if *recorded_boot != boot {
            if current_boot_id_at(proc_root)? == boot {
                return Ok(gone(format!(
                    "pid {pid} was recorded during an earlier boot"
                )));
            }
            return Ok(unproven(
                "the boot identity changed while it was read".to_owned(),
            ));
        }
    }
    if let Some(started_at) = &identity.started_at {
        if !valid_ticks(started_at) {
            return Ok(unproven(format!(
                "pid {pid} has an unreadable recorded start time '{started_at}'"
            )));
        }
    }
    if !pidfds_supported() {
        let current = read_stat_at(proc_root, pid)?;
        let mismatch = match (&current, &identity.started_at) {
            (None, _) => true,
            (Some(current), Some(started_at)) => current.starttime.to_string() != *started_at,
            (Some(_), None) => false,
        };
        if mismatch {
            return Ok(gone(format!(
                "pid {pid} no longer names the recorded process"
            )));
        }
        return Ok(unproven(
            "this host does not provide pidfds, so the process cannot be pinned".to_owned(),
        ));
    }
    let descriptor = open_pidfd(pid).map_err(|error| {
        ForeignOwnershipError::runner_identity_unknown(format!(
            "cannot open a pidfd for pid {pid}: {error}"
        ))
    })?;
    let Some(descriptor) = descriptor else {
        if current_boot_id_at(proc_root)? == boot {
            return Ok(gone(format!("pid {pid} has exited")));
        }
        return Ok(unproven(
            "the boot identity changed while it was read".to_owned(),
        ));
    };
    let current = read_stat_at(proc_root, pid)?;
    let Some(current) = current else {
        if fd_readable(&descriptor, Some(Duration::ZERO)) {
            return Ok(gone(format!("pid {pid} has exited")));
        }
        return Ok(unproven(format!(
            "pid {pid} vanished from procfs while its pidfd was open"
        )));
    };
    let Some(started_at) = &identity.started_at else {
        return Ok(unproven(format!(
            "pid {pid} has no recorded start time, so its generation is unknown"
        )));
    };
    if current.starttime.to_string() != *started_at {
        return Ok(gone(format!("pid {pid} now names a newer process")));
    }
    if DEAD_STATES.contains(&current.state.as_str()) {
        return Ok(gone(format!("pid {pid} has exited")));
    }
    if identity.boot_id.is_none() && !legacy_proof.is_some_and(|proof| proof(pid, &current)) {
        return Ok(unproven(format!(
            "pid {pid} has a legacy record without a boot identity and is not proven"
        )));
    }
    if fd_readable(&descriptor, Some(Duration::ZERO)) {
        return Ok(gone(format!("pid {pid} has exited")));
    }
    Ok(Pin {
        verdict: PinVerdict::Alive,
        reason: format!("pid {pid} is the recorded process"),
        process: Some(PinnedProcess {
            pid,
            starttime: current.starttime,
            descriptor,
        }),
    })
}

/// How long a cleanup waits for SIGKILLed processes to disappear. A process
/// in uninterruptible sleep outlives this bound and is reported, not
/// forgotten. (`$SUBAGENTS_HARNESS_KILL_WAIT`, default 10 seconds, as in
/// Python's `KILL_WAIT_S`; read per call rather than at import.)
fn default_kill_wait() -> Duration {
    std::env::var("SUBAGENTS_HARNESS_KILL_WAIT")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .and_then(|seconds| Duration::try_from_secs_f64(seconds.max(0.0)).ok())
        .unwrap_or(Duration::from_secs(10))
}

/// Python's `f"{bound:g}s"` shape for a seconds bound.
fn format_seconds(bound: Duration) -> String {
    let seconds = bound.as_secs_f64();
    if seconds.fract() == 0.0 {
        format!("{}", seconds as i64)
    } else {
        let text = format!("{seconds:.6}");
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// SIGTERM, then after `grace` SIGKILL, one pinned process.
///
/// Returns false when the process was already gone. An unprovable identity
/// errors with `runner_identity_unproven` (or `pidfd_unsupported`) without a
/// signal, and a process that outlives SIGKILL by `kill_wait` (default:
/// [`default_kill_wait`]) errors with `runner_stop_incomplete`.
pub fn stop_process(
    identity: &ProcessIdentity,
    grace: Duration,
    legacy_proof: Option<LegacyProof<'_>>,
    kill_wait: Option<Duration>,
    label: &str,
) -> Result<bool, ForeignOwnershipError> {
    stop_process_at(
        Path::new(PROC),
        identity,
        grace,
        legacy_proof,
        kill_wait,
        label,
    )
}

/// [`stop_process`] against an explicit procfs root (for tests).
pub fn stop_process_at(
    proc_root: &Path,
    identity: &ProcessIdentity,
    grace: Duration,
    legacy_proof: Option<LegacyProof<'_>>,
    kill_wait: Option<Duration>,
    label: &str,
) -> Result<bool, ForeignOwnershipError> {
    let result = pin_at(proc_root, identity, legacy_proof)?;
    if result.verdict == PinVerdict::Gone {
        return Ok(false);
    }
    let Some(process) = result.process else {
        let code = if pidfds_supported() {
            "runner_identity_unproven"
        } else {
            "pidfd_unsupported"
        };
        return Err(ForeignOwnershipError::new(
            code,
            format!("refusing to signal {label}: {}", result.reason),
        ));
    };
    process.send(libc::SIGTERM);
    if process.wait(Some(grace)) {
        return Ok(true);
    }
    process.send(libc::SIGKILL);
    let bound = kill_wait.unwrap_or_else(default_kill_wait);
    if process.wait(Some(bound)) {
        return Ok(true);
    }
    let state = quiet_stat_at(proc_root, process.pid());
    Err(ForeignOwnershipError::new(
        "runner_stop_incomplete",
        format!(
            "{label} pid {} is still present {}s after SIGKILL{}",
            process.pid(),
            format_seconds(bound),
            state.map_or(String::new(), |state| format!(" (state {})", state.state))
        ),
    ))
}

fn process_number(value: Option<&serde_json::Value>) -> Option<i64> {
    // A boolean is not accepted as a process number; range `1..=2^31-1`.
    let n = value?.as_i64()?;
    if (1..=MAX_PROCESS_ID).contains(&n) {
        Some(n)
    } else {
        None
    }
}

/// Parse `active-harness.json`; malformed ownership errors with
/// `harness_identity_unknown`.
///
/// Parse and validate a harness record document.
pub fn parse_harness_record(
    value: &serde_json::Value,
) -> Result<HarnessRecord, ForeignOwnershipError> {
    let bad = ForeignOwnershipError::harness_identity_unknown;
    let obj = value.as_object().ok_or_else(|| bad("not an object"))?;

    let pid = process_number(obj.get("pid"));
    let started_at = obj.get("started_at").and_then(serde_json::Value::as_str);
    let (pid, started_at) = match (pid, started_at) {
        (Some(pid), Some(started_at)) if pid > 1 => (pid, started_at.to_owned()),
        _ => return Err(bad("no valid harness pid and start time")),
    };

    let runner_pid = process_number(obj.get("runner_pid"));
    let runner_started_raw = obj.get("runner_started_at");
    let runner_started_ok = match runner_started_raw {
        None | Some(serde_json::Value::Null) => true,
        Some(v) => v.as_str().is_some(),
    };
    let runner_pid = match runner_pid {
        Some(p) if runner_started_ok => p,
        _ => return Err(bad("no valid runner pid")),
    };
    let runner_started_at: Option<String> = match runner_started_raw {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => v.as_str().map(str::to_owned),
    };

    if !obj.contains_key("schema") {
        return Ok(HarnessRecord {
            runner: ProcessIdentity::new(runner_pid, runner_started_at, None),
            leader: ProcessIdentity::new(pid, Some(started_at), None),
            process_group: Some(pid),
            generation: None,
            containment: Containment::Legacy,
            cgroup: None,
        });
    }

    if obj.get("schema").and_then(serde_json::Value::as_str) != Some(HARNESS_SCHEMA) {
        let schema = obj
            .get("schema")
            .map(std::string::ToString::to_string)
            .unwrap_or_default();
        return Err(bad(&format!("unsupported schema {schema:?}")));
    }

    let boot_id = obj.get("boot_id").and_then(serde_json::Value::as_str);
    let runner_boot_id = obj
        .get("runner_boot_id")
        .and_then(serde_json::Value::as_str);
    let runner_boot_ok = match obj.get("runner_boot_id") {
        None | Some(serde_json::Value::Null) => true,
        Some(_) => runner_boot_id.is_some_and(valid_boot_id),
    };
    if !boot_id.is_some_and(valid_boot_id) || !runner_boot_ok {
        return Err(bad("no valid boot identity"));
    }
    let boot_id = boot_id.expect("validated above").to_owned();

    let generation = obj.get("generation").and_then(serde_json::Value::as_str);
    if !generation.is_some_and(valid_generation) {
        return Err(bad("no valid generation"));
    }
    let generation = generation.expect("validated above").to_owned();

    let group = process_number(obj.get("process_group"));
    let containment = match obj.get("containment").and_then(serde_json::Value::as_str) {
        Some("process") => Containment::Process,
        Some("cgroup") => Containment::Cgroup,
        _ => return Err(bad("no valid process group or containment")),
    };
    let group = match group {
        Some(g) => g,
        None => return Err(bad("no valid process group or containment")),
    };

    let raw_cgroup = obj.get("cgroup");
    let cgroup: Option<PathBuf> = match containment {
        Containment::Cgroup => {
            let raw = raw_cgroup
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| bad("cgroup containment without a cgroup path"))?;
            let path = PathBuf::from(raw);
            let has_parent = path.components().any(|c| matches!(c, Component::ParentDir));
            let expected_name = format!("{CGROUP_PREFIX}{generation}");
            let name_ok = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy() == expected_name);
            if !path.is_absolute() || has_parent || !name_ok {
                return Err(bad("cgroup path does not name this generation's directory"));
            }
            Some(path)
        }
        _ => {
            if raw_cgroup.is_some_and(|v| !v.is_null()) {
                return Err(bad("process containment with a cgroup path"));
            }
            None
        }
    };

    Ok(HarnessRecord {
        runner: ProcessIdentity::new(
            runner_pid,
            runner_started_at,
            runner_boot_id.map(str::to_owned),
        ),
        leader: ProcessIdentity::new(pid, Some(started_at), Some(boot_id)),
        process_group: Some(group),
        generation: Some(generation),
        containment,
        cgroup,
    })
}

/// The configured per-turn cgroup mode: `off` (default), `auto`, or `require`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CgroupMode {
    /// No per-turn cgroup containment.
    Off,
    /// Use a cgroup when one can be created, else process containment.
    Auto,
    /// Refuse to spawn without per-turn cgroup containment.
    Require,
}

impl CgroupMode {
    /// The env spelling of this mode.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Require => "require",
        }
    }
}

/// Read [`CgroupMode`] from `$AGENTCTL_HEADLESS_CGROUP`.
pub fn cgroup_mode() -> Result<CgroupMode, ForeignOwnershipError> {
    cgroup_mode_from(std::env::var(CGROUP_MODE_ENV).ok().as_deref())
}

fn cgroup_mode_from(value: Option<&str>) -> Result<CgroupMode, ForeignOwnershipError> {
    match value.unwrap_or("off") {
        "off" => Ok(CgroupMode::Off),
        "auto" => Ok(CgroupMode::Auto),
        "require" => Ok(CgroupMode::Require),
        other => Err(ForeignOwnershipError::new(
            "invalid_cgroup_mode",
            format!("{CGROUP_MODE_ENV} must be off, auto or require, not '{other}'"),
        )),
    }
}

fn cgroup2_mount_at(proc_root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(proc_root.join("self/mountinfo")).ok()?;
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<&str> = left.split_whitespace().collect();
        if fields.len() >= 5 && right.split_whitespace().next() == Some("cgroup2") {
            return Some(PathBuf::from(fields[4]));
        }
    }
    None
}

fn cgroup_of_at(proc_root: &Path, pid: &str) -> Option<String> {
    let text = std::fs::read_to_string(proc_root.join(pid).join("cgroup")).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
}

/// This process's cgroup v2 directory, when the unified hierarchy is mounted.
pub fn own_cgroup_directory() -> Option<PathBuf> {
    own_cgroup_directory_at(Path::new(PROC))
}

/// [`own_cgroup_directory`] against an explicit procfs root (for tests).
pub fn own_cgroup_directory_at(proc_root: &Path) -> Option<PathBuf> {
    let mount = cgroup2_mount_at(proc_root)?;
    let relative = cgroup_of_at(proc_root, "self")?;
    let relative = relative.strip_prefix('/')?;
    Some(mount.join(relative))
}

fn cgroup_relative_at(proc_root: &Path, path: &Path) -> Option<String> {
    let mount = cgroup2_mount_at(proc_root)?;
    path.strip_prefix(&mount)
        .ok()
        .map(|relative| format!("/{}", relative.display()))
}

fn cgroup_events(path: &Path) -> std::io::Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path.join("cgroup.events"))?;
    Ok(text
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(' ').unwrap_or((line, ""));
            (key.to_owned(), value.trim().to_owned())
        })
        .collect())
}

fn cgroup_populated(path: &Path) -> std::io::Result<Option<bool>> {
    Ok(cgroup_events(path)?
        .into_iter()
        .find(|(key, _)| key == "populated")
        .map(|(_, value)| value == "1"))
}

fn cgroup_members(path: &Path) -> std::io::Result<Vec<i64>> {
    let text = match std::fs::read_to_string(path.join("cgroup.procs")) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    Ok(text
        .split_whitespace()
        .filter(|item| item.bytes().all(|byte| byte.is_ascii_digit()))
        .filter_map(|item| item.parse::<i64>().ok())
        .collect())
}

fn remove_cgroup(path: &Path) {
    for _attempt in 0..50 {
        match std::fs::remove_dir(path) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) if error.raw_os_error() == Some(libc::EBUSY) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return,
        }
    }
}

fn cgroup_error(context: &str, path: &Path, error: &std::io::Error) -> ForeignOwnershipError {
    ForeignOwnershipError::new(
        "harness_cgroup_unavailable",
        format!("cannot {context} {}: {error}", path.display()),
    )
}

/// Kill every process in one owned cgroup and wait for it to empty.
/// Returns how many members it named and who remains.
fn kill_cgroup_at(
    proc_root: &Path,
    path: &Path,
    deadline: Instant,
) -> Result<(usize, Vec<String>), ForeignOwnershipError> {
    if !path.is_dir() {
        return Ok((0, Vec::new()));
    }
    let members = cgroup_members(path).map_err(|error| cgroup_error("read", path, &error))?;
    let populated = match cgroup_populated(path) {
        Ok(populated) => populated.unwrap_or(false),
        Err(_) => !members.is_empty(),
    };
    if populated {
        let kill_file = path.join("cgroup.kill");
        if kill_file.exists() {
            std::fs::write(&kill_file, "1\n")
                .map_err(|error| cgroup_error("write", &kill_file, &error))?;
        } else {
            // Kernels before 5.14 lack cgroup.kill. Membership in this
            // generation's directory is the ownership proof; pin each member
            // and re-check that proof after the pidfd is open.
            let expected = cgroup_relative_at(proc_root, path);
            for pid in &members {
                let Some(stat) = quiet_stat_at(proc_root, *pid) else {
                    continue;
                };
                if !pidfds_supported() {
                    continue;
                }
                let Ok(Some(descriptor)) = open_pidfd(*pid) else {
                    continue;
                };
                let process = PinnedProcess {
                    pid: *pid,
                    starttime: stat.starttime,
                    descriptor,
                };
                let after = quiet_stat_at(proc_root, *pid);
                if after.is_some_and(|after| after.starttime == stat.starttime)
                    && expected.is_some()
                    && cgroup_of_at(proc_root, &pid.to_string()) == expected
                    && !process.exited()
                {
                    process.send(libc::SIGKILL);
                }
            }
        }
        while Instant::now() < deadline {
            match cgroup_populated(path) {
                Ok(Some(false)) => break,
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(cgroup_error("read", path, &error)),
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let still = matches!(cgroup_populated(path), Ok(Some(true)));
    let mut remaining = Vec::new();
    if still {
        let members = cgroup_members(path).map_err(|error| cgroup_error("read", path, &error))?;
        for pid in members {
            let stat = quiet_stat_at(proc_root, pid);
            remaining.push(format!(
                "pid {pid}{} in {}",
                stat.map_or(String::new(), |stat| format!(" (state {})", stat.state)),
                path.display()
            ));
        }
    } else {
        remove_cgroup(path);
    }
    Ok((members.len(), remaining))
}

/// What one harness-tree cleanup signalled, left behind, and could not prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanupReport {
    /// How the tree was contained.
    pub containment: Containment,
    /// How many processes were signalled (SIGKILL counted once per member).
    pub killed: usize,
    /// Proven members still present after the bounded wait.
    pub remaining: Vec<String>,
    /// Candidates never signalled because ownership was unproven.
    pub unproven: Vec<String>,
    /// Whether the census reached a fixed point before its deadline.
    pub converged: bool,
}

impl CleanupReport {
    fn new(containment: Containment) -> Self {
        Self {
            containment,
            killed: 0,
            remaining: Vec::new(),
            unproven: Vec::new(),
            converged: true,
        }
    }

    /// True when every proven member of the tree is gone.
    pub fn complete(&self) -> bool {
        self.remaining.is_empty()
    }

    /// One human-readable line for errors and transcripts.
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("{} process(es) signalled", self.killed)];
        if !self.remaining.is_empty() {
            parts.push(format!(
                "still present after SIGKILL: {}",
                self.remaining.join(", ")
            ));
        }
        if !self.converged {
            parts.push("the census did not converge before the deadline".to_owned());
        }
        if !self.unproven.is_empty() {
            parts.push(format!(
                "not signalled because ownership is unproven: {}",
                self.unproven.join(", ")
            ));
        }
        parts.join("; ")
    }
}

static SUBREAPER: AtomicBool = AtomicBool::new(false);

/// Whether this process has adopted its descendants' orphans
/// ([`set_child_subreaper`]).
pub fn is_child_subreaper() -> bool {
    SUBREAPER.load(Ordering::SeqCst)
}

/// Make this process adopt its descendants' orphans; false when unsupported.
///
/// Only a runner calls this. Its children are then exactly the current
/// harness leader and orphans adopted from that tree, which is what lets
/// "child of the runner" prove membership. A process that is not a
/// subreaper (a test, a CLI) never treats its own children as tree members.
#[cfg(target_os = "linux")]
pub fn set_child_subreaper() -> bool {
    // SAFETY: prctl takes only scalar arguments.
    let result = unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
    SUBREAPER.store(result == 0, Ordering::SeqCst);
    result == 0
}

/// Make this process adopt its descendants' orphans; Linux-only.
#[cfg(not(target_os = "linux"))]
pub fn set_child_subreaper() -> bool {
    SUBREAPER.store(false, Ordering::SeqCst);
    false
}

fn children_of_self_at(proc_root: &Path) -> Vec<i64> {
    let me = i64::from(std::process::id());
    let tasks = proc_root.join("self/task");
    let listed = std::fs::read_dir(&tasks).and_then(|entries| {
        let mut children = Vec::new();
        for entry in entries.flatten() {
            let text = std::fs::read_to_string(entry.path().join("children"))?;
            children.extend(
                text.split_whitespace()
                    .filter(|item| item.bytes().all(|byte| byte.is_ascii_digit()))
                    .filter_map(|item| item.parse::<i64>().ok()),
            );
        }
        Ok(children)
    });
    let mut children = match listed {
        Ok(children) => children,
        Err(_) => scan_processes_at(proc_root)
            .map(|stats| {
                stats
                    .into_iter()
                    .filter(|stat| stat.ppid == me)
                    .map(|stat| stat.pid)
                    .collect()
            })
            .unwrap_or_default(),
    };
    children.sort_unstable();
    children.dedup();
    children
}

/// Reap exited orphans this child subreaper adopted; never touches `exclude`.
/// A process that is not a subreaper adopts nothing and reaps nothing.
pub fn reap_adopted_children(exclude: &[i64]) -> usize {
    reap_adopted_children_at(Path::new(PROC), exclude)
}

fn reap_adopted_children_at(proc_root: &Path, exclude: &[i64]) -> usize {
    if !is_child_subreaper() {
        return 0;
    }
    let mut reaped = 0;
    for pid in children_of_self_at(proc_root) {
        if exclude.contains(&pid) {
            continue;
        }
        let Some(stat) = quiet_stat_at(proc_root, pid) else {
            continue;
        };
        if !DEAD_STATES.contains(&stat.state.as_str()) {
            continue;
        }
        // SAFETY: waitid writes only into the zeroed siginfo owned here.
        let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::waitid(
                libc::P_PID,
                libc::id_t::try_from(pid).unwrap_or(0),
                &mut information,
                libc::WEXITED | libc::WNOHANG,
            )
        };
        if status == 0 && unsafe { information.si_pid() } != 0 {
            reaped += 1;
        }
    }
    reaped
}

fn describe_handle_at(proc_root: &Path, pid: i64) -> String {
    let stat = quiet_stat_at(proc_root, pid);
    format!(
        "pid {pid}{}",
        stat.map_or(String::new(), |stat| format!(" (state {})", stat.state))
    )
}

fn children_supported_at(proc_root: &Path) -> bool {
    let tasks = proc_root.join("self/task");
    let Ok(entries) = std::fs::read_dir(&tasks) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.path().join("children").exists())
}

fn children_of_at(proc_root: &Path, pid: i64) -> Option<Vec<i64>> {
    let tasks = proc_root.join(pid.to_string()).join("task");
    let mut children = Vec::new();
    let listed = std::fs::read_dir(&tasks).and_then(|entries| {
        for entry in entries.flatten() {
            let text = std::fs::read_to_string(entry.path().join("children"))?;
            children.extend(
                text.split_whitespace()
                    .filter(|item| item.bytes().all(|byte| byte.is_ascii_digit()))
                    .filter_map(|item| item.parse::<i64>().ok()),
            );
        }
        Ok(())
    });
    match listed {
        Ok(()) => Some(children),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if !proc_root.join("self/task").is_dir() || children_supported_at(proc_root) {
                Some(Vec::new())
            } else {
                None
            }
        }
        Err(_) => None,
    }
}

fn minimum_start(record: &HarnessRecord) -> u128 {
    match record.leader.started_at.as_deref() {
        Some(started_at) if valid_ticks(started_at) => started_at.parse::<u128>().unwrap_or(0),
        _ => 0,
    }
}

fn tree_has_candidates_at(
    proc_root: &Path,
    record: &HarnessRecord,
) -> Result<bool, ForeignOwnershipError> {
    let leader = quiet_stat_at(proc_root, record.leader.pid);
    if leader.is_some_and(|leader| {
        record.leader.started_at.as_deref() == Some(leader.starttime.to_string().as_str())
            && !DEAD_STATES.contains(&leader.state.as_str())
    }) {
        return Ok(true);
    }
    let Some(generation) = record.generation.as_deref() else {
        return Ok(false);
    };
    let minimum = minimum_start(record);
    let me = i64::from(std::process::id());
    for stat in scan_processes_at(proc_root)? {
        if stat.pid != me
            && u128::from(stat.starttime) >= minimum
            && !DEAD_STATES.contains(&stat.state.as_str())
            && same_user_at(proc_root, stat.pid)
            && carries_generation_at(proc_root, stat.pid, generation)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Read-only proof for one recorded process: `None` when provably gone.
/// Returns a description when it may still run, and errors when the
/// evidence cannot be read or trusted.
pub fn recorded_process_alive(
    label: &str,
    identity: &ProcessIdentity,
) -> Result<Option<String>, ForeignOwnershipError> {
    recorded_process_alive_at(Path::new(PROC), label, identity)
}

/// [`recorded_process_alive`] against an explicit procfs root.
pub fn recorded_process_alive_at(
    proc_root: &Path,
    label: &str,
    identity: &ProcessIdentity,
) -> Result<Option<String>, ForeignOwnershipError> {
    let unknown = |message: String| ForeignOwnershipError::new("runner_identity_unknown", message);
    if let Some(boot_id) = &identity.boot_id {
        if !valid_boot_id(boot_id) {
            return Err(unknown(format!("{label} has a malformed boot identity")));
        }
        if *boot_id != current_boot_id_at(proc_root)? {
            return Ok(None);
        }
    }
    let Some(current) = read_stat_at(proc_root, identity.pid)? else {
        return Ok(None);
    };
    if DEAD_STATES.contains(&current.state.as_str()) {
        return Ok(None);
    }
    let Some(started_at) = &identity.started_at else {
        return Err(unknown(format!(
            "{label} (pid {}) exists and has no recorded start time, so it may still be that process",
            identity.pid
        )));
    };
    if !valid_ticks(started_at) {
        return Err(unknown(format!(
            "{label} has an unreadable start time '{started_at}'"
        )));
    }
    if *started_at != current.starttime.to_string() {
        return Ok(None);
    }
    Ok(Some(format!(
        "{label} (pid {}, start tick {}, state {}) is still running",
        identity.pid, current.starttime, current.state
    )))
}

/// Read-only: describe a live member of a recorded harness tree, or `None`.
/// Only the per-turn cgroup and the generation prove membership without a
/// live leader.
pub fn tree_alive(record: &HarnessRecord) -> Result<Option<String>, ForeignOwnershipError> {
    tree_alive_at(Path::new(PROC), record)
}

/// [`tree_alive`] against an explicit procfs root.
pub fn tree_alive_at(
    proc_root: &Path,
    record: &HarnessRecord,
) -> Result<Option<String>, ForeignOwnershipError> {
    if record.legacy() || record.leader.boot_id.as_deref() != Some(&current_boot_id_at(proc_root)?)
    {
        return Ok(None);
    }
    if let Some(cgroup) = &record.cgroup {
        if cgroup.is_dir() {
            let populated = cgroup_populated(cgroup).map_err(|error| {
                ForeignOwnershipError::new(
                    "harness_identity_unknown",
                    format!("cannot read {}/cgroup.events: {error}", cgroup.display()),
                )
            })?;
            match populated {
                Some(true) => {
                    return Ok(Some(format!(
                        "its harness cgroup {} still has processes",
                        cgroup.display()
                    )));
                }
                Some(false) => {}
                None => {
                    return Err(ForeignOwnershipError::new(
                        "harness_identity_unknown",
                        format!("{}/cgroup.events is malformed", cgroup.display()),
                    ));
                }
            }
        }
    }
    let Some(generation) = record.generation.as_deref() else {
        return Ok(None);
    };
    let minimum = minimum_start(record);
    for stat in scan_processes_at(proc_root)? {
        if !DEAD_STATES.contains(&stat.state.as_str())
            && u128::from(stat.starttime) >= minimum
            && carries_generation_at(proc_root, stat.pid, generation)
        {
            return Ok(Some(format!(
                "pid {} of its harness tree (generation {generation}) is still running",
                stat.pid
            )));
        }
    }
    Ok(None)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proof {
    Session,
    Child,
    Generation,
}

fn pin_member_at(
    proc_root: &Path,
    stat: &ProcessStat,
    reason: Proof,
    record: &HarnessRecord,
) -> Option<PinnedProcess> {
    let descriptor = open_pidfd(stat.pid).ok()??;
    let process = PinnedProcess {
        pid: stat.pid,
        starttime: stat.starttime,
        descriptor,
    };
    let after = quiet_stat_at(proc_root, stat.pid)?;
    let proven = after.starttime == stat.starttime
        && !DEAD_STATES.contains(&after.state.as_str())
        && match reason {
            Proof::Session => Some(after.session) == record.process_group,
            Proof::Child => after.ppid == stat.ppid,
            Proof::Generation => record
                .generation
                .as_deref()
                .is_some_and(|generation| carries_generation_at(proc_root, stat.pid, generation)),
        }
        && !process.exited();
    proven.then_some(process)
}

type Members = Vec<((i64, u64), PinnedProcess)>;

enum LeaderHandle<'a> {
    Borrowed(&'a ProcessHandle),
    Owned(PinnedProcess),
}

impl LeaderHandle<'_> {
    fn pid(&self) -> i64 {
        match self {
            Self::Borrowed(handle) => handle.pid(),
            Self::Owned(process) => process.pid(),
        }
    }

    fn starttime(&self) -> u64 {
        match self {
            Self::Borrowed(handle) => handle.starttime(),
            Self::Owned(process) => process.starttime(),
        }
    }

    fn exited(&self) -> bool {
        match self {
            Self::Borrowed(handle) => handle.exited(),
            Self::Owned(process) => process.exited(),
        }
    }

    fn wait(&self, timeout: Option<Duration>) -> bool {
        match self {
            Self::Borrowed(handle) => handle.wait(timeout),
            Self::Owned(process) => process.wait(timeout),
        }
    }

    fn send(&self, signum: i32) -> bool {
        match self {
            Self::Borrowed(handle) => handle.send(signum),
            Self::Owned(process) => process.send(signum),
        }
    }
}

struct Census<'a> {
    proc_root: &'a Path,
    record: &'a HarnessRecord,
    leader: Option<&'a LeaderHandle<'a>>,
    leader_unreaped: bool,
    runner_pid: i64,
    runner_is_self: bool,
    runner_handle: Option<&'a PinnedProcess>,
    minimum: u128,
    me: i64,
}

impl Census<'_> {
    fn leader_present(&self) -> bool {
        self.leader
            .is_some_and(|leader| self.leader_unreaped || !leader.exited())
    }

    fn runner_present(&self) -> bool {
        if self.runner_is_self {
            is_child_subreaper()
        } else {
            self.runner_handle.is_some_and(|runner| !runner.exited())
        }
    }

    fn parent_present(&self, members: &Members, ppid: i64) -> bool {
        if ppid == self.runner_pid {
            return self.runner_present();
        }
        if self.leader.is_some_and(|leader| ppid == leader.pid()) {
            return self.leader_present();
        }
        members
            .iter()
            .find(|(key, _)| key.0 == ppid)
            .is_some_and(|(_, handle)| !handle.exited())
    }

    #[allow(clippy::too_many_arguments)]
    fn consider(
        &self,
        stat: &ProcessStat,
        members: &mut Members,
        unproven: &mut HashSet<i64>,
        found: &mut usize,
        stop: bool,
    ) -> bool {
        if members
            .iter()
            .any(|(key, _)| key == &(stat.pid, stat.starttime))
        {
            return true;
        }
        if stat.pid == self.me || DEAD_STATES.contains(&stat.state.as_str()) {
            return false;
        }
        if self
            .leader
            .is_some_and(|leader| stat.pid == leader.pid() && stat.starttime == leader.starttime())
        {
            return false;
        }
        if stat.pid == self.runner_pid
            && (self.runner_is_self
                || self.record.runner.started_at.as_deref()
                    == Some(stat.starttime.to_string().as_str()))
        {
            return false;
        }
        let reason = if self.leader_present() && Some(stat.session) == self.record.process_group {
            Some(Proof::Session)
        } else if self.parent_present(members, stat.ppid) {
            Some(Proof::Child)
        } else if self.record.generation.as_deref().is_some_and(|generation| {
            u128::from(stat.starttime) >= self.minimum
                && same_user_at(self.proc_root, stat.pid)
                && carries_generation_at(self.proc_root, stat.pid, generation)
        }) {
            Some(Proof::Generation)
        } else {
            None
        };
        let Some(reason) = reason else {
            if Some(stat.session) == self.record.process_group && !self.leader_present() {
                unproven.insert(stat.pid);
            }
            return false;
        };
        let Some(handle) = pin_member_at(self.proc_root, stat, reason, self.record) else {
            return false;
        };
        // The session and parent proofs hold only while their anchor still
        // holds its number; check after the member's own re-check.
        if (reason == Proof::Session && !self.leader_present())
            || (reason == Proof::Child && !self.parent_present(members, stat.ppid))
        {
            return false;
        }
        members.push(((stat.pid, stat.starttime), handle));
        if stop {
            members
                .last()
                .expect("member just pushed")
                .1
                .send(libc::SIGSTOP);
        }
        *found += 1;
        true
    }

    fn round(
        &self,
        members: &mut Members,
        report: &mut CleanupReport,
        stop: bool,
        scan: bool,
    ) -> Result<usize, ForeignOwnershipError> {
        let mut found = 0;
        let mut unproven = HashSet::new();
        let mut seen = HashSet::new();
        let mut anchors: Vec<i64> = members.iter().map(|(key, _)| key.0).collect();
        if self.runner_present() {
            anchors.push(self.runner_pid);
        }
        if self.leader_present() {
            if let Some(leader) = self.leader {
                anchors.push(leader.pid());
            }
        }
        let mut walked = HashSet::new();
        let mut complete_walk = true;
        while let Some(parent) = anchors.pop() {
            if !walked.insert(parent) {
                continue;
            }
            let Some(children) = children_of_at(self.proc_root, parent) else {
                complete_walk = false;
                continue;
            };
            for child in children {
                if !seen.insert(child) {
                    continue;
                }
                if let Some(stat) = quiet_stat_at(self.proc_root, child) {
                    if self.consider(&stat, members, &mut unproven, &mut found, stop) {
                        anchors.push(child);
                    }
                }
            }
        }
        if scan || !complete_walk {
            for stat in scan_processes_at(self.proc_root)? {
                if seen.insert(stat.pid) {
                    self.consider(&stat, members, &mut unproven, &mut found, stop);
                }
            }
        }
        let mut unproven: Vec<i64> = unproven.into_iter().collect();
        unproven.sort_unstable();
        for pid in unproven {
            let entry =
                format!("pid {pid} (the exited leader's session, without this turn's generation)");
            if !report.unproven.contains(&entry) {
                report.unproven.push(entry);
            }
        }
        Ok(found)
    }
}

fn peel_children_at(
    proc_root: &Path,
    leader: &ChildHandle,
    report: &mut CleanupReport,
    deadline: Instant,
) -> Result<(), ForeignOwnershipError> {
    report.converged = false;
    let live_children = |proc_root: &Path| -> Vec<i64> {
        let candidates = if is_child_subreaper() {
            children_of_self_at(proc_root)
        } else {
            vec![leader.pid()]
        };
        candidates
            .into_iter()
            .filter(|pid| {
                quiet_stat_at(proc_root, *pid)
                    .is_some_and(|stat| !DEAD_STATES.contains(&stat.state.as_str()))
            })
            .collect()
    };
    while Instant::now() < deadline {
        let live = live_children(proc_root);
        if live.is_empty() {
            report.converged = true;
            break;
        }
        for pid in live {
            if pid == leader.pid() {
                if leader.send(libc::SIGKILL) {
                    report.killed += 1;
                }
            } else {
                // Parent-pinned: an unreaped adopted child keeps its number.
                // SAFETY: kill takes only this adopted child's pid.
                if unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) } >= 0 {
                    report.killed += 1;
                } else {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(ForeignOwnershipError::new(
                            "runner_identity_unknown",
                            format!("cannot signal adopted child pid {pid}: {error}"),
                        ));
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
        reap_adopted_children_at(proc_root, &[leader.pid()]);
    }
    for pid in live_children(proc_root) {
        report.remaining.push(describe_handle_at(proc_root, pid));
    }
    Ok(())
}

/// Kill every proven member of one turn's harness tree, bounded in time.
///
/// Inside the runner, pass the runner's own `leader` handle, set
/// `runner_is_self`, and set `leader_unreaped` while the runner has not
/// reaped the leader (its zombie still holds the session number). Other
/// callers pass neither; the leader and runner are pinned from the record.
pub fn kill_harness_tree(
    record: &HarnessRecord,
    leader: Option<&ProcessHandle>,
    leader_unreaped: bool,
    runner_is_self: bool,
    wait: Option<Duration>,
) -> Result<CleanupReport, ForeignOwnershipError> {
    kill_harness_tree_at(
        Path::new(PROC),
        record,
        leader,
        leader_unreaped,
        runner_is_self,
        wait,
    )
}

/// [`kill_harness_tree`] against an explicit procfs root. Pidfds and signals
/// always name real processes.
#[allow(clippy::too_many_arguments)]
pub fn kill_harness_tree_at(
    proc_root: &Path,
    record: &HarnessRecord,
    leader: Option<&ProcessHandle>,
    leader_unreaped: bool,
    runner_is_self: bool,
    wait: Option<Duration>,
) -> Result<CleanupReport, ForeignOwnershipError> {
    if record.legacy() {
        return Err(ForeignOwnershipError::new(
            "harness_identity_legacy",
            "a legacy harness record carries no generation to prove its tree".to_owned(),
        ));
    }
    let bound = wait.unwrap_or_else(default_kill_wait);
    let started = Instant::now();
    let deadline = started
        .checked_add(bound)
        .unwrap_or_else(|| started + Duration::from_secs(60 * 60 * 24 * 365));
    let mut report = CleanupReport::new(record.containment);
    let boot = current_boot_id_at(proc_root)?;
    if record.leader.boot_id.as_deref() != Some(boot.as_str()) {
        return Ok(report);
    }
    if let Some(cgroup) = &record.cgroup {
        if cgroup_relative_at(proc_root, cgroup).is_none() {
            return Err(ForeignOwnershipError::new(
                "harness_identity_unknown",
                format!(
                    "recorded harness cgroup {} is not inside the cgroup v2 hierarchy",
                    cgroup.display()
                ),
            ));
        }
        let (killed, remaining) = kill_cgroup_at(proc_root, cgroup, deadline)?;
        report.killed += killed;
        report.remaining.extend(remaining);
    }
    if !pidfds_supported() {
        if runner_is_self {
            if let Some(ProcessHandle::Child(child)) = leader {
                peel_children_at(proc_root, child, &mut report, deadline)?;
                return Ok(report);
            }
        }
        if tree_has_candidates_at(proc_root, record)? {
            return Err(ForeignOwnershipError::new(
                "pidfd_unsupported",
                format!(
                    "harness pid {} may still have live members, and this host does not provide pidfds to signal them safely",
                    record.leader.pid
                ),
            ));
        }
        return Ok(report);
    }

    let leader_handle: Option<LeaderHandle<'_>> = match leader {
        Some(handle) => Some(LeaderHandle::Borrowed(handle)),
        None => {
            let pinned = pin_at(proc_root, &record.leader, None)?;
            match pinned.process {
                Some(process) => {
                    let current = quiet_stat_at(proc_root, process.pid());
                    if current.is_some_and(|current| Some(current.session) != record.process_group)
                    {
                        return Err(ForeignOwnershipError::new(
                            "harness_identity_changed",
                            format!(
                                "harness pid {} no longer leads its recorded session {}",
                                record.leader.pid,
                                record
                                    .process_group
                                    .map_or_else(|| "None".to_owned(), |group| group.to_string())
                            ),
                        ));
                    }
                    Some(LeaderHandle::Owned(process))
                }
                None => None,
            }
        }
    };
    let runner_handle: Option<PinnedProcess> = if runner_is_self {
        None
    } else {
        pin_at(proc_root, &record.runner, None)?.process
    };
    let runner_pid = if runner_is_self {
        i64::from(std::process::id())
    } else {
        record.runner.pid
    };
    let census = Census {
        proc_root,
        record,
        leader: leader_handle.as_ref(),
        leader_unreaped,
        runner_pid,
        runner_is_self,
        runner_handle: runner_handle.as_ref(),
        minimum: minimum_start(record),
        me: i64::from(std::process::id()),
    };
    let census_no_leader = Census {
        proc_root,
        record,
        leader: None,
        leader_unreaped: false,
        runner_pid,
        runner_is_self,
        runner_handle: runner_handle.as_ref(),
        minimum: minimum_start(record),
        me: i64::from(std::process::id()),
    };

    let mut members: Members = Vec::new();
    let mut signalled: HashSet<(i64, u64)> = HashSet::new();
    let mut pending_error: Option<ForeignOwnershipError> = None;

    if let Some(leader) = &leader_handle {
        if !leader.exited() {
            leader.send(libc::SIGSTOP);
        }
    }
    let census_deadline = started
        .checked_add(bound / 2)
        .unwrap_or_else(|| started + Duration::from_secs(60 * 60 * 24 * 365));
    report.converged = false;
    let scan = !runner_is_self || !is_child_subreaper();

    'outer: loop {
        let mut first = true;
        'census: loop {
            match census.round(&mut members, &mut report, true, scan && first) {
                Ok(0) => {
                    report.converged = true;
                    break 'census;
                }
                Ok(_) => {}
                Err(error) => {
                    pending_error = Some(error);
                    break 'outer;
                }
            }
            first = false;
            if Instant::now() >= census_deadline {
                report.converged = false;
                break 'census;
            }
        }
        for (key, handle) in &members {
            if signalled.insert(*key) && handle.send(libc::SIGKILL) {
                report.killed += 1;
            }
        }
        if let Some(leader) = &leader_handle {
            let key = (leader.pid(), leader.starttime());
            if signalled.insert(key) && !leader.exited() && leader.send(libc::SIGKILL) {
                report.killed += 1;
            }
        }
        for (_, handle) in &members {
            handle.wait(Some(deadline.saturating_duration_since(Instant::now())));
        }
        if let Some(leader) = &leader_handle {
            leader.wait(Some(deadline.saturating_duration_since(Instant::now())));
        }
        let before = members.len();
        if let Err(error) = census_no_leader.round(&mut members, &mut report, true, scan) {
            pending_error = Some(error);
            break 'outer;
        }
        if members.len() == before || Instant::now() >= deadline {
            break 'outer;
        }
    }

    if let Some(error) = pending_error {
        // An unexpected failure must not leave proven members stopped.
        for (key, handle) in &members {
            if !signalled.contains(key) {
                handle.send(libc::SIGKILL);
            }
        }
        if let Some(leader) = &leader_handle {
            let key = (leader.pid(), leader.starttime());
            if !signalled.contains(&key) && !leader.exited() {
                leader.send(libc::SIGKILL);
            }
        }
        return Err(error);
    }

    for (_, handle) in &members {
        if !handle.exited() {
            report
                .remaining
                .push(describe_handle_at(proc_root, handle.pid()));
        }
    }
    if let Some(leader) = &leader_handle {
        if !leader.exited() {
            report
                .remaining
                .push(describe_handle_at(proc_root, leader.pid()));
        }
    }
    if runner_is_self {
        let exclude: Vec<i64> = leader_handle
            .as_ref()
            .map(|leader| vec![leader.pid()])
            .unwrap_or_default();
        reap_adopted_children_at(proc_root, &exclude);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BOOT: &str = "123e4567-e89b-42d3-a456-426614174000";
    const BOOT2: &str = "223e4567-e89b-42d3-a456-426614174000";
    const GEN: &str = "0123456789abcdef0123456789abcdef";

    fn v1(extra: serde_json::Value) -> serde_json::Value {
        let mut base = json!({
            "schema": HARNESS_SCHEMA,
            "runner_pid": 100,
            "runner_started_at": "111",
            "runner_boot_id": BOOT2,
            "pid": 200,
            "started_at": "222",
            "boot_id": BOOT,
            "process_group": 200,
            "generation": GEN,
            "containment": "process",
            "recorded_at": "2026-11-07T00:00:00Z",
        });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    }

    #[test]
    fn valid_boot_id_accepts_canonical_uuid() {
        assert!(valid_boot_id(BOOT));
        assert!(!valid_boot_id("123E4567-e89b-42d3-a456-426614174000"));
        assert!(!valid_boot_id("123e4567e89b42d3a456426614174000"));
        assert!(!valid_boot_id("123e4567-e89b-42d3-a456-42661417400"));
        assert!(!valid_boot_id(""));
    }

    #[test]
    fn valid_generation_and_ticks() {
        assert!(valid_generation(GEN));
        assert!(!valid_generation("0123456789ABCDEF0123456789abcdef"));
        assert!(!valid_generation("abc"));
        assert!(valid_ticks("0"));
        assert!(valid_ticks("12345678901234567890"));
        assert!(!valid_ticks("01"));
        assert!(!valid_ticks(""));
        assert!(!valid_ticks("123456789012345678901"));
        assert!(!valid_ticks("-1"));
    }

    #[test]
    fn parses_legacy_record_without_schema() {
        let rec =
            parse_harness_record(&json!({"pid": 200, "started_at": "222", "runner_pid": 100}))
                .unwrap();
        assert!(rec.legacy());
        assert_eq!(rec.leader.pid, 200);
        assert_eq!(rec.runner.pid, 100);
        assert_eq!(rec.process_group, Some(200));
        assert_eq!(rec.generation, None);
    }

    #[test]
    fn parses_v1_process_record() {
        let rec = parse_harness_record(&v1(json!({}))).unwrap();
        assert!(!rec.legacy());
        assert_eq!(rec.containment, Containment::Process);
        assert_eq!(rec.leader.boot_id.as_deref(), Some(BOOT));
        assert_eq!(rec.runner.boot_id.as_deref(), Some(BOOT2));
        assert_eq!(rec.generation.as_deref(), Some(GEN));
    }

    #[test]
    fn parses_v1_cgroup_record() {
        let path = format!("/sys/fs/cgroup/agentctl-harness-{GEN}");
        let rec =
            parse_harness_record(&v1(json!({"containment": "cgroup", "cgroup": path}))).unwrap();
        assert_eq!(rec.containment, Containment::Cgroup);
        assert_eq!(
            rec.cgroup,
            Some(PathBuf::from(format!(
                "/sys/fs/cgroup/agentctl-harness-{GEN}"
            )))
        );
    }

    #[test]
    fn document_round_trips_through_parse() {
        let rec = parse_harness_record(&v1(json!({}))).unwrap();
        let doc = rec.document("2026-11-07T00:00:00Z");
        assert_eq!(doc["schema"], json!(HARNESS_SCHEMA));
        assert_eq!(doc["containment"], json!("process"));
        let again = parse_harness_record(&doc).unwrap();
        assert_eq!(again, rec);
    }

    #[test]
    fn rejects_non_object() {
        let err = parse_harness_record(&json!([1, 2, 3])).unwrap_err();
        assert_eq!(err.code, "harness_identity_unknown");
        assert!(err.message.contains("not an object"));
    }

    #[test]
    fn rejects_bad_pid_and_started_at() {
        for v in [
            json!({"pid": 1, "started_at": "222", "runner_pid": 100}),
            json!({"pid": 200, "runner_pid": 100}),
            json!({"pid": "200", "started_at": "222", "runner_pid": 100}),
            json!({"pid": true, "started_at": "222", "runner_pid": 100}),
            json!({"pid": 200, "started_at": "222"}),
        ] {
            assert!(parse_harness_record(&v).is_err(), "expected rejection: {v}");
        }
    }

    #[test]
    fn rejects_unsupported_schema() {
        let err = parse_harness_record(&v1(json!({"schema": "other/v9"}))).unwrap_err();
        assert!(err.message.contains("unsupported schema"));
    }

    #[test]
    fn rejects_bad_boot_generation_group_containment() {
        for extra in [
            json!({"boot_id": "not-a-uuid"}),
            json!({"runner_boot_id": "not-a-uuid"}),
            json!({"generation": "xyz"}),
            json!({"process_group": 0}),
            json!({"containment": "weird"}),
        ] {
            assert!(parse_harness_record(&v1(extra)).is_err());
        }
    }

    #[test]
    fn rejects_cgroup_path_mismatch() {
        for extra in [
            json!({"containment": "cgroup"}),
            json!({"containment": "cgroup", "cgroup": "relative/agentctl-harness-0123456789abcdef0123456789abcdef"}),
            json!({"containment": "cgroup", "cgroup": "/sys/fs/cgroup/agentctl-harness-deadbeef"}),
            json!({"containment": "cgroup", "cgroup": format!("/sys/fs/cgroup/../agentctl-harness-{GEN}")}),
        ] {
            assert!(
                parse_harness_record(&v1(extra.clone())).is_err(),
                "expected rejection: {extra:?}"
            );
        }
    }

    #[test]
    fn rejects_process_containment_with_cgroup() {
        assert!(parse_harness_record(&v1(json!({"cgroup": "/sys/fs/cgroup/x"}))).is_err());
    }

    fn fake_proc(test: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("agentctl-b3-2-{}-{test}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sys/kernel/random")).unwrap();
        root
    }

    fn stat_record(pid: i64, starttime: u64) -> String {
        let mut tail: Vec<String> = vec![
            "S".to_owned(),
            "10".to_owned(),
            "20".to_owned(),
            "20".to_owned(),
        ];
        tail.extend(std::iter::repeat_n("0".to_owned(), 15));
        tail.push(starttime.to_string());
        tail.extend(std::iter::repeat_n("0".to_owned(), 30));
        format!("{pid} (fake proc) {}\n", tail.join(" "))
    }

    fn write_stat(root: &Path, dir_pid: i64, record: &str) {
        let dir = root.join(dir_pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), record).unwrap();
    }

    #[test]
    fn current_boot_id_reads_only_canonical_evidence() {
        let root = fake_proc("boot");
        let boot_file = root.join("sys/kernel/random/boot_id");
        std::fs::write(&boot_file, format!("{BOOT}\n")).unwrap();
        assert_eq!(current_boot_id_at(&root).unwrap(), BOOT);

        std::fs::write(&boot_file, "not-a-uuid\n").unwrap();
        let err = current_boot_id_at(&root).unwrap_err();
        assert_eq!(err.code, "runner_identity_unknown");
        assert!(err.message.contains("malformed"), "{}", err.message);

        // Over the byte limit is malformed even when the text would parse.
        std::fs::write(&boot_file, format!("{BOOT}{}\n", " ".repeat(200))).unwrap();
        assert!(current_boot_id_at(&root).is_err());

        std::fs::remove_file(&boot_file).unwrap();
        let err = current_boot_id_at(&root).unwrap_err();
        assert!(err.message.contains("cannot read"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_stat_distinguishes_gone_from_malformed() {
        let root = fake_proc("stat");
        write_stat(&root, 4242, &stat_record(4242, 987654));
        let stat = read_stat_at(&root, 4242).unwrap().unwrap();
        assert_eq!(stat.pid, 4242);
        assert_eq!(stat.starttime, 987654);

        // Positive disappearance is the only `None`.
        assert_eq!(read_stat_at(&root, 7777).unwrap(), None);

        write_stat(&root, 5555, "garbage");
        let err = read_stat_at(&root, 5555).unwrap_err();
        assert_eq!(err.code, "runner_identity_unknown");
        assert!(err.message.contains("cannot parse"), "{}", err.message);

        // A record naming a different pid is malformed evidence, not `None`.
        write_stat(&root, 6666, &stat_record(9999, 1));
        assert!(read_stat_at(&root, 6666).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_processes_skips_the_unreadable() {
        let root = fake_proc("scan");
        write_stat(&root, 4242, &stat_record(4242, 987654));
        write_stat(&root, 99, "garbage");
        std::fs::create_dir_all(root.join("55")).unwrap();
        std::fs::create_dir_all(root.join("self")).unwrap();
        let found = scan_processes_at(&root).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].pid, 4242);

        let err = scan_processes_at(&root.join("missing")).unwrap_err();
        assert!(err.message.contains("cannot list"), "{}", err.message);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_procfs_evidence_names_this_process() {
        let boot = current_boot_id().unwrap();
        assert!(valid_boot_id(&boot));
        let own = i64::from(std::process::id());
        let stat = read_stat(own).unwrap().unwrap();
        assert_eq!(stat.pid, own);
        assert!(scan_processes().unwrap().iter().any(|s| s.pid == own));
    }

    #[cfg(target_os = "linux")]
    fn spawn_sleep() -> std::process::Child {
        std::process::Command::new("sleep")
            .arg("10")
            .spawn()
            .expect("spawn sleep child")
    }

    #[cfg(target_os = "linux")]
    fn identity_of(pid: i64) -> ProcessIdentity {
        let stat = read_stat(pid).unwrap().unwrap();
        ProcessIdentity::new(
            pid,
            Some(stat.starttime.to_string()),
            Some(current_boot_id().unwrap()),
        )
    }

    #[cfg(target_os = "linux")]
    fn reap(child: &mut std::process::Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pin_requires_the_recorded_boot_and_start_tick() {
        if !pidfds_supported() {
            return;
        }
        let mut child = spawn_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);

        let pinned = pin(&exact, None).unwrap();
        assert_eq!(pinned.verdict, PinVerdict::Alive);
        let process = pinned.process.unwrap();
        assert_eq!(process.pid(), pid);
        assert!(!process.exited());
        process.close();

        let stale = ProcessIdentity::new(
            pid,
            Some((exact.started_at.as_deref().unwrap().parse::<u64>().unwrap() + 1).to_string()),
            exact.boot_id.clone(),
        );
        assert_eq!(pin(&stale, None).unwrap().verdict, PinVerdict::Gone);

        let other_boot = ProcessIdentity::new(
            pid,
            exact.started_at.clone(),
            Some("00000000-0000-4000-8000-000000000000".to_owned()),
        );
        let result = pin(&other_boot, None).unwrap();
        assert_eq!(result.verdict, PinVerdict::Gone);
        assert!(result.reason.contains("earlier boot"), "{}", result.reason);

        let legacy = ProcessIdentity::new(pid, exact.started_at.clone(), None);
        assert_eq!(pin(&legacy, None).unwrap().verdict, PinVerdict::Unproven);
        let proof = |candidate: i64, _stat: &ProcessStat| candidate == pid;
        let proven = pin(&legacy, Some(&proof)).unwrap();
        assert_eq!(proven.verdict, PinVerdict::Alive);
        proven.process.unwrap().close();

        let without_start = ProcessIdentity::new(pid, None, exact.boot_id.clone());
        assert_eq!(
            pin(&without_start, None).unwrap().verdict,
            PinVerdict::Unproven
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );
        reap(&mut child);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pin_rejects_invalid_recorded_values() {
        for identity in [
            ProcessIdentity::new(1, Some("1".to_owned()), Some(BOOT.to_owned())),
            ProcessIdentity::new(4242, Some("1".to_owned()), Some("nope".to_owned())),
            // The boot check runs before the ticks check (as in Python), so
            // the unreadable-ticks case needs this host's real boot identity.
            ProcessIdentity::new(
                4242,
                Some("01".to_owned()),
                Some(current_boot_id().unwrap()),
            ),
        ] {
            assert_eq!(
                pin(&identity, None).unwrap().verdict,
                PinVerdict::Unproven,
                "identity: {identity:?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stop_process_never_signals_a_reused_pid_or_an_earlier_boot() {
        if !pidfds_supported() {
            return;
        }
        let mut child = spawn_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        for recorded in [
            ProcessIdentity::new(
                pid,
                Some(
                    (exact.started_at.as_deref().unwrap().parse::<u64>().unwrap() - 1).to_string(),
                ),
                exact.boot_id.clone(),
            ),
            ProcessIdentity::new(
                pid,
                exact.started_at.clone(),
                Some("00000000-0000-4000-8000-000000000000".to_owned()),
            ),
        ] {
            assert!(!stop_process(&recorded, Duration::ZERO, None, None, "runner").unwrap());
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );
        reap(&mut child);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stop_process_terminates_then_kills_through_the_pidfd() {
        use std::io::BufRead as _;
        use std::os::unix::process::ExitStatusExt as _;
        if !pidfds_supported() {
            return;
        }
        // A cooperative child dies on SIGTERM.
        let mut coop = spawn_sleep();
        let coop_identity = identity_of(i64::from(coop.id()));
        assert!(
            stop_process(&coop_identity, Duration::from_secs(2), None, None, "runner").unwrap()
        );
        assert_eq!(coop.wait().unwrap().signal(), Some(libc::SIGTERM));

        // A stubborn child ignores SIGTERM and dies on SIGKILL after grace.
        let mut stubborn = std::process::Command::new("bash")
            .args([
                "-c",
                "trap '' TERM; echo ready; while true; do sleep 1; done",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn stubborn child");
        let mut line = String::new();
        std::io::BufReader::new(stubborn.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        let stubborn_identity = identity_of(i64::from(stubborn.id()));
        let started = Instant::now();
        assert!(stop_process(
            &stubborn_identity,
            Duration::from_millis(300),
            None,
            Some(Duration::from_secs(5)),
            "runner"
        )
        .unwrap());
        assert_eq!(stubborn.wait().unwrap().signal(), Some(libc::SIGKILL));
        assert!(started.elapsed() >= Duration::from_millis(300));
    }

    #[cfg(target_os = "linux")]
    fn spawn_session_sleep() -> std::process::Child {
        std::process::Command::new("setsid")
            .args(["sleep", "10"])
            .spawn()
            .expect("spawn session leader")
    }

    #[cfg(target_os = "linux")]
    fn harness_record(leader: ProcessIdentity, session: i64, generation: &str) -> HarnessRecord {
        HarnessRecord {
            runner: ProcessIdentity::new(1, Some("1".to_owned()), Some(current_boot_id().unwrap())),
            leader,
            process_group: Some(session),
            generation: Some(generation.to_owned()),
            containment: Containment::Process,
            cgroup: None,
        }
    }

    #[cfg(target_os = "linux")]
    fn process_gone(pid: i64) -> bool {
        read_stat(pid).map_or(true, |stat| {
            stat.is_none_or(|stat| DEAD_STATES.contains(&stat.state.as_str()))
        })
    }

    #[cfg(target_os = "linux")]
    fn wait_gone(pid: i64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if process_gone(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        process_gone(pid)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_handle_signals_only_its_unreaped_child() {
        use std::os::unix::process::ExitStatusExt as _;
        let mut child = spawn_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        let handle = ChildHandle::new(
            pid,
            exact.started_at.as_deref().unwrap().parse().unwrap(),
            || false,
        );
        assert!(!handle.exited());
        assert!(handle.send(libc::SIGTERM));
        assert!(handle.wait(Some(Duration::from_secs(2))));
        handle.close();
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cgroup_discovery_and_fake_kill_file() {
        let root = fake_proc("cgroup");
        let mount = root.join("cg");
        std::fs::create_dir_all(mount.join("user.slice/session.scope")).unwrap();
        std::fs::create_dir_all(root.join("self")).unwrap();
        std::fs::write(
            root.join("self/mountinfo"),
            format!(
                "36 35 0:32 / {} rw,relatime - cgroup2 cgroup rw\n",
                mount.display()
            ),
        )
        .unwrap();
        std::fs::write(root.join("self/cgroup"), "0::/user.slice/session.scope\n").unwrap();
        assert_eq!(
            own_cgroup_directory_at(&root),
            Some(mount.join("user.slice/session.scope"))
        );
        assert_eq!(
            cgroup_relative_at(&root, &mount.join("user.slice/session.scope")).as_deref(),
            Some("/user.slice/session.scope")
        );
        assert_eq!(cgroup_relative_at(&root, &root.join("outside")), None);
        assert_eq!(cgroup_mode_from(None).unwrap(), CgroupMode::Off);
        assert_eq!(
            cgroup_mode_from(Some("require")).unwrap(),
            CgroupMode::Require
        );
        assert_eq!(
            cgroup_mode_from(Some("sometimes")).unwrap_err().code,
            "invalid_cgroup_mode"
        );

        // A populated fake cgroup receives the kill write; its (fake)
        // membership is reported remaining because populated stays 1.
        let owned = mount.join(format!("{CGROUP_PREFIX}{GEN}"));
        std::fs::create_dir_all(&owned).unwrap();
        let me = std::process::id();
        std::fs::write(owned.join("cgroup.events"), "populated 1\n").unwrap();
        std::fs::write(owned.join("cgroup.procs"), format!("{me}\n")).unwrap();
        std::fs::write(owned.join("cgroup.kill"), "").unwrap();
        let (named, remaining) =
            kill_cgroup_at(&root, &owned, Instant::now() + Duration::from_millis(50)).unwrap();
        assert_eq!(named, 1);
        assert_eq!(
            std::fs::read_to_string(owned.join("cgroup.kill")).unwrap(),
            "1\n"
        );
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].contains(&format!("pid {me}")), "{remaining:?}");

        // An empty fake cgroup names nobody and reports nobody.
        std::fs::write(owned.join("cgroup.events"), "populated 0\n").unwrap();
        let (_named, remaining) =
            kill_cgroup_at(&root, &owned, Instant::now() + Duration::from_millis(50)).unwrap();
        assert!(remaining.is_empty());
        assert_eq!(
            kill_cgroup_at(&root, &root.join("missing"), Instant::now()).unwrap(),
            (0, Vec::new())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kill_tree_refuses_legacy_and_ignores_an_earlier_boot() {
        let mut child = spawn_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        let session = read_stat(pid).unwrap().unwrap().session;

        let legacy = HarnessRecord {
            containment: Containment::Legacy,
            ..harness_record(exact.clone(), session, GEN)
        };
        let err = kill_harness_tree(&legacy, None, false, false, Some(Duration::from_secs(1)))
            .unwrap_err();
        assert_eq!(err.code, "harness_identity_legacy");

        let earlier = harness_record(
            ProcessIdentity::new(
                pid,
                exact.started_at.clone(),
                Some("00000000-0000-4000-8000-000000000000".to_owned()),
            ),
            session,
            GEN,
        );
        let report =
            kill_harness_tree(&earlier, None, false, false, Some(Duration::from_secs(1))).unwrap();
        assert!(
            report.complete() && report.killed == 0,
            "{}",
            report.describe()
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );
        reap(&mut child);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_recycled_session_number_is_never_signalled() {
        if !pidfds_supported() {
            return;
        }
        let mut child = spawn_session_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        let stale = ProcessIdentity::new(
            pid,
            Some((exact.started_at.as_deref().unwrap().parse::<u64>().unwrap() - 1).to_string()),
            exact.boot_id.clone(),
        );
        let record = harness_record(stale, pid, GEN);
        let report =
            kill_harness_tree(&record, None, false, false, Some(Duration::from_secs(1))).unwrap();
        assert!(
            report.complete() && report.killed == 0,
            "{}",
            report.describe()
        );
        assert!(
            report
                .unproven
                .iter()
                .any(|entry| entry.contains(&format!("pid {pid}"))),
            "{:?}",
            report.unproven
        );
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );
        reap(&mut child);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn external_cleanup_reaches_session_and_generation_members() {
        use std::os::unix::process::ExitStatusExt as _;
        if !pidfds_supported() {
            return;
        }
        const TREE_GEN: &str = "fedcba9876543210fedcba9876543210";
        let root = std::env::temp_dir().join(format!("agentctl-b3-5-{}-tree", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let out = root.join("pids");
        let mut leader = std::process::Command::new("setsid")
            .args([
                "bash",
                "-c",
                "sleep 10 & c1=$!; setsid sleep 10 & c2=$!; printf '%s %s %s\n' \"$$\" \"$c1\" \"$c2\" > \"$1\"; wait",
                "bash",
                out.to_string_lossy().as_ref(),
            ])
            .env(GENERATION_ENV, TREE_GEN)
            .spawn()
            .expect("spawn tree leader");
        let mut pids = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&out) {
                pids = text
                    .split_whitespace()
                    .filter_map(|item| item.parse::<i64>().ok())
                    .collect();
                if pids.len() == 3 {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(pids.len(), 3, "tree did not publish its pids");
        assert_eq!(pids[0], i64::from(leader.id()));
        let leader_identity = identity_of(pids[0]);
        let session = read_stat(pids[0]).unwrap().unwrap().session;
        let record = harness_record(leader_identity, session, TREE_GEN);
        let report =
            kill_harness_tree(&record, None, false, false, Some(Duration::from_secs(5))).unwrap();
        assert!(
            report.complete() && report.converged,
            "{}",
            report.describe()
        );
        assert!(report.killed >= 3, "{}", report.describe());
        for pid in &pids {
            assert!(
                wait_gone(*pid),
                "pid {pid} still present: {}",
                report.describe()
            );
        }
        assert_eq!(leader.wait().unwrap().signal(), Some(libc::SIGKILL));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recorded_and_tree_alive_use_only_exact_proofs() {
        const ALIVE_GEN: &str = "abcdef0123456789abcdef0123456789";
        let mut child = std::process::Command::new("sleep")
            .arg("10")
            .env(GENERATION_ENV, ALIVE_GEN)
            .spawn()
            .expect("spawn generation child");
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        let description = recorded_process_alive("runner", &exact).unwrap().unwrap();
        assert!(description.contains(&format!("pid {pid}")), "{description}");
        let stale = ProcessIdentity::new(
            pid,
            Some((exact.started_at.as_deref().unwrap().parse::<u64>().unwrap() + 1).to_string()),
            exact.boot_id.clone(),
        );
        assert_eq!(recorded_process_alive("runner", &stale).unwrap(), None);
        let no_start = ProcessIdentity::new(pid, None, exact.boot_id.clone());
        assert_eq!(
            recorded_process_alive("runner", &no_start)
                .unwrap_err()
                .code,
            "runner_identity_unknown"
        );

        let session = read_stat(pid).unwrap().unwrap().session;
        let record = harness_record(exact, session, ALIVE_GEN);
        let description = tree_alive(&record).unwrap().unwrap();
        assert!(description.contains(&format!("pid {pid}")), "{description}");
        reap(&mut child);
        assert_eq!(tree_alive(&record).unwrap(), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stop_process_refuses_an_unproven_identity_without_a_signal() {
        if !pidfds_supported() {
            return;
        }
        let mut child = spawn_sleep();
        let pid = i64::from(child.id());
        let exact = identity_of(pid);
        let legacy = ProcessIdentity::new(pid, exact.started_at.clone(), None);
        let err = stop_process(&legacy, Duration::ZERO, None, None, "runner").unwrap_err();
        assert_eq!(err.code, "runner_identity_unproven");
        assert!(
            err.message.contains("refusing to signal"),
            "{}",
            err.message
        );
        assert!(
            child.try_wait().unwrap().is_none(),
            "child must be untouched"
        );
        reap(&mut child);
    }
}
