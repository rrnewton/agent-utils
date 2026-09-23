//! Direct typed access to Herdr's public CLI for interactive agent control.
//!
//! Herdr owns protocol negotiation and terminal processes. This adapter starts no
//! server and has no shell executor, broker, or allowlist dependency.
use crate::error::{AdapterError, Result};
use chat_subscription_plugin::process::ProcessPluginChild;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::ffi::{CStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const CONTROL_STDERR_BYTES: usize = 64 * 1024;
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CAPTURE_READ_BURST: usize = 256 * 1024;
const PROC_STAT_BYTES: usize = 8 * 1024;
const BOOT_ID_BYTES: usize = 128;
const SUPPORTED_PANE_SHELLS: [&str; 6] = ["bash", "zsh", "sh", "dash", "fish", "ksh"];
#[cfg(all(test, target_os = "linux"))]
static POLL_CAPTURE_INTERRUPTS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Durable identity of one pane-owned process generation.
///
/// A pathname is deliberately absent: package upgrades may atomically replace the
/// installed image while the old process remains alive. The boot/start tuple binds
/// the PID, while device/inode binds the executable image that Linux actually ran.
/// Each observation holds a pidfd so PID reuse cannot interleave its `/proc` reads.
/// Across separate invocations, Linux exposes no persistent handle in this schema;
/// reuse of the same PID by the same executable within one scheduler tick remains a
/// theoretical ambiguity. The identity is used only alongside pane and terminal-state
/// checks; it never independently authorizes a signal to the PID.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CustomProcessIdentity {
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

impl CustomProcessIdentity {
    pub(crate) fn valid(&self) -> bool {
        self.version == 1
            && canonical_boot_uuid(&self.boot_id)
            && (1..=i32::MAX as u64).contains(&self.pid)
            && self.starttime_ticks > 0
            && self.executable_device > 0
            && self.executable_inode > 0
    }
}

#[derive(Debug)]
struct PinnedHarnessExecutable {
    path: PathBuf,
    _image: File,
    device: u64,
    inode: u64,
}

#[derive(Debug, Eq, PartialEq)]
struct LiveCustomProcess {
    identity: CustomProcessIdentity,
    process_group_id: u64,
    executable_path: PathBuf,
}

#[derive(Debug)]
struct PaneProcessState {
    shell_pid: u64,
    foreground_process_group_id: u64,
    processes: Vec<Map<String, Value>>,
}

fn canonical_boot_uuid(value: &str) -> bool {
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
        AdapterError::unavailable(format!("cannot read {label} {}: {error}", path.display()))
    })?;
    if value.is_empty() || value.len() > limit {
        return Err(AdapterError::unavailable(format!(
            "{label} {} has an invalid length",
            path.display()
        )));
    }
    Ok(value)
}

fn current_boot_uuid() -> Result<String> {
    let path = Path::new("/proc/sys/kernel/random/boot_id");
    let value = bounded_file(path, BOOT_ID_BYTES, "boot identity")?;
    let value = std::str::from_utf8(&value)
        .map_err(|_| AdapterError::unavailable("Linux boot identity is not UTF-8"))?
        .trim();
    if !canonical_boot_uuid(value) {
        return Err(AdapterError::unavailable(
            "Linux boot identity is not a canonical UUID",
        ));
    }
    Ok(value.to_owned())
}

fn process_stat(pid: u64) -> Result<(u64, u64)> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let value = bounded_file(&path, PROC_STAT_BYTES, "process stat")?;
    let value = std::str::from_utf8(&value)
        .map_err(|_| AdapterError::unavailable("Linux process stat is not UTF-8"))?;
    let close = value.rfind(')').ok_or_else(|| {
        AdapterError::unavailable(format!("cannot parse process stat for pid {pid}"))
    })?;
    let recorded_pid = value[..close]
        .split_once(" (")
        .and_then(|(value, _)| value.parse::<u64>().ok())
        .filter(|value| *value == pid)
        .ok_or_else(|| {
            AdapterError::unavailable(format!("process stat identity changed for pid {pid}"))
        })?;
    debug_assert_eq!(recorded_pid, pid);
    let fields = value[close + 1..].split_whitespace().collect::<Vec<_>>();
    if fields.len() <= 19 || fields[0] == "Z" {
        return Err(AdapterError::unavailable(format!(
            "process stat for pid {pid} is incomplete or exited"
        )));
    }
    let process_group_id = fields[2]
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0 && *value <= i32::MAX as u64)
        .ok_or_else(|| {
            AdapterError::unavailable(format!("process group is invalid for pid {pid}"))
        })?;
    let starttime_ticks = fields[19]
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            AdapterError::unavailable(format!("process start time is invalid for pid {pid}"))
        })?;
    Ok((process_group_id, starttime_ticks))
}

#[cfg(target_os = "linux")]
fn open_pidfd(pid: u64) -> Result<OwnedFd> {
    let pid = libc::pid_t::try_from(pid)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| AdapterError::unavailable("custom process id is invalid"))?;
    for _attempt in 0..4 {
        // SAFETY: pidfd_open receives a checked positive pid and no pointer arguments.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if descriptor >= 0 {
            // SAFETY: pidfd_open returned a new descriptor owned by this call.
            return Ok(unsafe { OwnedFd::from_raw_fd(descriptor as i32) });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(AdapterError::unavailable(format!(
                "cannot pin custom process {pid}: {error}"
            )));
        }
    }
    Err(AdapterError::unavailable(format!(
        "cannot pin custom process {pid}: interrupted repeatedly"
    )))
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
            return Err(AdapterError::unavailable(format!(
                "cannot inspect pinned custom process {pid}: {}",
                io::Error::last_os_error()
            )));
        }
        return Err(AdapterError::unavailable(format!(
            "custom process {pid} exited while its identity was inspected"
        )));
    }
}

#[cfg(target_os = "linux")]
fn executable_identity(pid: u64) -> Result<(u64, u64)> {
    let path = PathBuf::from(format!("/proc/{pid}/exe"));
    let image = File::open(&path).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot open executable image for custom process {pid}: {error}"
        ))
    })?;
    let metadata = image.metadata().map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot inspect executable image for custom process {pid}: {error}"
        ))
    })?;
    if !metadata.is_file() || metadata.dev() == 0 || metadata.ino() == 0 {
        return Err(AdapterError::unavailable(format!(
            "custom process {pid} has an invalid executable image"
        )));
    }
    Ok((metadata.dev(), metadata.ino()))
}

#[cfg(target_os = "linux")]
fn live_custom_process(pid: u64) -> Result<LiveCustomProcess> {
    if !(1..=i32::MAX as u64).contains(&pid) {
        return Err(AdapterError::unavailable(
            "custom process id is not a positive Linux process id",
        ));
    }
    let pidfd = open_pidfd(pid)?;
    require_live_pidfd(&pidfd, pid)?;
    let boot_before = current_boot_uuid()?;
    let stat_before = process_stat(pid)?;
    let executable_path = PathBuf::from(format!("/proc/{pid}/exe"));
    let executable_path_before = fs::read_link(&executable_path).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot read executable path for custom process {pid}: {error}"
        ))
    })?;
    let executable_before = executable_identity(pid)?;
    let stat_after = process_stat(pid)?;
    let executable_after = executable_identity(pid)?;
    let executable_path_after = fs::read_link(&executable_path).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot reread executable path for custom process {pid}: {error}"
        ))
    })?;
    let boot_after = current_boot_uuid()?;
    require_live_pidfd(&pidfd, pid)?;
    if boot_before != boot_after
        || stat_before != stat_after
        || executable_before != executable_after
        || executable_path_before != executable_path_after
    {
        return Err(AdapterError::unavailable(format!(
            "custom process {pid} identity changed while it was inspected"
        )));
    }
    Ok(LiveCustomProcess {
        identity: CustomProcessIdentity {
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

#[cfg(not(target_os = "linux"))]
fn live_custom_process(_pid: u64) -> Result<LiveCustomProcess> {
    Err(AdapterError::unavailable(
        "custom process identity requires Linux pidfds and procfs",
    ))
}

fn supported_shell_name(path: &Path) -> bool {
    let Some(mut name) = path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if let Some(value) = name.strip_suffix(" (deleted)") {
        name = value;
    }
    SUPPORTED_PANE_SHELLS.contains(&name)
}

fn supported_shell_process(pid: u64) -> Result<Option<LiveCustomProcess>> {
    let observed = live_custom_process(pid)?;
    Ok(supported_shell_name(&observed.executable_path).then_some(observed))
}

fn safe_executable_metadata(metadata: &fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.permissions().mode() & 0o111 != 0
        && metadata.permissions().mode() & 0o022 == 0
        && metadata.dev() > 0
        && metadata.ino() > 0
}

fn pin_harness_executable(path: PathBuf) -> Result<PinnedHarnessExecutable> {
    let mut image = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
        .map_err(|error| {
            AdapterError::unavailable(format!(
                "cannot pin custom harness executable {}: {error}",
                path.display()
            ))
        })?;
    let metadata = image.metadata().map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot inspect pinned custom harness executable {}: {error}",
            path.display()
        ))
    })?;
    if !safe_executable_metadata(&metadata) {
        return Err(AdapterError::unavailable(format!(
            "refusing unsafe pinned custom harness executable: {}",
            path.display()
        )));
    }
    let mut magic = [0_u8; 4];
    image.read_exact(&mut magic).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot read pinned custom harness executable {}: {error}",
            path.display()
        ))
    })?;
    if magic != *b"\x7fELF" {
        return Err(AdapterError::unavailable(format!(
            "custom harness executable must be an ELF image, not a script or binfmt payload: {}",
            path.display()
        )));
    }
    let revalidated = image.metadata().map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot revalidate pinned custom harness executable {}: {error}",
            path.display()
        ))
    })?;
    let current = fs::metadata(&path).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot recheck custom harness executable {}: {error}",
            path.display()
        ))
    })?;
    if !safe_executable_metadata(&revalidated)
        || !safe_executable_metadata(&current)
        || revalidated.dev() != metadata.dev()
        || revalidated.ino() != metadata.ino()
        || current.dev() != metadata.dev()
        || current.ino() != metadata.ino()
    {
        return Err(AdapterError::unavailable(format!(
            "custom harness executable changed while it was pinned: {}",
            path.display()
        )));
    }
    Ok(PinnedHarnessExecutable {
        path,
        _image: image,
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

fn muse_effort(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
        })
}

