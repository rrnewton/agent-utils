//! Process identity: which kernel process generation a record names, whether it is still alive,
//! and whether it is an ancestor of the caller.
//!
//! A process identifier alone is reused; `(boot_id, pid, starttime_ticks)` is not, within one
//! boot, and a recorded executable device and inode bind the image the kernel ran.

use std::fs::{self, File};
use std::io;
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Largest `/proc/PID/stat` record read.
pub const PROC_STAT_BYTES: usize = 8 * 1024;
const BOOT_ID_BYTES: usize = 128;
/// Deepest ancestry walked before giving up.
const MAX_ANCESTRY: usize = 4096;

/// Durable identity of one process generation.
///
/// A pathname is deliberately absent: package upgrades may atomically replace the
/// installed image while the old process remains alive. The boot/start tuple binds
/// the PID, while device/inode binds the executable image that Linux actually ran.
/// Each observation holds a pidfd so PID reuse cannot interleave its `/proc` reads.
/// Across separate invocations, Linux exposes no persistent handle in this schema;
/// reuse of the same PID by the same executable within one scheduler tick remains a
/// theoretical ambiguity. The identity never independently authorizes a signal to the PID.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    /// Identity schema version.
    pub version: u32,
    /// Canonical Linux boot UUID.
    pub boot_id: String,
    /// Positive Linux process identifier.
    pub pid: u64,
    /// Field 22 from `/proc/PID/stat`.
    pub starttime_ticks: u64,
    /// Device number of the opened executable image.
    pub executable_device: u64,
    /// Inode number of the opened executable image.
    pub executable_inode: u64,
}

impl ProcessIdentity {
    /// Whether every field is well formed: version 1, a canonical boot UUID, a positive 32-bit
    /// PID, and nonzero start time, device and inode.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.version == 1
            && canonical_boot_uuid(&self.boot_id)
            && (1..=i32::MAX as u64).contains(&self.pid)
            && self.starttime_ticks > 0
            && self.executable_device > 0
            && self.executable_inode > 0
    }
}

/// Whether a recorded local process can still be the same kernel generation.
///
/// This classification never independently authorizes input or a signal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLiveness {
    /// A coherent process generation and executable image match the record.
    Alive,
    /// The kernel positively establishes that the recorded generation is gone.
    Dead,
    /// The probe failed, changed during observation, or found a different live image.
    Unknown,
}

/// A process observed coherently: its identity, process group and executable path.
#[derive(Debug, Eq, PartialEq)]
pub struct LiveProcess {
    /// Kernel process and image generation.
    pub identity: ProcessIdentity,
    /// Process group identifier at observation.
    pub process_group_id: u64,
    /// Kernel-reported executable path at observation.
    pub executable_path: PathBuf,
}

/// The identity fields of one `/proc/PID/stat` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessStat {
    /// One-letter process state.
    pub state: u8,
    /// Parent process identifier.
    pub parent_pid: u64,
    /// Process group identifier.
    pub process_group_id: u64,
    /// Start time after boot, in clock ticks.
    pub starttime_ticks: u64,
}

/// Whether `value` is a lowercase canonical UUID, as Linux prints its boot identifier.
#[must_use]
pub fn canonical_boot_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

fn bounded_file(path: &Path, limit: usize, label: &str) -> Result<Vec<u8>> {
    let value = fs::read(path).map_err(|error| {
        Error::process(format!("cannot read {label} {}: {error}", path.display()))
    })?;
    if value.is_empty() || value.len() > limit {
        return Err(Error::process(format!(
            "{label} {} has an invalid length",
            path.display()
        )));
    }
    Ok(value)
}

/// The current boot's identifier, from `/proc/sys/kernel/random/boot_id`.
pub fn current_boot_id() -> Result<String> {
    let path = Path::new("/proc/sys/kernel/random/boot_id");
    let value = bounded_file(path, BOOT_ID_BYTES, "boot identity")?;
    let value = std::str::from_utf8(&value)
        .map_err(|_| Error::process("Linux boot identity is not UTF-8"))?
        .trim();
    if !canonical_boot_uuid(value) {
        return Err(Error::process(
            "Linux boot identity is not a canonical UUID",
        ));
    }
    Ok(value.to_owned())
}