/// Return only Muse's bounded downgrade sentence and its explicit effective effort.
pub(crate) fn muse_startup_metadata(screen: &str) -> (Option<String>, Option<String>) {
    for raw_line in screen.lines() {
        let line = raw_line.trim();
        if !line.is_ascii() || line.len() > 256 {
            continue;
        }
        let Some(rest) = line.strip_prefix("reasoning effort ") else {
            continue;
        };
        let Some((unavailable, effective)) = rest.rsplit_once("; using ") else {
            continue;
        };
        let Some((requested, detail)) = unavailable.split_once(" is not available") else {
            continue;
        };
        if !muse_effort(requested) || !muse_effort(effective) {
            continue;
        }
        // Either there is no trailing detail at all, or it is a parenthesised aside. Named rather
        // than written inline: clippy refuses the inline form as a non-minimal boolean, and its
        // own suggested rewrite folds the two acceptable shapes into one negated disjunction that
        // no longer says which shapes are acceptable.
        let detail_is_acceptable =
            detail.is_empty() || (detail.starts_with(" (") && detail.ends_with(')'));
        if !detail_is_acceptable {
            continue;
        }
        return (Some(line.to_owned()), Some(effective.to_owned()));
    }
    (None, None)
}

/// Recognize only explicit workspace-trust questions.
pub(crate) fn muse_trust_prompt(screen: &str) -> bool {
    let lowered = screen.to_ascii_lowercase();
    lowered.contains("trust this workspace")
        || (lowered.contains("do you trust")
            && (lowered.contains("workspace") || lowered.contains("folder")))
        || lowered.contains("workspace trust")
}

pub(crate) fn muse_idle_composer(screen: &str) -> bool {
    screen.contains("Auto-review") && screen.lines().any(|line| matches!(line.trim(), "❯" | "›"))
}

fn muse_text_visible(screen: &str, text: &str) -> bool {
    let wanted = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let rendered = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    if wanted.is_empty() {
        return false;
    }
    if wanted.chars().count() <= 160 {
        return rendered.contains(&wanted);
    }
    let prefix = wanted.chars().take(80).collect::<String>();
    let mut suffix = wanted.chars().rev().take(80).collect::<Vec<_>>();
    suffix.reverse();
    let suffix = suffix.into_iter().collect::<String>();
    rendered.contains(&prefix) && rendered.contains(&suffix)
}

fn muse_composer_regions(screen: &str) -> Option<(String, String)> {
    let lines = screen.lines().collect::<Vec<_>>();
    let footer = lines
        .iter()
        .rposition(|line| line.contains("Auto-review"))?;
    let dividers = lines[..footer]
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim();
            (trimmed.chars().count() >= 3
                && trimmed
                    .chars()
                    .all(|value| matches!(value, '─' | '━' | '═')))
            .then_some(index)
        })
        .collect::<Vec<_>>();
    let bottom = *dividers.last()?;
    let top = *dividers.get(dividers.len().checked_sub(2)?)?;
    Some((lines[..top].join("\n"), lines[top + 1..bottom].join("\n")))
}

pub(crate) fn muse_prompt_in_composer(screen: &str, text: &str) -> bool {
    muse_composer_regions(screen).is_some_and(|(_, composer)| muse_text_visible(&composer, text))
}

pub(crate) fn muse_prompt_transcript_count(screen: &str, text: &str) -> usize {
    let Some((transcript, _)) = muse_composer_regions(screen) else {
        return 0;
    };
    let wanted = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let rendered = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    if wanted.is_empty() {
        return 0;
    }
    if wanted.chars().count() <= 160 {
        return rendered.match_indices(&wanted).count();
    }
    let prefix = wanted.chars().take(80).collect::<String>();
    let mut suffix = wanted.chars().rev().take(80).collect::<Vec<_>>();
    suffix.reverse();
    let suffix = suffix.into_iter().collect::<String>();
    rendered
        .match_indices(&prefix)
        .count()
        .min(rendered.match_indices(&suffix).count())
}
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Capture a subprocess while enforcing wall-clock and byte bounds on multiplexed output pipes.
pub(crate) fn bounded_output(command: Command, timeout: Duration) -> io::Result<BoundedOutput> {
    bounded_output_with_cancellation(command, timeout, &|| false)
}

fn bounded_output_with_cancellation(
    command: Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<BoundedOutput> {
    bounded_output_with_cancellation_and_shutdown(
        command,
        timeout,
        cancelled,
        &mut ProcessPluginChild::shutdown,
    )
}

fn bounded_output_with_cancellation_and_shutdown(
    command: Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
    shutdown: &mut dyn FnMut(&mut ProcessPluginChild, Duration) -> io::Result<ExitStatus>,
) -> io::Result<BoundedOutput> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "control command timeout is too large",
        )
    })?;
    let mut captured = CapturedProcess::spawn(command)?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_eof = false;
    let mut stderr_eof = false;
    loop {
        let was_cancelled = cancelled();
        if was_cancelled || Instant::now() >= deadline {
            // ProcessPluginChild's live atomic supervisor pins the private process-group identity
            // until shutdown has signalled the group. In particular, no reaped command leader can
            // turn this cleanup into a signal to a numerically reused process group. Escaped
            // setsid descendants can retain their copies of these pipes, but dropping our readers
            // below is nonblocking and does not wait for their EOF.
            let primary = if was_cancelled {
                io::Error::new(io::ErrorKind::Interrupted, "control command was cancelled")
            } else {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("control command timed out after {}s", timeout.as_secs()),
                )
            };
            return match shutdown(&mut captured.child, Duration::ZERO) {
                Ok(_) => Err(primary),
                Err(cleanup) => Err(io::Error::new(
                    primary.kind(),
                    format!("{primary}; process cleanup is uncertain: {cleanup}"),
                )),
            };
        }
        if !stdout_eof {
            stdout_eof = drain_available(
                &mut captured.stdout,
                &mut stdout,
                CONTROL_STDOUT_BYTES,
                "stdout",
            )?;
        }
        if !stderr_eof {
            stderr_eof = drain_available(
                &mut captured.stderr,
                &mut stderr,
                CONTROL_STDERR_BYTES,
                "stderr",
            )?;
        }
        // executable_status uses waitid(P_PIDFD, WNOWAIT): observing exit does not release the
        // executable's numeric identity before the pinned private group has been terminated.
        if stdout_eof && stderr_eof && captured.child.executable_status()?.is_some() {
            let status = shutdown(&mut captured.child, Duration::ZERO)?;
            return Ok(BoundedOutput {
                status,
                stdout,
                stderr,
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        poll_capture(
            (!stdout_eof).then_some(&captured.stdout),
            (!stderr_eof).then_some(&captured.stderr),
            remaining.min(CAPTURE_POLL_INTERVAL),
        )?;
    }
}

struct CapturedProcess {
    child: ProcessPluginChild,
    stdout: File,
    stderr: File,
}

impl CapturedProcess {
    #[cfg(target_os = "linux")]
    fn spawn(mut command: Command) -> io::Result<Self> {
        let (stderr, stderr_writer) = pipe_cloexec()?;
        // ProcessPluginChild deliberately sends a protocol plugin's stderr to /dev/null. Herdr is
        // an ordinary control command, so restore this private capture pipe after Command has
        // installed its standard descriptors but before exec. The captured OwnedFd keeps the
        // writer live in the pre-exec child and O_CLOEXEC closes the extra copy on successful exec.
        unsafe {
            command.pre_exec(move || loop {
                if libc::dup2(stderr_writer.as_raw_fd(), libc::STDERR_FILENO) >= 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            });
        }
        let mut child = ProcessPluginChild::spawn(command)?;
        let (stdout, stdin) = child.take_transport()?;
        drop(stdin);
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;
        Ok(Self {
            child,
            stdout,
            stderr,
        })
    }

    #[cfg(not(target_os = "linux"))]
    fn spawn(_command: Command) -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "bounded control commands require Linux pidfd supervision",
        ))
    }
}