fn linux_process_state(state: u8) -> bool {
    matches!(
        state,
        b'R' | b'S' | b'D' | b'Z' | b'T' | b't' | b'X' | b'x' | b'K' | b'W' | b'P' | b'I'
    )
}

/// Parse a `/proc/PID/stat` record, whose command name is opaque bytes up to the last `)`,
/// requiring it to be the record of `expected_pid`.
pub fn parse_process_stat(value: &[u8], expected_pid: u64) -> Result<ProcessStat> {
    let bad = || Error::process(format!("cannot parse process stat for pid {expected_pid}"));
    let opening = value
        .windows(2)
        .position(|window| window == b" (")
        .ok_or_else(bad)?;
    let close = value
        .iter()
        .rposition(|byte| *byte == b')')
        .filter(|close| *close > opening + 1)
        .ok_or_else(bad)?;
    let state = match value.get(close + 1..close + 4) {
        Some([b' ', state, b' ']) if linux_process_state(*state) => *state,
        _ => return Err(bad()),
    };
    let recorded_pid = std::str::from_utf8(&value[..opening])
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value == expected_pid)
        .ok_or_else(|| {
            Error::process(format!(
                "process stat identity changed for pid {expected_pid}"
            ))
        })?;
    debug_assert_eq!(recorded_pid, expected_pid);
    let fields = value[close + 2..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() <= 19 {
        return Err(bad());
    }
    let number = |field: &[u8]| {
        std::str::from_utf8(field)
            .ok()
            .and_then(|field| field.parse::<u64>().ok())
            .ok_or_else(bad)
    };
    Ok(ProcessStat {
        state,
        parent_pid: number(fields[1])?,
        process_group_id: number(fields[2])?,
        starttime_ticks: number(fields[19])?,
    })
}

/// Read and parse `/proc/PID/stat`.
pub fn read_process_stat(pid: u64) -> Result<ProcessStat> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let value = bounded_file(&path, PROC_STAT_BYTES, "process stat")?;
    parse_process_stat(&value, pid)
}

/// The process group and start time of a live, non-zombie `pid`.
fn process_stat(pid: u64) -> Result<(u64, u64)> {
    let stat = read_process_stat(pid)?;
    if stat.state == b'Z' {
        return Err(Error::process(format!(
            "process stat for pid {pid} is incomplete or exited"
        )));
    }
    let process_group_id = Some(stat.process_group_id)
        .filter(|value| *value > 0 && *value <= i32::MAX as u64)
        .ok_or_else(|| Error::process(format!("process group is invalid for pid {pid}")))?;
    let starttime_ticks = Some(stat.starttime_ticks)
        .filter(|value| *value > 0)
        .ok_or_else(|| Error::process(format!("process start time is invalid for pid {pid}")))?;
    Ok((process_group_id, starttime_ticks))
}

#[cfg(target_os = "linux")]
fn open_pidfd_raw(pid: u64) -> io::Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "custom process id is invalid")
        })?;
    for _attempt in 0..4 {
        // SAFETY: pidfd_open receives a checked positive pid and no pointer arguments.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if descriptor >= 0 {
            // SAFETY: pidfd_open returned a new descriptor owned by this call.
            return Ok(unsafe { OwnedFd::from_raw_fd(descriptor as i32) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Interrupted,
        "pidfd_open interrupted repeatedly",
    ))
}

/// Open a pidfd for `pid`, so later reads of it cannot reach a reused identifier.
#[cfg(target_os = "linux")]
pub fn open_pidfd(pid: u64) -> Result<OwnedFd> {
    open_pidfd_raw(pid)
        .map_err(|error| Error::process(format!("cannot pin custom process {pid}: {error}")))
}

#[cfg(target_os = "linux")]
fn pidfd_exited(pidfd: &OwnedFd) -> Result<bool> {
    for _attempt in 0..4 {
        let mut descriptor = libc::pollfd {
            fd: pidfd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor points to one initialized pollfd for this call.
        let status = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if status == 0 && descriptor.revents == 0 {
            return Ok(false);
        }
        if status < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if status == 1
            && descriptor.revents & libc::POLLIN != 0
            && descriptor.revents & !(libc::POLLIN | libc::POLLHUP) == 0
        {
            return Ok(true);
        }
        return Err(Error::process(
            "cannot establish a valid pidfd exit indication",
        ));
    }
    Err(Error::process("pidfd inspection interrupted repeatedly"))
}

#[cfg(target_os = "linux")]
trait ProcessLivenessProbe {
    type Handle;

    fn boot_identity(&mut self) -> Result<String>;
    fn pin(&mut self, pid: u64) -> io::Result<Self::Handle>;
    fn exited(&mut self, handle: &Self::Handle) -> Result<bool>;
    fn process_stat(&mut self, pid: u64) -> Result<ProcessStat>;
    fn executable_identity(&mut self, pid: u64) -> Result<(u64, u64)>;
}

#[cfg(target_os = "linux")]
struct KernelProcessLivenessProbe;

#[cfg(target_os = "linux")]
impl ProcessLivenessProbe for KernelProcessLivenessProbe {
    type Handle = OwnedFd;

    fn boot_identity(&mut self) -> Result<String> {
        current_boot_id()
    }

    fn pin(&mut self, pid: u64) -> io::Result<OwnedFd> {
        open_pidfd_raw(pid)
    }

    fn exited(&mut self, handle: &OwnedFd) -> Result<bool> {
        pidfd_exited(handle)
    }

    fn process_stat(&mut self, pid: u64) -> Result<ProcessStat> {
        let stat = read_process_stat(pid)?;
        if stat.starttime_ticks == 0
            || stat.parent_pid > i32::MAX as u64
            || stat.process_group_id > i32::MAX as u64
        {
            return Err(Error::process("invalid Linux process generation"));
        }
        Ok(stat)
    }

    fn executable_identity(&mut self, pid: u64) -> Result<(u64, u64)> {
        executable_identity(pid)
    }
}

/// Inspect a recorded local process without confusing probe failure with death.
///
/// A changed boot or process generation, or a positive kernel exit indication,
/// establishes death. A different live executable image remains unknown. This
/// proof does not authorize input or a signal.
#[must_use]
pub fn liveness(expected: &ProcessIdentity) -> ProcessLiveness {
    #[cfg(target_os = "linux")]
    {
        process_liveness_with_probe(expected, &mut KernelProcessLivenessProbe)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = expected;
        ProcessLiveness::Unknown
    }
}

#[cfg(target_os = "linux")]
fn process_liveness_with_probe<P: ProcessLivenessProbe>(
    expected: &ProcessIdentity,
    probe: &mut P,
) -> ProcessLiveness {
    fn inspect<P: ProcessLivenessProbe>(
        expected: &ProcessIdentity,
        probe: &mut P,
    ) -> Result<ProcessLiveness> {
        let boot = probe.boot_identity()?;
        if !canonical_boot_uuid(&boot) {
            return Ok(ProcessLiveness::Unknown);
        }
        let stable_boot = |probe: &mut P| {
            probe
                .boot_identity()
                .map(|after| canonical_boot_uuid(&after) && after == boot)
        };
        if boot != expected.boot_id {
            return Ok(if stable_boot(probe)? {
                ProcessLiveness::Dead
            } else {
                ProcessLiveness::Unknown
            });
        }
        let handle = match probe.pin(expected.pid) {
            Ok(handle) => handle,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {
                return Ok(if stable_boot(probe)? {
                    ProcessLiveness::Dead
                } else {
                    ProcessLiveness::Unknown
                });
            }
            Err(_) => return Ok(ProcessLiveness::Unknown),
        };
        if probe.exited(&handle)? {
            return Ok(if stable_boot(probe)? {
                ProcessLiveness::Dead
            } else {
                ProcessLiveness::Unknown
            });
        }
        let first = probe.process_stat(expected.pid)?;
        if first.starttime_ticks == 0 || !linux_process_state(first.state) {
            return Ok(ProcessLiveness::Unknown);
        }
        let dead_state = |state| matches!(state, b'Z' | b'X' | b'x');
        if first.starttime_ticks != expected.starttime_ticks || dead_state(first.state) {
            let second = probe.process_stat(expected.pid)?;
            if second.starttime_ticks == 0 || !linux_process_state(second.state) {
                return Ok(ProcessLiveness::Unknown);
            }
            let stable_boot = stable_boot(probe)?;
            probe.exited(&handle)?;
            return Ok(
                if stable_boot
                    && first.starttime_ticks == second.starttime_ticks
                    && (first.starttime_ticks != expected.starttime_ticks
                        || dead_state(second.state))
                {
                    ProcessLiveness::Dead
                } else {
                    ProcessLiveness::Unknown
                },
            );
        }
        let image_before = probe.executable_identity(expected.pid)?;
        let second = probe.process_stat(expected.pid)?;
        if second.starttime_ticks == 0 || !linux_process_state(second.state) {
            return Ok(ProcessLiveness::Unknown);
        }
        let image_after = probe.executable_identity(expected.pid)?;
        let stable_boot = stable_boot(probe)?;
        let exited = probe.exited(&handle)?;
        if !stable_boot
            || first.starttime_ticks != second.starttime_ticks
            || image_before != image_after
        {
            return Ok(ProcessLiveness::Unknown);
        }
        if exited {
            return Ok(ProcessLiveness::Dead);
        }
        Ok(
            if !dead_state(second.state)
                && image_before == (expected.executable_device, expected.executable_inode)
            {
                ProcessLiveness::Alive
            } else {
                ProcessLiveness::Unknown
            },
        )
    }

    if !expected.valid() {
        return ProcessLiveness::Unknown;
    }
    inspect(expected, probe).unwrap_or(ProcessLiveness::Unknown)
}

#[cfg(target_os = "linux")]
fn require_live_pidfd(pidfd: &OwnedFd, pid: u64) -> Result<()> {
    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: descriptor points to one initialized pollfd for the duration of this call.
        let status = unsafe { libc::poll(&mut descriptor, 1, 0) };
        if status == 0 && descriptor.revents == 0 {
            return Ok(());
        }
        if status < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        if status < 0 {
            return Err(Error::process(format!(
                "cannot inspect pinned custom process {pid}: {}",
                io::Error::last_os_error()
            )));
        }
        return Err(Error::process(format!(
            "custom process {pid} exited while its identity was inspected"
        )));
    }
}