#[cfg(target_os = "linux")]
fn pipe_cloexec() -> io::Result<(File, OwnedFd)> {
    let mut descriptors = [-1_i32; 2];
    // SAFETY: descriptors points to writable storage for exactly two file descriptors.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful pipe2 returned two fresh descriptors owned by this function.
    Ok(unsafe {
        (
            File::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: fcntl receives a live descriptor and F_GETFL has no third argument.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFL accepts the retrieved flags plus O_NONBLOCK for the same descriptor.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain_available(
    reader: &mut File,
    output: &mut Vec<u8>,
    limit: usize,
    stream: &str,
) -> io::Result<bool> {
    let mut drained = 0;
    let mut buffer = [0_u8; 16 * 1024];
    while drained < CAPTURE_READ_BURST {
        let remaining = limit.saturating_sub(output.len());
        let read_bound = remaining.saturating_add(1).min(buffer.len());
        match reader.read(&mut buffer[..read_bound]) {
            Ok(0) => return Ok(true),
            Ok(count) if count > remaining => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("control command {stream} exceeds its {limit}-byte limit"),
                ));
            }
            Ok(count) => {
                output.extend_from_slice(&buffer[..count]);
                drained += count;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn poll_capture(stdout: Option<&File>, stderr: Option<&File>, timeout: Duration) -> io::Result<()> {
    let mut descriptors = [
        libc::pollfd {
            fd: stdout.map_or(-1, AsRawFd::as_raw_fd),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: stderr.map_or(-1, AsRawFd::as_raw_fd),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let milliseconds = timeout
        .as_millis()
        .max(u128::from(!timeout.is_zero()))
        .min(i32::MAX as u128) as i32;
    // SAFETY: descriptors is initialized and remains live for the matching array length.
    let result = unsafe {
        libc::poll(
            descriptors.as_mut_ptr(),
            descriptors.len() as libc::nfds_t,
            milliseconds,
        )
    };
    if result >= 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    // Return EINTR to the absolute-deadline owner loop instead of restarting the full relative
    // timeout here; a signal stream must not extend control-command cancellation indefinitely.
    if error.kind() == io::ErrorKind::Interrupted {
        #[cfg(all(test, target_os = "linux"))]
        POLL_CAPTURE_INTERRUPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        return Ok(());
    }
    Err(error)
}

/// One terminal pane and its owning tab and workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pane {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Herdr tab identifier.
    pub tab_id: String,
    /// Herdr workspace identifier.
    pub workspace_id: String,
}

/// Identity and readiness fields for one interactive-agent pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentPaneInfo {
    /// Herdr pane identifier.
    pub pane_id: String,
    /// Owning workspace identifier.
    pub workspace_id: String,
    /// Pane working directory as reported by Herdr.
    pub cwd: String,
    /// Interactive agent implementation, when Herdr recognizes one.
    pub agent: Option<String>,
    /// Native Herdr agent status, or `"unknown"` when no status was reported.
    pub status: String,
    /// Agent name carried by the stable session identity, when present.
    pub session_agent: Option<String>,
    /// Stable session value, when present.
    pub session_value: Option<String>,
}

/// Captured result of one external command invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    /// Conventional numeric process status; zero denotes success.
    pub status: i32,
    /// Captured standard output decoded as UTF-8 with replacement.
    pub stdout: String,
    /// Captured standard error decoded as UTF-8 with replacement.
    pub stderr: String,
}

/// Direct Herdr CLI adapter; executable availability is checked when used.
#[derive(Clone, Debug)]
pub struct HerdrClient {
    executable: PathBuf,
}
impl HerdrClient {
    /// Select a configured Herdr executable. Only the direct adapter is supported.
    pub fn with_executable(adapter: &str, executable: &Path) -> Result<Self> {
        if adapter != "direct" {
            return Err(AdapterError::unavailable(
                "agentctl supports direct Herdr access only",
            ));
        }
        Ok(Self {
            executable: executable.to_owned(),
        })
    }
    fn invoke_with_timeout(&self, args: &[String], timeout: Duration) -> Result<CommandOutput> {
        self.invoke_with_timeout_and_cancellation(args, timeout, &|| false)
    }
    fn invoke_with_timeout_and_cancellation(
        &self,
        args: &[String],
        timeout: Duration,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<CommandOutput> {
        let executable = resolve_executable(&self.executable)?;
        let mut command = Command::new(executable);
        command.args(args);
        let output = bounded_output_with_cancellation(command, timeout, cancelled)
            .map_err(|error| AdapterError::unavailable(format!("cannot invoke Herdr: {error}")))?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>> {
        let workspaces = self.workspace_entries()?;
        unique_label_id(&workspaces, "workspace_id", label, "workspace")
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<(String, String, String)> {
        let arguments = workspace_create_arguments(label, cwd, environment);
        let result = self.call(&arguments, &format!("workspace create {label:?}"))?;
        let workspace = required_object(&result, "workspace", "workspace create")?;
        let tab = required_object(&result, "tab", "workspace create")?;
        let pane = required_object(&result, "root_pane", "workspace create")?;
        Ok((
            required_string(workspace, "workspace_id", "workspace create")?,
            required_string(tab, "tab_id", "workspace create")?,
            required_string(pane, "pane_id", "workspace create")?,
        ))
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn create_tab(
        &self,
        workspace_id: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<String> {
        let arguments = tab_create_arguments(workspace_id, label, cwd, environment);
        let result = self.call(&arguments, &format!("tab create {label:?}"))?;
        let tab = match result.get("tab") {
            Some(value) => value
                .as_object()
                .ok_or_else(|| AdapterError::unavailable("tab create: 'tab' is not an object"))?,
            None => &result,
        };
        required_string(tab, "tab_id", "tab create")
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
        self.call(
            &strings(&["tab", "rename", tab_id, label]),
            &format!("tab rename {tab_id}"),
        )?;
        Ok(())
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn close_tab(&self, tab_id: &str) -> Result<()> {
        self.call_ok(
            &strings(&["tab", "close", tab_id]),
            &format!("tab close {tab_id}"),
        )
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn start_agent(
        &self,
        name: &str,
        kind: &str,
        pane_id: &str,
        arguments: &[String],
        timeout: Duration,
    ) -> Result<()> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(AdapterError::unavailable(
                "agent startup timeout must be between 0 and 300 seconds",
            ));
        }
        let mut args = strings(&[
            "agent",
            "start",
            name,
            "--kind",
            kind,
            "--pane",
            pane_id,
            "--timeout",
        ]);
        args.extend([timeout.as_millis().max(1).to_string(), "--".to_owned()]);
        args.extend_from_slice(arguments);
        let result = self.invoke_with_timeout(&args, timeout + CONTROL_TIMEOUT)?;
        if result.status != 0 {
            return Err(AdapterError::unavailable(format!(
                "agent start {name:?}: {}",
                stderr_detail(&result)
            )));
        }
        Ok(())
    }

    fn pane_process_state(
        &self,
        pane_id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PaneProcessState> {
        let result = self.call_with_cancellation(
            &strings(&["pane", "process-info", "--pane", pane_id]),
            &format!("pane process-info {pane_id}"),
            cancelled,
        )?;
        let info = required_object(&result, "process_info", "pane process-info")?;
        if required_string(info, "pane_id", "pane process-info")? != pane_id {
            return Err(AdapterError::unavailable(
                "pane process-info returned a different pane identity",
            ));
        }
        let shell_pid = info
            .get("shell_pid")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .ok_or_else(|| {
                AdapterError::unavailable(
                    "pane process-info: shell_pid is not a positive Linux process id",
                )
            })?;
        let foreground_process_group_id = info
            .get("foreground_process_group_id")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .ok_or_else(|| {
                AdapterError::unavailable(
                    "pane process-info: foreground_process_group_id is not a positive Linux process id",
                )
            })?;
        let processes = value_array(info.get("foreground_processes"), "foreground_processes")?;
        Ok(PaneProcessState {
            shell_pid,
            foreground_process_group_id,
            processes: object_entries(processes, "foreground process")?,
        })
    }

    fn foreground_processes(
        &self,
        pane_id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(u64, Vec<Map<String, Value>>)> {
        let result = self.call_with_cancellation(
            &strings(&["pane", "process-info", "--pane", pane_id]),
            &format!("pane process-info {pane_id}"),
            cancelled,
        )?;
        let info = required_object(&result, "process_info", "pane process-info")?;
        if required_string(info, "pane_id", "pane process-info")? != pane_id {
            return Err(AdapterError::unavailable(
                "pane process-info returned a different pane identity",
            ));
        }
        let foreground_process_group_id = info
            .get("foreground_process_group_id")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .ok_or_else(|| {
                AdapterError::unavailable(
                    "pane process-info: foreground_process_group_id is not a positive Linux process id",
                )
            })?;
        let processes = value_array(info.get("foreground_processes"), "foreground_processes")?;
        Ok((
            foreground_process_group_id,
            object_entries(processes, "foreground process")?,
        ))
    }

    fn custom_harness_observed(
        &self,
        pane_id: &str,
        executable: &PinnedHarnessExecutable,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Option<CustomProcessIdentity>> {
        let (foreground_process_group_id, processes) =
            self.foreground_processes(pane_id, cancelled)?;
        let mut candidate = None;
        for process in processes {
            let pid = process
                .get("pid")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0 && *value <= i32::MAX as u64)
                .ok_or_else(|| {
                    AdapterError::unavailable(
                        "foreground process: \"pid\" is not a positive Linux process id",
                    )
                })?;
            let argv0 = process
                .get("argv")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .and_then(Value::as_str)
                .unwrap_or_default();
            let exact_argv =
                fs::canonicalize(argv0).is_ok_and(|path| path == executable.path.as_path());
            if !exact_argv {
                continue;
            }
            if candidate.replace(pid).is_some() {
                return Err(AdapterError::unavailable(
                    "pane process-info returned multiple matching custom harness processes",
                ));
            }
        }
        let Some(pid) = candidate else {
            return Ok(None);
        };
        let observed = live_custom_process(pid)?;
        Ok((observed.process_group_id == foreground_process_group_id
            && observed.identity.executable_device == executable.device
            && observed.identity.executable_inode == executable.inode)
            .then_some(observed.identity))
    }

    fn recorded_custom_harness_observed(
        &self,
        pane_id: &str,
        identity: &CustomProcessIdentity,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<bool> {
        if !identity.valid() {
            return Err(AdapterError::unavailable(
                "recorded custom process identity is invalid",
            ));
        }
        let (foreground_process_group_id, processes) =
            self.foreground_processes(pane_id, cancelled)?;
        let mut matches = 0_u8;
        for process in processes {
            let pid = process
                .get("pid")
                .and_then(Value::as_u64)
                .filter(|value| *value > 0 && *value <= i32::MAX as u64)
                .ok_or_else(|| {
                    AdapterError::unavailable(
                        "foreground process: \"pid\" is not a positive Linux process id",
                    )
                })?;
            if pid == identity.pid {
                matches = matches.saturating_add(1);
            }
        }
        if matches != 1 {
            return Ok(false);
        }
        let observed = live_custom_process(identity.pid)?;
        Ok(observed.process_group_id == foreground_process_group_id
            && observed.identity == *identity)
    }

    /// Require the fixed-location custom harness executable in one exact foreground pane.
    pub fn verify_custom_harness(
        &self,
        pane_id: &str,
        kind: &str,
        identity: Option<&CustomProcessIdentity>,
    ) -> Result<()> {
        self.verify_custom_harness_with_cancellation(pane_id, kind, identity, &|| false)
    }

    /// Prove that no child command owns the terminal and the foreground process
    /// group contains only the pane's Herdr-reported shell process.
    pub fn pane_is_idle_shell(&self, pane_id: &str) -> Result<bool> {
        let state = self.pane_process_state(pane_id, &|| false)?;
        if state.foreground_process_group_id != state.shell_pid || state.processes.len() != 1 {
            return Ok(false);
        }
        let process = &state.processes[0];
        let pid = process
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .ok_or_else(|| {
                AdapterError::unavailable(
                    "foreground process: \"pid\" is not a positive Linux process id",
                )
            })?;
        let argv0 = process
            .get("argv")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .unwrap_or_default();
        let argv_executable = fs::canonicalize(argv0).ok();
        let kernel_executable = fs::canonicalize(format!("/proc/{}/exe", state.shell_pid)).ok();
        let observed = supported_shell_process(state.shell_pid)?;
        Ok(pid == state.shell_pid
            && argv_executable.is_some()
            && argv_executable == kernel_executable
            && observed.is_some_and(|value| value.process_group_id == state.shell_pid))
    }

    /// Capture the exact Linux process generation and executable image for a pane shell.
    pub fn pane_shell_identity(&self, pane_id: &str) -> Result<CustomProcessIdentity> {
        let state = self.pane_process_state(pane_id, &|| false)?;
        let observed = supported_shell_process(state.shell_pid)?
            .filter(|value| value.process_group_id == state.shell_pid)
            .ok_or_else(|| {
                AdapterError::unavailable(format!(
                    "cannot capture a supported identity-bound shell process for pane {pane_id}"
                ))
            })?;
        Ok(observed.identity)
    }

    /// Require the recorded shell generation and Herdr's strict idle-shell presentation.
    pub fn pane_is_same_idle_shell(
        &self,
        pane_id: &str,
        expected: &CustomProcessIdentity,
    ) -> Result<bool> {
        if !expected.valid() {
            return Err(AdapterError::unavailable(
                "recorded pane shell identity is invalid",
            ));
        }
        let state = self.pane_process_state(pane_id, &|| false)?;
        if state.shell_pid != expected.pid
            || state.foreground_process_group_id != state.shell_pid
            || state.processes.len() != 1
        {
            return Ok(false);
        }
        let process = &state.processes[0];
        let pid = process
            .get("pid")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .ok_or_else(|| {
                AdapterError::unavailable(
                    "foreground process: \"pid\" is not a positive Linux process id",
                )
            })?;
        let argv0 = process
            .get("argv")
            .and_then(Value::as_array)
            .and_then(|values| values.first())
            .and_then(Value::as_str)
            .unwrap_or_default();
        let argv_executable = fs::canonicalize(argv0).ok();
        let kernel_executable = fs::canonicalize(format!("/proc/{}/exe", state.shell_pid)).ok();
        let observed = supported_shell_process(state.shell_pid)?;
        Ok(pid == state.shell_pid
            && argv_executable.is_some()
            && argv_executable == kernel_executable
            && observed.is_some_and(|value| {
                value.identity == *expected
                    && value.process_group_id == state.foreground_process_group_id
            }))
    }

    pub(crate) fn verify_custom_harness_with_cancellation(
        &self,
        pane_id: &str,
        kind: &str,
        identity: Option<&CustomProcessIdentity>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let observed = if let Some(identity) = identity {
            // Do not consult the installation pathname here. An upgrade may have atomically
            // replaced it while this still-running process retains the recorded image inode.
            self.recorded_custom_harness_observed(pane_id, identity, cancelled)?
        } else {
            // Records without a durable process tuple retain strict current-path
            // check; never interpret a missing identity as a wildcard for a deleted executable.
            let executable = pin_harness_executable(resolve_harness_executable(kind)?)?;
            self.custom_harness_observed(pane_id, &executable, cancelled)?
                .is_some()
        };
        if !observed {
            return Err(AdapterError::unavailable(format!(
                "custom harness {kind:?} is not the foreground process in pane {pane_id}"
            )));
        }
        Ok(())
    }

    /// Launch an otherwise unsupported TUI through one exact pane.
    ///
    /// This path never supplies input to a workspace-trust prompt. It proves
    /// the foreground executable, publishes a pane-local agent report, and
    /// re-reads that exact pane rather than consulting a name registry.
    pub fn start_pane_agent(
        &self,
        _name: &str,
        kind: &str,
        pane_id: &str,
        arguments: &[String],
        timeout: Duration,
        persist_identity: &mut dyn FnMut(CustomProcessIdentity) -> Result<()>,
    ) -> Result<()> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(AdapterError::unavailable(
                "agent startup timeout must be between 0 and 300 seconds",
            ));
        }
        if kind.is_empty()
            || !kind.as_bytes()[0].is_ascii_lowercase()
            || !kind
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(AdapterError::unavailable(
                "custom harness name must contain lowercase ASCII letters, digits, or hyphens",
            ));
        }
        let executable = pin_harness_executable(resolve_harness_executable(kind)?)?;
        let mut command = vec![executable.path.display().to_string()];
        command.extend_from_slice(arguments);
        let line = shell_join(&command)?;
        self.call_ok(
            &[
                "pane".to_owned(),
                "run".to_owned(),
                pane_id.to_owned(),
                line,
            ],
            &format!("pane run {kind:?}"),
        )?;
        let deadline = Instant::now() + timeout;
        let mut observed = None;
        while Instant::now() < deadline {
            observed = self.custom_harness_observed(pane_id, &executable, &|| false)?;
            if observed.is_some() {
                break;
            }
            thread::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        let observed = observed.ok_or_else(|| {
            AdapterError::unavailable(format!(
                "custom harness {kind:?} was not the foreground process in pane {pane_id}"
            ))
        })?;
        // Persist before trust/readiness/report checks. If any later step fails, stop still has
        // enough kernel identity to close only this observed process's pane.
        persist_identity(observed.clone())?;
        let mut ready = false;
        while Instant::now() < deadline {
            if !self.recorded_custom_harness_observed(pane_id, &observed, &|| false)? {
                return Err(AdapterError::unavailable(format!(
                    "custom harness {kind:?} identity changed before readiness in pane {pane_id}"
                )));
            }
            let screen = self.read(pane_id, "visible", Some(200))?;
            if muse_trust_prompt(&screen)
                && !arguments.iter().any(|value| value == "--trust-workspace")
            {
                return Err(AdapterError::unavailable(format!(
                    "{kind} workspace trust prompt requires human attention; no input was submitted"
                )));
            }
            ready = muse_idle_composer(&screen);
            if ready {
                break;
            }
            thread::sleep(
                Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        if !ready {
            return Err(AdapterError::unavailable(format!(
                "custom harness {kind:?} did not reach a verified idle composer in pane {pane_id}"
            )));
        }
        if !self.recorded_custom_harness_observed(pane_id, &observed, &|| false)? {
            return Err(AdapterError::unavailable(format!(
                "custom harness {kind:?} identity changed before report in pane {pane_id}"
            )));
        }
        self.call_ok(
            &strings(&[
                "pane",
                "report-agent",
                pane_id,
                "--source",
                "agentctl",
                "--agent",
                kind,
                "--state",
                "idle",
                "--message",
                "agentctl custom harness",
            ]),
            &format!("report custom agent {pane_id}"),
        )?;
        Ok(())
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn agent_pane(&self, name: &str) -> Result<String> {
        self.agent_pane_with_cancellation(name, &|| false)
    }
    pub(crate) fn agent_pane_with_cancellation(
        &self,
        name: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String> {
        let result = self.call_with_cancellation(
            &strings(&["agent", "get", name]),
            &format!("agent get {name:?}"),
            cancelled,
        )?;
        let info = required_object(&result, "agent", "agent get")?;
        if required_string(info, "name", "agent get")? != name {
            return Err(AdapterError::unavailable(format!(
                "agent get: returned a different agent name for {name:?}"
            )));
        }
        required_string(info, "pane_id", "agent get")
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn report_agent_session(
        &self,
        name: &str,
        pane_id: &str,
        kind: &str,
        session_id: &str,
    ) -> Result<()> {
        if self.agent_pane(name)? != pane_id {
            return Err(AdapterError::unavailable(
                "cannot bind a session to a different named agent pane",
            ));
        }
        self.call_ok(
            &strings(&[
                "pane",
                "report-agent-session",
                pane_id,
                "--source",
                "herdr-agent",
                "--agent",
                kind,
                "--agent-session-id",
                session_id,
            ]),
            "report managed agent session",
        )
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>> {
        self.panes_with_cancellation(workspace_id, &|| false)
    }
    pub(crate) fn panes_with_cancellation(
        &self,
        workspace_id: Option<&str>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Pane>> {
        let mut args = strings(&["pane", "list"]);
        if let Some(workspace_id) = workspace_id {
            args.extend(strings(&["--workspace", workspace_id]));
        }
        let result = self.call_with_cancellation(&args, "pane list", cancelled)?;
        let values = value_array(result.get("panes"), "pane list.panes")?;
        let entries = object_entries(values, "pane list entry")?;
        let panes = entries
            .iter()
            .map(|pane| {
                Ok(Pane {
                    pane_id: required_string(pane, "pane_id", "pane list entry")?,
                    tab_id: required_string(pane, "tab_id", "pane list entry")?,
                    workspace_id: required_string(pane, "workspace_id", "pane list entry")?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        if let Some(expected) = workspace_id {
            if let Some(pane) = panes.iter().find(|pane| pane.workspace_id != expected) {
                return Err(AdapterError::unavailable(format!(
                    "pane list: returned pane {:?} from workspace {:?}, expected {expected:?}",
                    pane.pane_id, pane.workspace_id
                )));
            }
        }
        Ok(panes)
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn pane_info(&self, pane_id: &str) -> Result<AgentPaneInfo> {
        self.pane_info_with_cancellation(pane_id, &|| false)
    }
    pub(crate) fn pane_info_with_cancellation(
        &self,
        pane_id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<AgentPaneInfo> {
        let result = self.call_with_cancellation(
            &strings(&["pane", "get", pane_id]),
            &format!("pane get {pane_id}"),
            cancelled,
        )?;
        parse_pane_info(&result, pane_id)
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn workspace_label(&self, workspace_id: &str) -> Result<String> {
        self.workspace_label_with_cancellation(workspace_id, &|| false)
    }
    pub(crate) fn workspace_label_with_cancellation(
        &self,
        workspace_id: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String> {
        let result = self.call_with_cancellation(
            &strings(&["workspace", "get", workspace_id]),
            &format!("workspace get {workspace_id}"),
            cancelled,
        )?;
        let workspace = required_object(&result, "workspace", "workspace get")?;
        let returned = required_string(workspace, "workspace_id", "workspace get")?;
        if returned != workspace_id {
            return Err(AdapterError::unavailable(format!(
                "workspace get: returned workspace {returned:?}, expected {workspace_id:?}"
            )));
        }
        required_string(workspace, "label", "workspace get")
    }
    /// Resolve the compatible running server's absolute event-socket path.
    pub fn event_socket(&self) -> Result<PathBuf> {
        self.event_socket_with_cancellation(&|| false)
    }
    pub(crate) fn event_socket_with_cancellation(
        &self,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<PathBuf> {
        let completed = self.invoke_with_timeout_and_cancellation(
            &strings(&["status", "server", "--json"]),
            CONTROL_TIMEOUT,
            cancelled,
        )?;
        if completed.status != 0 {
            return Err(AdapterError::unavailable(format!(
                "cannot discover the running Herdr server: {}",
                stderr_detail(&completed)
            )));
        }
        let document = serde_json::from_str::<Value>(&completed.stdout).map_err(|error| {
            AdapterError::unavailable(format!("invalid Herdr server status: {error}"))
        })?;
        let status = document.as_object().ok_or_else(|| {
            AdapterError::unavailable("invalid Herdr server status: expected an object")
        })?;
        if status.get("running").and_then(Value::as_bool) != Some(true)
            || status.get("compatible").and_then(Value::as_bool) != Some(true)
        {
            return Err(AdapterError::unavailable(
                "output subscriptions require a running, compatible Herdr server",
            ));
        }
        let socket = required_string(status, "socket", "Herdr server status")?;
        let socket = PathBuf::from(socket);
        if !socket.is_absolute() || socket.as_os_str().as_bytes().contains(&0) {
            return Err(AdapterError::unavailable(
                "Herdr event socket must be an absolute NUL-free path",
            ));
        }
        Ok(socket)
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn wait_agent_status(&self, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()> {
        self.wait_agent_status_with_cancellation(pane_id, status, timeout_ms, &|| false)
    }
    pub(crate) fn wait_agent_status_with_cancellation(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let purpose = format!("wait for pane {pane_id} status {status}");
        let completed = self.invoke_with_timeout_and_cancellation(
            &[
                "agent".to_owned(),
                "wait".to_owned(),
                pane_id.to_owned(),
                "--until".to_owned(),
                status.to_owned(),
                "--timeout".to_owned(),
                timeout_ms.to_string(),
            ],
            CONTROL_TIMEOUT
                .max(Duration::from_millis(timeout_ms).saturating_add(Duration::from_secs(5))),
            cancelled,
        )?;
        if completed.status != 0 {
            return Err(AdapterError::unavailable(format!(
                "{purpose}: {}",
                stderr_detail(&completed)
            )));
        }
        let document = serde_json::from_str::<Value>(&completed.stdout).map_err(|error| {
            AdapterError::unavailable(format!("{purpose}: invalid Herdr event response: {error}"))
        })?;
        let envelope = document.as_object().ok_or_else(|| {
            AdapterError::unavailable(format!("{purpose}: event response is not an object"))
        })?;
        let result = required_object(envelope, "result", &purpose)?;
        let data = required_object(result, "agent", &purpose)?;
        let returned_pane = required_string(data, "pane_id", &purpose)?;
        let returned_status = required_string(data, "agent_status", &purpose)?;
        if returned_pane != pane_id || returned_status != status {
            return Err(AdapterError::unavailable(format!(
                "{purpose}: event reported pane={returned_pane:?} status={returned_status:?}"
            )));
        }
        Ok(())
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn read(&self, pane_id: &str, source: &str, lines: Option<usize>) -> Result<String> {
        self.read_with_cancellation(pane_id, source, lines, &|| false)
    }
    pub(crate) fn read_with_cancellation(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<String> {
        let mut args = strings(&["pane", "read", pane_id, "--source", source]);
        if let Some(lines) = lines {
            args.extend(["--lines".to_owned(), lines.to_string()]);
        }
        let completed =
            self.invoke_with_timeout_and_cancellation(&args, CONTROL_TIMEOUT, cancelled)?;
        if completed.status != 0 {
            return Err(AdapterError::unavailable(format!(
                "pane read {pane_id}: {}",
                stderr_detail(&completed)
            )));
        }
        Ok(completed.stdout)
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn prompt_agent(&self, pane_id: &str, text: &str) -> Result<()> {
        self.prompt_agent_with_cancellation(pane_id, text, &|| false)
    }
    /// Insert literal text into one pane without submitting it.
    pub fn send_text(&self, pane_id: &str, text: &str) -> Result<()> {
        self.send_text_with_cancellation(pane_id, text, &|| false)
    }
    pub(crate) fn send_text_with_cancellation(
        &self,
        pane_id: &str,
        text: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        self.call_ok_with_cancellation(
            &strings(&["pane", "send-text", pane_id, text]),
            &format!("pane send-text {pane_id}"),
            cancelled,
        )
    }
    pub(crate) fn prompt_agent_with_cancellation(
        &self,
        pane_id: &str,
        text: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        self.call_ok_with_cancellation(
            &strings(&["agent", "prompt", pane_id, text]),
            &format!("agent prompt {pane_id}"),
            cancelled,
        )
    }
    /// Invoke and validate the corresponding Herdr agent-control operation.
    pub fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
        self.send_keys_with_cancellation(pane_id, keys, &|| false)
    }
    pub(crate) fn send_keys_with_cancellation(
        &self,
        pane_id: &str,
        keys: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        self.call_ok_with_cancellation(
            &strings(&["pane", "send-keys", pane_id, keys]),
            &format!("pane send-keys {pane_id}"),
            cancelled,
        )
    }
    fn workspace_entries(&self) -> Result<Vec<Map<String, Value>>> {
        let result = self.call(&strings(&["workspace", "list"]), "workspace list")?;
        let values = value_array(result.get("workspaces"), "workspace list.workspaces")?;
        object_entries(values, "workspace list entry")
    }
    fn call(&self, args: &[String], purpose: &str) -> Result<Map<String, Value>> {
        self.call_with_cancellation(args, purpose, &|| false)
    }
    fn call_with_cancellation(
        &self,
        args: &[String],
        purpose: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Map<String, Value>> {
        let completed =
            self.invoke_with_timeout_and_cancellation(args, CONTROL_TIMEOUT, cancelled)?;
        if completed.status != 0 {
            return Err(AdapterError::unavailable(format!(
                "{purpose}: {}",
                detail(&completed)
            )));
        }
        let document = serde_json::from_str::<Value>(&completed.stdout).map_err(|_| {
            let preview: String = completed.stdout.trim().chars().take(200).collect();
            AdapterError::unavailable(format!(
                "{purpose}: herdr returned non-JSON output: {preview:?}"
            ))
        })?;
        let envelope = document.as_object().ok_or_else(|| {
            AdapterError::unavailable(format!("{purpose}: Herdr response is not an object"))
        })?;
        envelope
            .get("result")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                AdapterError::unavailable(format!("{purpose}: Herdr response has no result object"))
            })
    }
    fn call_ok(&self, args: &[String], purpose: &str) -> Result<()> {
        self.call_ok_with_cancellation(args, purpose, &|| false)
    }
    fn call_ok_with_cancellation(
        &self,
        args: &[String],
        purpose: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<()> {
        let completed =
            self.invoke_with_timeout_and_cancellation(args, CONTROL_TIMEOUT, cancelled)?;
        if completed.status == 0 {
            Ok(())
        } else {
            Err(AdapterError::unavailable(format!(
                "{purpose}: {}",
                detail(&completed)
            )))
        }
    }
    /// Create a tab and capture its initial pane from the same allocation result.
    pub fn create_tab_with_pane(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<(String, String)> {
        let arguments = tab_create_arguments(workspace, label, cwd, environment);
        let result = self.call(&arguments, "tab create")?;
        let tab = required_object(&result, "tab", "tab create")?;
        let pane = required_object(&result, "root_pane", "tab create")?;
        let tab_id = required_string(tab, "tab_id", "tab create")?;
        if required_string(pane, "tab_id", "created pane")? != tab_id
            || required_string(pane, "workspace_id", "created pane")? != workspace
        {
            return Err(AdapterError::unavailable(
                "created pane does not belong to the allocated tab/workspace",
            ));
        }
        Ok((tab_id, required_string(pane, "pane_id", "tab create")?))
    }
    /// Close exactly the owned pane; concurrent human-created sibling panes survive.
    pub fn close_pane(&self, pane: &str) -> Result<()> {
        self.call_ok(&strings(&["pane", "close", pane]), "pane close")
    }
    /// Focus the named pane in its existing Herdr workspace.
    pub fn focus_pane(&self, pane: &str) -> Result<()> {
        self.call_ok(&strings(&["agent", "focus", pane]), "agent focus")
    }
}

fn workspace_create_arguments(label: &str, cwd: &str, environment: &[String]) -> Vec<String> {
    let mut arguments = strings(&["workspace", "create", "--label", label, "--cwd", cwd]);
    append_environment_and_no_focus(&mut arguments, environment);
    arguments
}

fn tab_create_arguments(
    workspace: &str,
    label: &str,
    cwd: &str,
    environment: &[String],
) -> Vec<String> {
    let mut arguments = strings(&[
        "tab",
        "create",
        "--workspace",
        workspace,
        "--label",
        label,
        "--cwd",
        cwd,
    ]);
    append_environment_and_no_focus(&mut arguments, environment);
    arguments
}

fn append_environment_and_no_focus(arguments: &mut Vec<String>, environment: &[String]) {
    for entry in environment {
        arguments.extend(["--env".to_owned(), entry.clone()]);
    }
    arguments.push("--no-focus".to_owned());
}

fn parse_pane_info(result: &Map<String, Value>, pane_id: &str) -> Result<AgentPaneInfo> {
    let pane = required_object(result, "pane", "pane get")?;
    let returned = required_string(pane, "pane_id", "pane get")?;
    if returned != pane_id {
        return Err(AdapterError::unavailable(format!(
            "pane get: returned pane {returned:?}, expected {pane_id:?}"
        )));
    }
    let (session_agent, session_value) = match pane.get("agent_session") {
        None | Some(Value::Null) => (None, None),
        Some(Value::Object(session)) => (
            optional_string(session, "agent", "pane agent_session")?,
            optional_string(session, "value", "pane agent_session")?,
        ),
        Some(_) => {
            return Err(AdapterError::unavailable(
                "pane get: 'agent_session' is not an object",
            ));
        }
    };
    Ok(AgentPaneInfo {
        pane_id: returned,
        workspace_id: required_string(pane, "workspace_id", "pane get")?,
        cwd: required_string(pane, "cwd", "pane get")?,
        agent: optional_string(pane, "agent", "pane get")?,
        status: optional_string(pane, "agent_status", "pane get")?
            .unwrap_or_else(|| "unknown".to_owned()),
        session_agent,
        session_value,
    })
}

fn resolve_executable(configured: &Path) -> Result<PathBuf> {
    let candidates = if configured.components().count() > 1 || configured.is_absolute() {
        vec![configured.to_owned()]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(configured))
            .collect()
    };
    for path in candidates {
        if fs::metadata(&path)
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        {
            return fs::canonicalize(&path).map_err(|error| {
                AdapterError::unavailable(format!("cannot resolve Herdr executable: {error}"))
            });
        }
    }
    Err(AdapterError::unavailable(format!(
        "Herdr executable not found: {}; install Herdr or pass --herdr-bin",
        configured.display()
    )))
}

pub(crate) fn account_home() -> Result<PathBuf> {
    // SAFETY: getpwuid_r writes only into the supplied record and byte buffer;
    // the C string is copied before either backing allocation is dropped.
    let uid = unsafe { libc::getuid() };
    let suggested = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    let mut size = if suggested > 0 {
        usize::try_from(suggested).unwrap_or(16_384)
    } else {
        16_384
    };
    loop {
        let mut record = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0_u8; size];
        let code = unsafe {
            libc::getpwuid_r(
                uid,
                record.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut result,
            )
        };
        if code == libc::ERANGE {
            size = size.saturating_mul(2);
            if size > 16 * 1024 * 1024 {
                return Err(AdapterError::unavailable(
                    "cannot resolve current account home: account record is too large",
                ));
            }
            continue;
        }
        if code != 0 {
            return Err(AdapterError::unavailable(format!(
                "cannot resolve current account home: {}",
                io::Error::from_raw_os_error(code)
            )));
        }
        if result.is_null() {
            return Err(AdapterError::unavailable(
                "cannot resolve current account home: no account record",
            ));
        }
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            return Err(AdapterError::unavailable(
                "the current account has no home directory",
            ));
        }
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            return Err(AdapterError::unavailable(
                "the current account has no home directory",
            ));
        }
        return Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())));
    }
}

pub(crate) fn resolve_harness_executable(kind: &str) -> Result<PathBuf> {
    if kind.is_empty()
        || !kind.as_bytes()[0].is_ascii_lowercase()
        || !kind
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(AdapterError::unavailable(
            "custom harness name must contain lowercase ASCII letters, digits, or hyphens",
        ));
    }
    if kind == "muse" {
        if let Some(configured) = std::env::var_os("AGENTCTL_MUSE_BIN") {
            let path = PathBuf::from(configured);
            if !path.is_absolute() {
                return Err(AdapterError::unavailable(
                    "AGENTCTL_MUSE_BIN must be an absolute path",
                ));
            }
            let resolved = fs::canonicalize(&path).map_err(|error| {
                AdapterError::unavailable(format!(
                    "cannot inspect muse executable {}: {error}",
                    path.display()
                ))
            })?;
            let metadata = fs::metadata(&resolved).map_err(|error| {
                AdapterError::unavailable(format!(
                    "cannot inspect muse executable {}: {error}",
                    resolved.display()
                ))
            })?;
            if !metadata.is_file()
                || metadata.permissions().mode() & 0o111 == 0
                || metadata.permissions().mode() & 0o022 != 0
            {
                return Err(AdapterError::unavailable(format!(
                    "refusing unsafe muse executable: {}",
                    resolved.display()
                )));
            }
            return Ok(resolved);
        }
    }
    let home = account_home()?;
    let candidates = [
        PathBuf::from("/usr/local/bin").join(kind),
        PathBuf::from("/usr/bin").join(kind),
        home.join(".local/bin").join(kind),
        home.join("bin").join(kind),
        home.join(".cargo/bin").join(kind),
    ];
    for path in candidates {
        if fs::metadata(&path).is_ok_and(|metadata| {
            metadata.is_file()
                && metadata.permissions().mode() & 0o111 != 0
                && metadata.permissions().mode() & 0o022 == 0
        }) {
            return fs::canonicalize(&path).map_err(|error| {
                AdapterError::unavailable(format!(
                    "cannot resolve custom harness executable {kind:?}: {error}"
                ))
            });
        }
    }
    Err(AdapterError::unavailable(format!(
        "custom harness executable {kind:?} was not found in fixed install locations"
    )))
}

fn shell_join(arguments: &[String]) -> Result<String> {
    let mut quoted = Vec::with_capacity(arguments.len());
    for value in arguments {
        if value.contains('\0') {
            return Err(AdapterError::unavailable(
                "harness arguments must contain no NUL",
            ));
        }
        if !value.is_empty()
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b'/' | b':' | b'=' | b',' | b'+')
            })
        {
            quoted.push(value.clone());
        } else {
            quoted.push(format!("'{}'", value.replace('\'', "'\\''")));
        }
    }
    Ok(quoted.join(" "))
}
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn detail(output: &CommandOutput) -> String {
    let detail = if output.stderr.trim().is_empty() {
        output.stdout.trim()
    } else {
        output.stderr.trim()
    };
    if detail.is_empty() {
        format!("exit {}", output.status)
    } else {
        detail.to_owned()
    }
}

fn stderr_detail(output: &CommandOutput) -> String {
    let detail = output.stderr.trim();
    if detail.is_empty() {
        format!("exit {}", output.status)
    } else {
        detail.to_owned()
    }
}

fn value_array<'a>(value: Option<&'a Value>, what: &str) -> Result<&'a [Value]> {
    match value {
        Some(Value::Array(values)) => Ok(values),
        None | Some(_) => Err(AdapterError::unavailable(format!(
            "{what}: expected an array"
        ))),
    }
}

fn object_entries(values: &[Value], what: &str) -> Result<Vec<Map<String, Value>>> {
    values
        .iter()
        .map(|value| {
            value
                .as_object()
                .cloned()
                .ok_or_else(|| AdapterError::unavailable(format!("{what}: expected an object")))
        })
        .collect()
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .ok_or_else(|| AdapterError::unavailable(format!("{what}: {key:?} is not an object")))
}

fn required_string(object: &Map<String, Value>, key: &str, what: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| AdapterError::unavailable(format!("{what}: {key:?} is not a string")))
}

fn optional_string(object: &Map<String, Value>, key: &str, what: &str) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(AdapterError::unavailable(format!(
            "{what}: {key:?} is not a string"
        ))),
    }
}

fn unique_label_id(
    entries: &[Map<String, Value>],
    id_key: &str,
    label: &str,
    kind: &str,
) -> Result<Option<String>> {
    let mut matches = Vec::new();
    for entry in entries {
        if optional_string(entry, "label", &format!("{kind} list entry"))?.as_deref() == Some(label)
        {
            matches.push(required_string(
                entry,
                id_key,
                &format!("{kind} list entry"),
            )?);
        }
    }
    if matches.len() > 1 {
        return Err(AdapterError::unavailable(format!(
            "{kind} label {label:?} is ambiguous: {} matching IDs",
            matches.len()
        )));
    }
    Ok(matches.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::process::Child;
    #[cfg(target_os = "linux")]
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    #[cfg(target_os = "linux")]
    use std::sync::Arc;
    #[cfg(target_os = "linux")]
    use std::thread;

    #[cfg(target_os = "linux")]
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    #[cfg(target_os = "linux")]
    static EXECUTABLE_FIXTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    #[cfg(target_os = "linux")]
    static SIGNAL_FIXTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn muse_prompt_must_move_from_composer_to_transcript() {
        let prompt = "literal $(unexpanded) delivery\nsecond line";
        let header = "Muse Code 1.3.0\n";
        let divider = "────────────────\n";
        let footer = "watermelon-preview · xhigh · /work/project · Auto-review\n";
        let staged = format!("{header}{divider}❯ {prompt}\n{divider}{footer}");
        let accepted = format!("{header}❯ {prompt}\nWorking...\n{divider}❯\n{divider}{footer}");
        let error_redraw = format!("{header}{divider}❯ {prompt}\nError: retry\n{divider}{footer}");
        assert!(muse_prompt_in_composer(&staged, prompt));
        assert_eq!(muse_prompt_transcript_count(&staged, prompt), 0);
        assert_eq!(muse_prompt_transcript_count(&accepted, prompt), 1);
        assert!(!muse_prompt_in_composer(&accepted, prompt));
        assert!(muse_prompt_in_composer(&error_redraw, prompt));
        assert_eq!(muse_prompt_transcript_count(&error_redraw, prompt), 0);
        let repeated_staged =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯ {prompt}\n{divider}{footer}");
        let cleared_without_submit =
            format!("{header}❯ {prompt}\n◆ prior answer\n{divider}❯\n{divider}{footer}");
        assert_eq!(muse_prompt_transcript_count(&repeated_staged, prompt), 1);
        assert_eq!(
            muse_prompt_transcript_count(&cleared_without_submit, prompt),
            1
        );
    }

    #[cfg(target_os = "linux")]
    struct FakeExecutable {
        root: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }
    #[cfg(target_os = "linux")]
    impl FakeExecutable {
        fn new(response: &str) -> Self {
            let guard = EXECUTABLE_FIXTURE.lock().unwrap();
            let root = std::env::temp_dir().join(format!(
                "agentctl-adapter-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let fixture = Self {
                root,
                _guard: guard,
            };
            fixture.set_response(response);
            fixture
        }
        fn set_response(&self, response: &str) {
            let script = format!("#!/usr/bin/python3\nimport json, pathlib, sys\npathlib.Path(__file__).with_name('args').write_text(json.dumps(sys.argv[1:]))\nprint({response:?})\n");
            fs::write(self.root.join("herdr"), script).unwrap();
            fs::set_permissions(self.root.join("herdr"), fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        fn client(&self) -> HerdrClient {
            HerdrClient::with_executable("direct", &self.root.join("herdr")).unwrap()
        }
        fn arguments(&self) -> Value {
            serde_json::from_slice(&fs::read(self.root.join("args")).unwrap()).unwrap()
        }
    }
    #[cfg(target_os = "linux")]
    impl Drop for FakeExecutable {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(target_os = "linux")]
    struct RecordedChild {
        root: PathBuf,
        group_pid_file: PathBuf,
        escaped_pid_file: PathBuf,
    }

    #[cfg(target_os = "linux")]
    impl RecordedChild {
        fn new(kind: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-{kind}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let group_pid_file = root.join("group-pid");
            let escaped_pid_file = root.join("escaped-pid");
            Self {
                root,
                group_pid_file,
                escaped_pid_file,
            }
        }

        fn escaped_pipe_command(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "/usr/bin/setsid /bin/sleep 30 & printf '%s\\n' \"$!\" > \"$1\"; exit 0",
                    "agentctl-test",
                ])
                .arg(&self.escaped_pid_file);
            command
        }

        fn mixed_descendant_command(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "/bin/sleep 30 & printf '%s\\n' \"$!\" > \"$1\"; /usr/bin/setsid /bin/sleep 30 & printf '%s\\n' \"$!\" > \"$2\"; wait",
                    "agentctl-test",
                ])
                .arg(&self.group_pid_file)
                .arg(&self.escaped_pid_file);
            command
        }

        fn group_stdout_overflow_command(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "/bin/sleep 30 & printf '%s\\n' \"$!\" > \"$1\"; exec /usr/bin/head -c \"$2\" /dev/zero",
                    "agentctl-test",
                ])
                .arg(&self.group_pid_file)
                .arg((CONTROL_STDOUT_BYTES + 1).to_string());
            command
        }

        fn read_pid(path: &Path) -> libc::pid_t {
            fs::read_to_string(path)
                .expect("fixture child wrote its pid")
                .trim()
                .parse()
                .expect("fixture child pid is numeric")
        }

        fn group_pid(&self) -> libc::pid_t {
            Self::read_pid(&self.group_pid_file)
        }

        fn escaped_pid(&self) -> libc::pid_t {
            Self::read_pid(&self.escaped_pid_file)
        }

        fn terminate_escaped(&self) {
            let pid = self.escaped_pid();
            // SAFETY: the fixture recorded this exact process immediately before this call.
            assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
            assert!(
                wait_until_process_is_gone(pid, Duration::from_secs(2)),
                "escaped pipe holder must be cleaned after the assertion"
            );
            fs::remove_file(&self.escaped_pid_file).expect("retire escaped pid marker");
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for RecordedChild {
        fn drop(&mut self) {
            if let Ok(value) = fs::read_to_string(&self.escaped_pid_file) {
                if let Ok(pid) = value.trim().parse::<libc::pid_t>() {
                    // SAFETY: this fixture recorded the exact process it created and is solely
                    // responsible for bounding that deliberately escaped test descendant.
                    let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(target_os = "linux")]
    struct ChildGuard(Child);

    #[cfg(target_os = "linux")]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(target_os = "linux")]
    fn install_test_elf(source: &str, destination: &Path) {
        fs::copy(source, destination).unwrap();
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[cfg(target_os = "linux")]
    fn spawn_test_process(executable: &Path) -> ChildGuard {
        let mut command = Command::new(executable);
        command.arg("30").process_group(0);
        ChildGuard(command.spawn().expect("start test executable"))
    }

    #[cfg(target_os = "linux")]
    fn process_info_response(pid: u32, executable: &Path) -> String {
        serde_json::json!({
            "result": {
                "process_info": {
                    "pane_id": "pane",
                    "foreground_process_group_id": pid,
                    "foreground_processes": [{
                        "pid": pid,
                        "argv": [executable.display().to_string(), "30"]
                    }]
                }
            }
        })
        .to_string()
    }

    #[cfg(target_os = "linux")]
    extern "C" fn test_signal_handler(_signal: libc::c_int) {}

    #[cfg(target_os = "linux")]
    struct SignalHandlerGuard {
        previous_action: libc::sigaction,
        previous_mask: libc::sigset_t,
    }

    #[cfg(target_os = "linux")]
    impl SignalHandlerGuard {
        fn install() -> Self {
            // SAFETY: zero is a valid starting representation for sigaction/sigset_t before the
            // libc initialization calls below fill their public fields.
            let mut action = unsafe { std::mem::zeroed::<libc::sigaction>() };
            action.sa_sigaction = test_signal_handler as *const () as usize;
            // SAFETY: action owns writable mask storage.
            assert_eq!(unsafe { libc::sigemptyset(&mut action.sa_mask) }, 0);
            let mut previous_action = std::mem::MaybeUninit::<libc::sigaction>::uninit();
            // SAFETY: both action pointers remain live and sigaction initializes the old action.
            assert_eq!(
                unsafe { libc::sigaction(libc::SIGUSR1, &action, previous_action.as_mut_ptr()) },
                0
            );
            // SAFETY: successful sigaction initialized previous_action.
            let previous_action = unsafe { previous_action.assume_init() };

            // Ensure the test thread can receive the targeted signal even if its harness parent
            // happened to block SIGUSR1, and restore its exact mask afterward.
            // SAFETY: zeroed sigset_t is initialized by sigemptyset before use.
            let mut signal_set = unsafe { std::mem::zeroed::<libc::sigset_t>() };
            // SAFETY: signal_set is writable and SIGUSR1 is valid.
            assert_eq!(unsafe { libc::sigemptyset(&mut signal_set) }, 0);
            assert_eq!(
                unsafe { libc::sigaddset(&mut signal_set, libc::SIGUSR1) },
                0
            );
            let mut previous_mask = std::mem::MaybeUninit::<libc::sigset_t>::uninit();
            // SAFETY: signal_set is initialized and previous_mask is writable.
            assert_eq!(
                unsafe {
                    libc::pthread_sigmask(
                        libc::SIG_UNBLOCK,
                        &signal_set,
                        previous_mask.as_mut_ptr(),
                    )
                },
                0
            );
            // SAFETY: successful pthread_sigmask initialized previous_mask.
            let previous_mask = unsafe { previous_mask.assume_init() };
            Self {
                previous_action,
                previous_mask,
            }
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for SignalHandlerGuard {
        fn drop(&mut self) {
            // SAFETY: both saved values came from successful libc queries in install.
            assert_eq!(
                unsafe {
                    libc::pthread_sigmask(
                        libc::SIG_SETMASK,
                        &self.previous_mask,
                        std::ptr::null_mut(),
                    )
                },
                0
            );
            // SAFETY: previous_action was initialized by sigaction for this same signal.
            assert_eq!(
                unsafe {
                    libc::sigaction(libc::SIGUSR1, &self.previous_action, std::ptr::null_mut())
                },
                0
            );
        }
    }

    #[cfg(target_os = "linux")]
    fn wait_until_process_is_gone(pid: libc::pid_t, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            // SAFETY: signal zero only probes the numeric identity recorded by this test fixture.
            if unsafe { libc::kill(pid, 0) } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn allocation_arguments_preserve_literal_values() {
        let environment = [
            "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai".to_owned(),
            "LITERAL=a b=$(unexpanded)=tail".to_owned(),
        ];
        assert_eq!(
            tab_create_arguments("workspace", "worker", "/tmp", &environment),
            strings(&[
                "tab",
                "create",
                "--workspace",
                "workspace",
                "--label",
                "worker",
                "--cwd",
                "/tmp",
                "--env",
                "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
                "--env",
                "LITERAL=a b=$(unexpanded)=tail",
                "--no-focus",
            ])
        );
        assert_eq!(
            workspace_create_arguments("subagents", "/tmp", &environment),
            strings(&[
                "workspace",
                "create",
                "--label",
                "subagents",
                "--cwd",
                "/tmp",
                "--env",
                "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
                "--env",
                "LITERAL=a b=$(unexpanded)=tail",
                "--no-focus",
            ])
        );
    }

    #[test]
    fn pane_identity_parser_rejects_a_different_pane() {
        let document = serde_json::json!({
            "pane": {
                "pane_id": "other",
                "workspace_id": "workspace",
                "cwd": "/tmp",
                "agent": "codex",
                "agent_status": "idle"
            }
        });
        let result = document.as_object().expect("fixture object");
        let error = parse_pane_info(result, "expected").expect_err("mismatch must be refused");
        assert!(error.to_string().contains("other"));
        assert!(error.to_string().contains("expected"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cancellation_and_timeout_preserve_primary_kind_with_cleanup_uncertainty() {
        for (cancelled, timeout, expected_kind, primary_text) in [
            (
                true,
                Duration::from_secs(30),
                io::ErrorKind::Interrupted,
                "control command was cancelled",
            ),
            (
                false,
                Duration::ZERO,
                io::ErrorKind::TimedOut,
                "control command timed out",
            ),
        ] {
            let mut shutdown = |_child: &mut ProcessPluginChild, _grace: Duration| {
                Err(io::Error::other("injected shutdown failure"))
            };
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            let error = bounded_output_with_cancellation_and_shutdown(
                command,
                timeout,
                &|| cancelled,
                &mut shutdown,
            )
            .expect_err("primary stop plus failed shutdown must be reported");
            assert_eq!(error.kind(), expected_kind);
            let message = error.to_string();
            assert!(message.contains(primary_text), "{message}");
            assert!(
                message.contains("process cleanup is uncertain"),
                "{message}"
            );
            assert!(message.contains("injected shutdown failure"), "{message}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn poll_interruptions_do_not_extend_the_absolute_deadline() {
        let _fixture = SIGNAL_FIXTURE.lock().expect("lock signal fixture");
        let _handler = SignalHandlerGuard::install();
        let target = unsafe { libc::pthread_self() };
        let stop = Arc::new(AtomicBool::new(false));
        let sender_stop = Arc::clone(&stop);
        let sender = thread::spawn(move || {
            while !sender_stop.load(Ordering::SeqCst) {
                // SAFETY: target names the live test thread until stop is set and this worker is
                // joined. SIGUSR1 has the no-op handler installed above.
                assert_eq!(unsafe { libc::pthread_kill(target, libc::SIGUSR1) }, 0);
                thread::sleep(Duration::from_millis(1));
            }
        });
        let interrupts_before = POLL_CAPTURE_INTERRUPTS.load(Ordering::SeqCst);
        let started = Instant::now();
        let deadline = started + Duration::from_millis(150);
        while Instant::now() < deadline {
            poll_capture(
                None,
                None,
                deadline.saturating_duration_since(Instant::now()),
            )
            .expect("EINTR returns control to the absolute-deadline loop");
        }
        let elapsed = started.elapsed();
        stop.store(true, Ordering::SeqCst);
        sender.join().expect("join signal sender");
        assert!(elapsed >= Duration::from_millis(100), "elapsed={elapsed:?}");
        assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
        assert!(
            POLL_CAPTURE_INTERRUPTS.load(Ordering::SeqCst) > interrupts_before,
            "fixture must causally exercise poll's EINTR branch"
        );
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn bounded_capture_refuses_before_executing_on_unsupported_platforms() {
        let error = bounded_output(Command::new("must-not-run"), Duration::from_secs(1))
            .expect_err("non-Linux control capture must be refused");
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn tab_allocation_captures_root_pane_without_a_followup_query() {
        let executable = FakeExecutable::new(
            r#"{"result":{"tab":{"tab_id":"tab"},"root_pane":{"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}}}"#,
        );
        assert_eq!(
            executable
                .client()
                .create_tab_with_pane(
                    "workspace",
                    "worker",
                    "/tmp",
                    &[
                        "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai".to_owned(),
                        "LITERAL=a b=$(unexpanded)=tail".to_owned(),
                    ],
                )
                .unwrap(),
            ("tab".to_owned(), "pane".to_owned())
        );
        assert_eq!(
            executable.arguments(),
            serde_json::json!([
                "tab",
                "create",
                "--workspace",
                "workspace",
                "--label",
                "worker",
                "--cwd",
                "/tmp",
                "--env",
                "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
                "--env",
                "LITERAL=a b=$(unexpanded)=tail",
                "--no-focus"
            ])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn workspace_allocation_passes_literal_environment() {
        let executable = FakeExecutable::new(
            r#"{"result":{"workspace":{"workspace_id":"workspace"},"tab":{"tab_id":"tab"},"root_pane":{"pane_id":"pane"}}}"#,
        );
        assert_eq!(
            executable
                .client()
                .create_workspace(
                    "subagents",
                    "/tmp",
                    &[
                        "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai".to_owned(),
                        "LITERAL=a b=$(unexpanded)=tail".to_owned(),
                    ],
                )
                .unwrap(),
            ("workspace".to_owned(), "tab".to_owned(), "pane".to_owned())
        );
        assert_eq!(
            executable.arguments(),
            serde_json::json!([
                "workspace",
                "create",
                "--label",
                "subagents",
                "--cwd",
                "/tmp",
                "--env",
                "META_CODEX_AI_GATEWAY=azure-codex-cyber:openai",
                "--env",
                "LITERAL=a b=$(unexpanded)=tail",
                "--no-focus"
            ])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bounded_capture_preserves_stdout_stderr_and_exit_status() {
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "printf 'ordinary stdout'; printf 'ordinary stderr' >&2; exit 7",
        ]);
        let output = bounded_output(command, Duration::from_secs(5)).unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"ordinary stdout");
        assert_eq!(output.stderr, b"ordinary stderr");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn focus_uses_agent_focus_and_shutdown_targets_one_exact_pane() {
        let executable = FakeExecutable::new("{}");
        executable.client().focus_pane("workspace:pane").unwrap();
        assert_eq!(
            executable.arguments(),
            serde_json::json!(["agent", "focus", "workspace:pane"])
        );
        executable.client().close_pane("workspace:pane").unwrap();
        assert_eq!(
            executable.arguments(),
            serde_json::json!(["pane", "close", "workspace:pane"])
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recorded_pane_shell_identity_refuses_every_generation_change() {
        let executable = fs::canonicalize("/bin/bash").unwrap();
        let mut command = Command::new(&executable);
        command
            .args(["--noprofile", "--norc"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        let shell = ChildGuard(command.spawn().expect("start real test shell"));
        let pid = shell.0.id();
        let response = |argv0: &Path| {
            serde_json::json!({
                "result": {
                    "process_info": {
                        "pane_id": "pane",
                        "shell_pid": pid,
                        "foreground_process_group_id": pid,
                        "foreground_processes": [{
                            "pid": pid,
                            "argv": [argv0.display().to_string()]
                        }]
                    }
                }
            })
            .to_string()
        };
        let herdr = FakeExecutable::new(&response(&executable));
        let identity = herdr.client().pane_shell_identity("pane").unwrap();
        assert!(herdr
            .client()
            .pane_is_same_idle_shell("pane", &identity)
            .unwrap());

        let mut mismatches = Vec::new();
        let mut value = identity.clone();
        value.boot_id = "ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned();
        mismatches.push(value);
        let mut value = identity.clone();
        value.pid += 1;
        mismatches.push(value);
        let mut value = identity.clone();
        value.starttime_ticks += 1;
        mismatches.push(value);
        let mut value = identity.clone();
        value.executable_device += 1;
        mismatches.push(value);
        let mut value = identity.clone();
        value.executable_inode += 1;
        mismatches.push(value);
        for mismatch in mismatches {
            assert!(!herdr
                .client()
                .pane_is_same_idle_shell("pane", &mismatch)
                .unwrap());
        }

        herdr.set_response(&response(Path::new("/usr/bin/sleep")));
        assert!(!herdr
            .client()
            .pane_is_same_idle_shell("pane", &identity)
            .unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn real_non_shell_process_cannot_be_adopted_as_a_pane_shell() {
        let executable = fs::canonicalize("/usr/bin/sleep").unwrap();
        let process = spawn_test_process(&executable);
        let pid = process.0.id();
        let response = serde_json::json!({
            "result": {
                "process_info": {
                    "pane_id": "pane",
                    "shell_pid": pid,
                    "foreground_process_group_id": pid,
                    "foreground_processes": [{
                        "pid": pid,
                        "argv": [executable.display().to_string(), "30"]
                    }]
                }
            }
        })
        .to_string();
        let herdr = FakeExecutable::new(&response);
        let identity = live_custom_process(u64::from(pid)).unwrap().identity;

        let error = herdr.client().pane_shell_identity("pane").unwrap_err();
        assert!(error.to_string().contains("supported identity-bound shell"));
        assert!(!herdr.client().pane_is_idle_shell("pane").unwrap());
        assert!(!herdr
            .client()
            .pane_is_same_idle_shell("pane", &identity)
            .unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recorded_custom_identity_survives_atomic_pathname_replacement() {
        let herdr = FakeExecutable::new("{}");
        let harness = herdr.root.join("muse");
        install_test_elf("/bin/sleep", &harness);
        let pinned = pin_harness_executable(harness.clone()).unwrap();
        let process = spawn_test_process(&harness);
        let pid = process.0.id();
        herdr.set_response(&process_info_response(pid, &harness));
        let identity = herdr
            .client()
            .custom_harness_observed("pane", &pinned, &|| false)
            .unwrap()
            .expect("observe the pinned launch image");

        let replacement = herdr.root.join("muse.replacement");
        install_test_elf("/bin/true", &replacement);
        fs::rename(&replacement, &harness).unwrap();

        herdr
            .client()
            .verify_custom_harness("pane", "muse", Some(&identity))
            .expect("verification uses the recorded image, not the replaced pathname");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn recorded_custom_identity_refuses_replacement_process_and_field_mismatches() {
        let herdr = FakeExecutable::new("{}");
        let harness = herdr.root.join("muse");
        install_test_elf("/bin/sleep", &harness);
        let pinned = pin_harness_executable(harness.clone()).unwrap();
        let mut original = spawn_test_process(&harness);
        let original_pid = original.0.id();
        herdr.set_response(&process_info_response(original_pid, &harness));
        let identity = herdr
            .client()
            .custom_harness_observed("pane", &pinned, &|| false)
            .unwrap()
            .expect("observe original process");

        let mut mismatches = Vec::new();
        let mut value = identity.clone();
        value.boot_id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_owned();
        mismatches.push(value);
        let mut value = identity.clone();
        value.pid = value.pid.saturating_add(1);
        mismatches.push(value);
        let mut value = identity.clone();
        value.starttime_ticks += 1;
        mismatches.push(value);
        let mut value = identity.clone();
        value.executable_device += 1;
        mismatches.push(value);
        let mut value = identity.clone();
        value.executable_inode += 1;
        mismatches.push(value);
        for mismatch in mismatches {
            assert!(herdr
                .client()
                .verify_custom_harness("pane", "muse", Some(&mismatch))
                .is_err());
        }
        let wrong_group = serde_json::json!({
            "result": {
                "process_info": {
                    "pane_id": "pane",
                    "foreground_process_group_id": u64::from(original_pid) + 1,
                    "foreground_processes": [{
                        "pid": original_pid,
                        "argv": [harness.display().to_string(), "30"]
                    }]
                }
            }
        })
        .to_string();
        herdr.set_response(&wrong_group);
        assert!(herdr
            .client()
            .verify_custom_harness("pane", "muse", Some(&identity))
            .is_err());
        let duplicate = serde_json::json!({
            "result": {
                "process_info": {
                    "pane_id": "pane",
                    "foreground_process_group_id": original_pid,
                    "foreground_processes": [
                        {"pid": original_pid, "argv": [harness.display().to_string(), "30"]},
                        {"pid": u64::from(original_pid) + 1, "argv": [harness.display().to_string(), "30"]}
                    ]
                }
            }
        })
        .to_string();
        herdr.set_response(&duplicate);
        assert!(herdr
            .client()
            .custom_harness_observed("pane", &pinned, &|| false)
            .is_err());

        let duplicate_identity = serde_json::json!({
            "result": {
                "process_info": {
                    "pane_id": "pane",
                    "foreground_process_group_id": original_pid,
                    "foreground_processes": [
                        {"pid": original_pid, "argv": [harness.display().to_string(), "30"]},
                        {"pid": original_pid, "argv": [harness.display().to_string(), "30"]}
                    ]
                }
            }
        })
        .to_string();
        herdr.set_response(&duplicate_identity);
        assert!(herdr
            .client()
            .verify_custom_harness("pane", "muse", Some(&identity))
            .is_err());

        original.0.kill().unwrap();
        original.0.wait().unwrap();
        let replacement = herdr.root.join("muse.replacement");
        install_test_elf("/bin/sleep", &replacement);
        fs::rename(&replacement, &harness).unwrap();
        let replacement = spawn_test_process(&harness);
        herdr.set_response(&process_info_response(replacement.0.id(), &harness));
        assert!(herdr
            .client()
            .verify_custom_harness("pane", "muse", Some(&identity))
            .is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn custom_harness_pin_refuses_interpreter_payloads() {
        let herdr = FakeExecutable::new("{}");
        let harness = herdr.root.join("muse-script");
        fs::write(&harness, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&harness, fs::Permissions::from_mode(0o700)).unwrap();
        let error = pin_harness_executable(harness).unwrap_err();
        assert!(error.to_string().contains("ELF image"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mismatched_pane_identity_is_rejected() {
        let executable = FakeExecutable::new(
            r#"{"result":{"pane":{"pane_id":"other","workspace_id":"workspace","cwd":"/tmp","agent":"codex","agent_status":"idle"}}}"#,
        );
        assert!(executable
            .client()
            .pane_info("expected")
            .unwrap_err()
            .to_string()
            .contains("expected"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn control_timeout_includes_pipes_inherited_after_the_parent_exits() {
        let escaped = RecordedChild::new("setsid-holder");
        let mut unrelated = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let started = Instant::now();
        // The timeout remains explicit, but leaves enough startup budget for the fixture to
        // prove that its escaped descendant actually inherited the capture pipes under a loaded
        // parallel test harness. The separate EINTR test exercises the short 150ms deadline.
        let result = bounded_output(escaped.escaped_pipe_command(), Duration::from_secs(1));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        // The setsid child escaped the supervised group and really was retaining both capture
        // pipes, so returning promptly did not depend on receiving EOF from reader threads.
        assert_eq!(unsafe { libc::kill(escaped.escaped_pid(), 0) }, 0);
        // The pinned private group must not be confused with any unrelated numeric identity.
        assert!(unrelated.0.try_wait().unwrap().is_none());
        escaped.terminate_escaped();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn control_cancellation_kills_command_group_and_inherited_pipes_promptly() {
        let escaped = RecordedChild::new("setsid-holder");
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let group_pid_file = escaped.group_pid_file.clone();
        let escaped_pid_file = escaped.escaped_pid_file.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let markers_are_complete =
                    [&group_pid_file, &escaped_pid_file].iter().all(|path| {
                        fs::read_to_string(path)
                            .ok()
                            .and_then(|value| value.trim().parse::<libc::pid_t>().ok())
                            .is_some()
                    });
                if markers_are_complete || Instant::now() >= deadline {
                    trigger.store(true, Ordering::SeqCst);
                    return markers_are_complete;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let started = Instant::now();
        let result = bounded_output_with_cancellation(
            escaped.mixed_descendant_command(),
            Duration::from_secs(30),
            &|| cancelled.load(Ordering::SeqCst),
        );
        assert!(
            worker.join().expect("join cancellation trigger"),
            "cancellation fixture must record the escaped pipe holder before cancellation"
        );
        assert_eq!(
            result.expect_err("cancel command").kind(),
            io::ErrorKind::Interrupted
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            wait_until_process_is_gone(escaped.group_pid(), Duration::from_secs(2)),
            "cancellation must kill a live descendant in the supervised group"
        );
        assert_eq!(unsafe { libc::kill(escaped.escaped_pid(), 0) }, 0);
        escaped.terminate_escaped();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn control_capture_enforces_independent_stream_byte_caps() {
        let group_child = RecordedChild::new("group-holder");
        let stdout_error = bounded_output(
            group_child.group_stdout_overflow_command(),
            Duration::from_secs(5),
        )
        .expect_err("oversized stdout must be refused");
        assert_eq!(stdout_error.kind(), io::ErrorKind::InvalidData);
        assert!(stdout_error.to_string().contains("stdout"));
        assert!(
            wait_until_process_is_gone(group_child.group_pid(), Duration::from_secs(2)),
            "output-cap refusal must terminate a descendant in the supervised group"
        );

        let mut stderr_command = Command::new("/bin/sh");
        stderr_command
            .args([
                "-c",
                "exec /usr/bin/head -c \"$1\" /dev/zero >&2",
                "agentctl-test",
            ])
            .arg((CONTROL_STDERR_BYTES + 1).to_string());
        let stderr_error = bounded_output(stderr_command, Duration::from_secs(5))
            .expect_err("oversized stderr must be refused");
        assert_eq!(stderr_error.kind(), io::ErrorKind::InvalidData);
        assert!(stderr_error.to_string().contains("stderr"));
    }
}