/// The device and inode of the executable image `pid` runs.
#[cfg(target_os = "linux")]
pub fn executable_identity(pid: u64) -> Result<(u64, u64)> {
    let path = PathBuf::from(format!("/proc/{pid}/exe"));
    let image = File::open(&path).map_err(|error| {
        Error::process(format!(
            "cannot open executable image for custom process {pid}: {error}"
        ))
    })?;
    let metadata = image.metadata().map_err(|error| {
        Error::process(format!(
            "cannot inspect executable image for custom process {pid}: {error}"
        ))
    })?;
    if !metadata.is_file() || metadata.dev() == 0 || metadata.ino() == 0 {
        return Err(Error::process(format!(
            "custom process {pid} has an invalid executable image"
        )));
    }
    Ok((metadata.dev(), metadata.ino()))
}

/// Observe `pid` coherently: pin it with a pidfd, read its boot, stat, executable path and image
/// twice, and fail if anything changed in between or the process exited.
#[cfg(target_os = "linux")]
pub fn observe(pid: u64) -> Result<LiveProcess> {
    if !(1..=i32::MAX as u64).contains(&pid) {
        return Err(Error::process(
            "custom process id is not a positive Linux process id",
        ));
    }
    let pidfd = open_pidfd(pid)?;
    require_live_pidfd(&pidfd, pid)?;
    let boot_before = current_boot_id()?;
    let stat_before = process_stat(pid)?;
    let executable_path = PathBuf::from(format!("/proc/{pid}/exe"));
    let executable_path_before = fs::read_link(&executable_path).map_err(|error| {
        Error::process(format!(
            "cannot read executable path for custom process {pid}: {error}"
        ))
    })?;
    let executable_before = executable_identity(pid)?;
    let stat_after = process_stat(pid)?;
    let executable_after = executable_identity(pid)?;
    let executable_path_after = fs::read_link(&executable_path).map_err(|error| {
        Error::process(format!(
            "cannot reread executable path for custom process {pid}: {error}"
        ))
    })?;
    let boot_after = current_boot_id()?;
    require_live_pidfd(&pidfd, pid)?;
    if boot_before != boot_after
        || stat_before != stat_after
        || executable_before != executable_after
        || executable_path_before != executable_path_after
    {
        return Err(Error::process(format!(
            "custom process {pid} identity changed while it was inspected"
        )));
    }
    Ok(LiveProcess {
        identity: ProcessIdentity {
            version: 1,
            boot_id: boot_before,
            pid,
            starttime_ticks: stat_before.1,
            executable_device: executable_before.0,
            executable_inode: executable_before.1,
        },
        process_group_id: stat_before.0,
        executable_path: executable_path_before,
    })
}

/// Process observation requires Linux.
#[cfg(not(target_os = "linux"))]
pub fn observe(_pid: u64) -> Result<LiveProcess> {
    Err(Error::process(
        "custom process identity requires Linux pidfds and procfs",
    ))
}

/// One link of a process's ancestry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ancestor {
    /// Process identifier.
    pub pid: u64,
    /// Start time after boot, in clock ticks.
    pub starttime_ticks: u64,
}

/// A process and its ancestors within one boot, nearest first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ancestry {
    /// The boot every link belongs to.
    pub boot_id: String,
    /// The starting process, then its parent, and so on up to the first process whose parent is
    /// zero (the root of the PID namespace) or cannot be read.
    pub links: Vec<Ancestor>,
}

impl Ancestry {
    /// The calling process's ancestry.
    pub fn of_self() -> Result<Self> {
        Self::of(u64::from(std::process::id()))
    }

    /// The ancestry of `pid`, read twice: a link that changed between the two walks (a parent
    /// that exited and was reparented, or a reused identifier) fails the walk instead of
    /// returning a chain that never existed. `/proc` must belong to the caller's PID namespace,
    /// as it does unless a container mounts another one.
    pub fn of(pid: u64) -> Result<Self> {
        if pid == 0 {
            return Err(Error::process("process 0 has no ancestry"));
        }
        let boot_id = current_boot_id()?;
        let first = walk(pid)?;
        let second = walk(pid)?;
        if first != second || current_boot_id()? != boot_id {
            return Err(Error::process(format!(
                "the ancestry of process {pid} changed while it was read"
            )));
        }
        Ok(Self {
            boot_id,
            links: first,
        })
    }

    /// Whether the process `identity` names is a link of this ancestry: same boot, same PID and
    /// same start time. Its executable image is not compared, since a harness may replace its
    /// own image by `exec`.
    #[must_use]
    pub fn contains(&self, identity: &ProcessIdentity) -> bool {
        self.position(identity).is_some()
    }

    /// The index of the link `identity` names, `0` being the starting process.
    #[must_use]
    pub fn position(&self, identity: &ProcessIdentity) -> Option<usize> {
        if identity.boot_id != self.boot_id {
            return None;
        }
        self.links.iter().position(|link| {
            link.pid == identity.pid && link.starttime_ticks == identity.starttime_ticks
        })
    }

    /// Of `candidates`, the one naming the nearest link, with its index in `candidates`.
    pub fn nearest<'a, I>(&self, candidates: I) -> Option<(usize, &'a ProcessIdentity)>
    where
        I: IntoIterator<Item = &'a ProcessIdentity>,
    {
        candidates
            .into_iter()
            .enumerate()
            .filter_map(|(index, identity)| {
                self.position(identity)
                    .map(|depth| (depth, index, identity))
            })
            .min_by_key(|(depth, _, _)| *depth)
            .map(|(_, index, identity)| (index, identity))
    }
}

fn walk(pid: u64) -> Result<Vec<Ancestor>> {
    let mut links = Vec::new();
    let mut current = pid;
    while current != 0 {
        if links.len() >= MAX_ANCESTRY {
            return Err(Error::process(format!(
                "the ancestry of process {pid} is deeper than {MAX_ANCESTRY}"
            )));
        }
        let stat = match read_process_stat(current) {
            Ok(stat) => stat,
            // The start may itself be unreadable; an unreadable ancestor ends the chain.
            Err(error) if links.is_empty() => return Err(error),
            Err(_) => break,
        };
        // A parent starts before its child; a later start is a reused identifier.
        if links
            .last()
            .is_some_and(|child: &Ancestor| stat.starttime_ticks > child.starttime_ticks)
        {
            return Err(Error::process(format!(
                "process {current}, the parent in the ancestry of {pid}, started after its child"
            )));
        }
        links.push(Ancestor {
            pid: current,
            starttime_ticks: stat.starttime_ticks,
        });
        if stat.parent_pid == current {
            break;
        }
        current = stat.parent_pid;
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[derive(Debug)]
    enum LivenessReply {
        Boot(Result<String>),
        Pin(io::Result<()>),
        Exited(Result<bool>),
        Stat(Result<ProcessStat>),
        Image(Result<(u64, u64)>),
    }

    #[cfg(target_os = "linux")]
    struct ScriptedLivenessProbe(std::collections::VecDeque<LivenessReply>);

    #[cfg(target_os = "linux")]
    impl ScriptedLivenessProbe {
        fn next(&mut self) -> LivenessReply {
            self.0.pop_front().expect("unexpected liveness probe call")
        }
    }

    #[cfg(target_os = "linux")]
    impl ProcessLivenessProbe for ScriptedLivenessProbe {
        type Handle = ();

        fn boot_identity(&mut self) -> Result<String> {
            match self.next() {
                LivenessReply::Boot(value) => value,
                reply => panic!("expected boot probe, got {reply:?}"),
            }
        }

        fn pin(&mut self, pid: u64) -> io::Result<()> {
            assert_eq!(pid, 4242);
            match self.next() {
                LivenessReply::Pin(value) => value,
                reply => panic!("expected pidfd_open, got {reply:?}"),
            }
        }

        fn exited(&mut self, _: &()) -> Result<bool> {
            match self.next() {
                LivenessReply::Exited(value) => value,
                reply => panic!("expected pidfd poll, got {reply:?}"),
            }
        }

        fn process_stat(&mut self, pid: u64) -> Result<ProcessStat> {
            assert_eq!(pid, 4242);
            match self.next() {
                LivenessReply::Stat(value) => value,
                reply => panic!("expected stat probe, got {reply:?}"),
            }
        }

        fn executable_identity(&mut self, pid: u64) -> Result<(u64, u64)> {
            assert_eq!(pid, 4242);
            match self.next() {
                LivenessReply::Image(value) => value,
                reply => panic!("expected image probe, got {reply:?}"),
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn liveness_record() -> ProcessIdentity {
        ProcessIdentity {
            version: 1,
            boot_id: "00000000-0000-0000-0000-000000000000".to_owned(),
            pid: 4242,
            starttime_ticks: 99,
            executable_device: 10,
            executable_inode: 11,
        }
    }

    #[cfg(target_os = "linux")]
    fn liveness_stat(state: u8, starttime_ticks: u64) -> ProcessStat {
        ProcessStat {
            state,
            parent_pid: 1,
            process_group_id: 4242,
            starttime_ticks,
        }
    }

    #[cfg(target_os = "linux")]
    fn boot_reply() -> LivenessReply {
        LivenessReply::Boot(Ok(liveness_record().boot_id))
    }

    #[cfg(target_os = "linux")]
    fn alive_replies() -> Vec<LivenessReply> {
        vec![
            boot_reply(),
            LivenessReply::Pin(Ok(())),
            LivenessReply::Exited(Ok(false)),
            LivenessReply::Stat(Ok(liveness_stat(b'S', 99))),
            LivenessReply::Image(Ok((10, 11))),
            LivenessReply::Stat(Ok(liveness_stat(b'R', 99))),
            LivenessReply::Image(Ok((10, 11))),
            boot_reply(),
            LivenessReply::Exited(Ok(false)),
        ]
    }

    #[cfg(target_os = "linux")]
    fn inspect_script(expected: &ProcessIdentity, replies: Vec<LivenessReply>) -> ProcessLiveness {
        process_liveness_with_probe(expected, &mut ScriptedLivenessProbe(replies.into()))
    }

    #[cfg(target_os = "linux")]
    fn unavailable(detail: impl Into<String>) -> Error {
        Error::process(detail)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_liveness_requires_positive_generation_or_exit_evidence() {
        let expected = liveness_record();
        assert_eq!(
            inspect_script(&expected, alive_replies()),
            ProcessLiveness::Alive
        );
        assert_eq!(
            inspect_script(
                &expected,
                vec![
                    boot_reply(),
                    LivenessReply::Pin(Err(io::Error::from_raw_os_error(libc::ESRCH))),
                    boot_reply(),
                ]
            ),
            ProcessLiveness::Dead
        );
        assert_eq!(
            inspect_script(
                &expected,
                vec![
                    boot_reply(),
                    LivenessReply::Pin(Ok(())),
                    LivenessReply::Exited(Ok(true)),
                    boot_reply(),
                ]
            ),
            ProcessLiveness::Dead
        );
        let mut old_boot = expected.clone();
        old_boot.boot_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned();
        assert_eq!(
            inspect_script(&old_boot, vec![boot_reply(), boot_reply()]),
            ProcessLiveness::Dead
        );
        for (state, ticks) in [(b'S', 100), (b'Z', 99), (b'X', 99), (b'x', 99)] {
            assert_eq!(
                inspect_script(
                    &expected,
                    vec![
                        boot_reply(),
                        LivenessReply::Pin(Ok(())),
                        LivenessReply::Exited(Ok(false)),
                        LivenessReply::Stat(Ok(liveness_stat(state, ticks))),
                        LivenessReply::Stat(Ok(liveness_stat(state, ticks))),
                        boot_reply(),
                        LivenessReply::Exited(Ok(false)),
                    ]
                ),
                ProcessLiveness::Dead
            );
        }
        for field in ["device", "inode"] {
            let mut different_image = expected.clone();
            if field == "device" {
                different_image.executable_device += 1;
            } else {
                different_image.executable_inode += 1;
            }
            assert_eq!(
                inspect_script(&different_image, alive_replies()),
                ProcessLiveness::Unknown
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_liveness_keeps_failed_and_malformed_probes_unknown() {
        let expected = liveness_record();
        for errno in [
            libc::EPERM,
            libc::EACCES,
            libc::ENOSYS,
            libc::EINVAL,
            libc::EIO,
            libc::EINTR,
        ] {
            let mut replies = alive_replies();
            replies[1] = LivenessReply::Pin(Err(io::Error::from_raw_os_error(errno)));
            assert_eq!(inspect_script(&expected, replies), ProcessLiveness::Unknown);
        }
        for detail in [
            "permission denied",
            "I/O failure",
            "missing proc file",
            "malformed or oversized read",
        ] {
            for index in [0, 2, 3, 4, 5, 6, 7, 8] {
                let mut replies = alive_replies();
                let error = Err(unavailable(detail));
                replies[index] = match index {
                    0 | 7 => LivenessReply::Boot(error),
                    2 | 8 => LivenessReply::Exited(Err(unavailable(detail))),
                    3 | 5 => LivenessReply::Stat(Err(unavailable(detail))),
                    4 | 6 => LivenessReply::Image(Err(unavailable(detail))),
                    _ => unreachable!(),
                };
                assert_eq!(inspect_script(&expected, replies), ProcessLiveness::Unknown);
            }
        }
        for boot in [String::new(), "invalid".to_owned(), "x".repeat(129)] {
            assert_eq!(
                inspect_script(&expected, vec![LivenessReply::Boot(Ok(boot))]),
                ProcessLiveness::Unknown
            );
        }
        for stat in [liveness_stat(b'S', 0), liveness_stat(b'?', 99)] {
            let mut replies = alive_replies();
            replies[3] = LivenessReply::Stat(Ok(stat));
            assert_eq!(inspect_script(&expected, replies), ProcessLiveness::Unknown);
        }
        let mut invalid = expected.clone();
        invalid.version = 0;
        assert_eq!(inspect_script(&invalid, vec![]), ProcessLiveness::Unknown);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn process_liveness_refuses_changing_boot_stat_or_image_reads() {
        let expected = liveness_record();
        for (index, replacement) in [
            (
                7,
                LivenessReply::Boot(Ok("ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned())),
            ),
            (5, LivenessReply::Stat(Ok(liveness_stat(b'S', 100)))),
            (6, LivenessReply::Image(Ok((10, 12)))),
        ] {
            let mut replies = alive_replies();
            replies[index] = replacement;
            assert_eq!(inspect_script(&expected, replies), ProcessLiveness::Unknown);
        }
        for (first, second) in [
            (liveness_stat(b'S', 100), liveness_stat(b'S', 101)),
            (liveness_stat(b'Z', 99), liveness_stat(b'S', 99)),
        ] {
            assert_eq!(
                inspect_script(
                    &expected,
                    vec![
                        boot_reply(),
                        LivenessReply::Pin(Ok(())),
                        LivenessReply::Exited(Ok(false)),
                        LivenessReply::Stat(Ok(first)),
                        LivenessReply::Stat(Ok(second)),
                        boot_reply(),
                        LivenessReply::Exited(Ok(false)),
                    ]
                ),
                ProcessLiveness::Unknown
            );
        }
        // Positive absence still needs a stable readable boot identity.
        assert_eq!(
            inspect_script(
                &expected,
                vec![
                    boot_reply(),
                    LivenessReply::Pin(Err(io::Error::from_raw_os_error(libc::ESRCH))),
                    LivenessReply::Boot(Err(unavailable("boot unreadable"))),
                ]
            ),
            ProcessLiveness::Unknown
        );
    }

    #[test]
    fn process_stat_parser_treats_the_command_as_opaque_bytes() {
        let mut record = b"4242 (name\xff) Z 999 888\nPPid:\t123 (more)".to_vec();
        record.extend_from_slice(b") S 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0\n");
        let parsed = parse_process_stat(&record, 4242).unwrap();
        assert_eq!(parsed.state, b'S');
        assert_eq!(parsed.parent_pid, 7);
        assert_eq!(parsed.process_group_id, 9);
        assert_eq!(parsed.starttime_ticks, 22);
        assert!(parse_process_stat(&record, 4241).is_err());

        let mut malformed = record;
        malformed.push(b')');
        assert!(parse_process_stat(&malformed, 4242).is_err());

        let mut shifted =
            b"4242 (worker) S 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0".to_vec();
        shifted.extend_from_slice(b") 7 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22");
        assert!(parse_process_stat(&shifted, 4242).is_err());

        for state in b"RSDZTtXxKWPI" {
            let mut value =
                b"4242 (worker) S 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0".to_vec();
            let closing = value.iter().rposition(|byte| *byte == b')').unwrap();
            value[closing + 2] = *state;
            assert_eq!(parse_process_stat(&value, 4242).unwrap().state, *state);
        }
        for state in b"Q0)\x7f" {
            let mut value =
                b"4242 (worker) S 7 9 11 0 -1 4194304 1 2 3 4 13 14 15 16 20 0 1 0 22 0 0".to_vec();
            let closing = value.iter().rposition(|byte| *byte == b')').unwrap();
            value[closing + 2] = *state;
            assert!(parse_process_stat(&value, 4242).is_err());
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ancestry_of_self_contains_this_process_and_its_parent() {
        let ancestry = Ancestry::of_self().unwrap();
        let me = observe(u64::from(std::process::id())).unwrap().identity;
        assert_eq!(ancestry.position(&me), Some(0));
        // SAFETY: getppid has no preconditions and cannot fail.
        let parent = u64::try_from(unsafe { libc::getppid() }).unwrap();
        assert_eq!(ancestry.links[1].pid, parent);
        let mut other_boot = me.clone();
        other_boot.boot_id = "00000000-0000-0000-0000-000000000000".to_owned();
        assert!(!ancestry.contains(&other_boot));
        let mut reused = me.clone();
        reused.starttime_ticks += 1;
        assert!(!ancestry.contains(&reused));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ancestry_picks_the_nearest_candidate_and_ignores_a_sibling() {
        struct Reap(std::process::Child);
        impl Drop for Reap {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let child = Reap(
            std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let child = &child.0;
        let sibling = observe(u64::from(child.id())).unwrap().identity;
        let me = observe(u64::from(std::process::id())).unwrap().identity;
        // SAFETY: getppid has no preconditions and cannot fail.
        let parent_pid = u64::try_from(unsafe { libc::getppid() }).unwrap();
        let parent = ProcessIdentity {
            pid: parent_pid,
            starttime_ticks: read_process_stat(parent_pid).unwrap().starttime_ticks,
            ..me.clone()
        };
        let ancestry = Ancestry::of_self().unwrap();
        let candidates = [sibling.clone(), parent.clone(), me.clone()];
        assert_eq!(
            ancestry.nearest(&candidates).map(|(index, _)| index),
            Some(2)
        );
        assert_eq!(
            ancestry.nearest(&candidates[..2]).map(|(index, _)| index),
            Some(1)
        );
        assert!(ancestry.nearest(std::slice::from_ref(&sibling)).is_none());
        let child_ancestry = Ancestry::of(u64::from(child.id())).unwrap();
        assert_eq!(child_ancestry.position(&me), Some(1));
    }
}
