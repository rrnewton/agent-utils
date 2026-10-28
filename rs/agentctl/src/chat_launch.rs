//! One-command coordinator launch from an existing Herdr shell pane.
//!
//! Bind the bridge configuration to the invoking shell pane, spawn the
//! coordinator harness and the bridge (`chat run` of this same
//! executable) under exact pidfd supervision, and run the bridge for
//! the coordinator's lifetime. Children are spawned through an inert
//! gate (parent-death lifeline armed and readiness published before
//! activation), so a child can never run unsupervised if this process
//! dies first.
//!
//! This drives the current bridge generation (`chat init`/`run`
//! state). A polling-bridge config or state directory in the retired
//! format is refused, never converted or read: create a fresh state
//! with `agentctl chat init` instead.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::chat_runtime::BridgeConfiguration;
use crate::chat_service::ChatServiceError;
use crate::client::HerdrClient;
use crate::subagents::ManagedAgents;

type Result<T> = std::result::Result<T, ChatServiceError>;

const GATE_READY_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_COALESCE: Duration = Duration::from_millis(200);
const LOG_SEGMENT_BYTES: u64 = 1 << 20;
const AGENT_READY_TIMEOUT: Duration = Duration::from_secs(30);

fn fail(message: impl Into<String>) -> ChatServiceError {
    ChatServiceError::Generation(message.into())
}

#[derive(Clone, Debug)]
struct ProcIdentity {
    pid: i64,
    starttime: u64,
    ppid: i64,
    pgrp: i64,
    session: i64,
    state: String,
}

fn proc_identity(pid: i64) -> Option<ProcIdentity> {
    let raw = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    let text = String::from_utf8_lossy(&raw);
    let closing = text.rfind(") ")?;
    let fields: Vec<&str> = text[closing + 2..].split_whitespace().collect();
    Some(ProcIdentity {
        pid,
        state: fields.first()?.to_string(),
        ppid: fields.get(1)?.parse().ok()?,
        pgrp: fields.get(2)?.parse().ok()?,
        session: fields.get(3)?.parse().ok()?,
        starttime: fields.get(19)?.parse().ok()?,
    })
}

fn pidfd_open(pid: i64) -> Result<OwnedFd> {
    // SAFETY: pidfd_open takes scalar arguments and returns a fresh descriptor.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0_u32) };
    if descriptor < 0 {
        return Err(ChatServiceError::Io(std::io::Error::last_os_error()));
    }
    let descriptor = i32::try_from(descriptor)
        .map_err(|_| ChatServiceError::Io(std::io::Error::other("pidfd_open descriptor")))?;
    // SAFETY: the successful syscall returned a fresh descriptor owned here.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn pidfd_signal(pidfd: &OwnedFd, signum: i32) {
    // SAFETY: pidfd addresses a stable process identity; null siginfo is the
    // documented ordinary signal form. ESRCH (already gone) is ignored.
    unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signum,
            std::ptr::null::<libc::siginfo_t>(),
            0_u32,
        );
    }
}

fn wait_fd(fd: i32, timeout: Option<Duration>) -> bool {
    loop {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let millis = timeout
            .map(|limit| i32::try_from(limit.as_millis().clamp(1, i32::MAX as u128)).unwrap_or(-1))
            .unwrap_or(-1);
        // SAFETY: one initialized pollfd, retained for the call.
        let result = unsafe { libc::poll(&raw mut descriptor, 1, millis) };
        if result > 0 {
            return descriptor.revents != 0;
        }
        if result == 0 {
            return false;
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return false;
        }
    }
}

fn pipe_pair() -> Result<(OwnedFd, OwnedFd)> {
    let mut descriptors = [0_i32; 2];
    // SAFETY: pipe2 writes two fresh descriptors into the array.
    if unsafe { libc::pipe2(descriptors.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(ChatServiceError::Io(std::io::Error::last_os_error()));
    }
    // SAFETY: pipe2 returned two fresh descriptors owned here.
    Ok(unsafe {
        (
            OwnedFd::from_raw_fd(descriptors[0]),
            OwnedFd::from_raw_fd(descriptors[1]),
        )
    })
}

/// Entry point of the inert launch gate (`agentctl --chat-launch-gate`).
///
/// The gate is a separate process: it
/// arms the parent-death lifeline, publishes readiness, holds inert
/// until the launcher activates this exact process, and only then
/// execs the real command, so the child can never run unsupervised.
/// Activation descriptors arrive by number and are inherited (not
/// close-on-exec) from the launcher.
pub fn gate_main(arguments: &[String]) -> i32 {
    let parse = |index: usize| {
        arguments
            .get(index)
            .and_then(|value| value.parse::<i64>().ok())
    };
    let (Some(parent_pid), Some(activation_fd), Some(readiness_fd)) =
        (parse(0), parse(1), parse(2))
    else {
        return 125;
    };
    if arguments.get(3).map(String::as_str) != Some("--") || arguments.len() < 5 {
        return 125;
    }
    let command = &arguments[4..];
    // SAFETY: prctl/getppid take scalar arguments only.
    unsafe {
        if libc::prctl(
            libc::PR_SET_PDEATHSIG,
            libc::SIGKILL as libc::c_ulong,
            0,
            0,
            0,
        ) != 0
        {
            libc::_exit(125);
        }
        if libc::getppid() as i64 != parent_pid {
            libc::_exit(125);
        }
        let ready = b"1";
        if libc::write(
            readiness_fd as i32,
            ready.as_ptr() as *const libc::c_void,
            1,
        ) != 1
        {
            libc::_exit(125);
        }
        let _ = libc::close(readiness_fd as i32);
        let mut byte = [0_u8; 1];
        let read = libc::read(
            activation_fd as i32,
            byte.as_mut_ptr() as *mut libc::c_void,
            1,
        );
        let _ = libc::close(activation_fd as i32);
        if read != 1 || byte != *ready || libc::getppid() as i64 != parent_pid {
            libc::_exit(125);
        }
        // Keep only stdio for the exec, like Python's close_fds spawn.
        libc::syscall(libc::SYS_close_range, 3_u32, u32::MAX, 0_u32);
    }
    use std::os::unix::process::CommandExt as _;
    let error = Command::new(&command[0]).args(&command[1..]).exec();
    let _ = writeln!(std::io::stderr(), "chat launch gate: {error}");
    125
}

struct LaunchChild {
    name: &'static str,
    child: Child,
    pidfd: OwnedFd,
    identity: ProcIdentity,
    activation: Option<OwnedFd>,
    group_leader: bool,
    reaped: bool,
    observed: Option<i32>,
}

impl LaunchChild {
    fn status(&mut self) -> Result<i32> {
        if let Some(code) = self.observed {
            return Ok(code);
        }
        let status = self.child.wait().map_err(ChatServiceError::Io)?;
        self.reaped = true;
        let code = status.code().unwrap_or_else(|| {
            use std::os::unix::process::ExitStatusExt as _;
            -status.signal().unwrap_or(0)
        });
        self.observed = Some(code);
        Ok(code)
    }

    fn stop(&mut self, group: bool) -> Result<i32> {
        if self.reaped {
            return self.status();
        }
        drop(self.activation.take());
        if !wait_fd(self.pidfd.as_raw_fd(), Some(Duration::ZERO)) {
            if group {
                signal_group(self, libc::SIGTERM);
            } else {
                pidfd_signal(&self.pidfd, libc::SIGTERM);
            }
            if !wait_fd(self.pidfd.as_raw_fd(), Some(Duration::from_secs(10))) {
                if group {
                    signal_group(self, libc::SIGKILL);
                } else {
                    pidfd_signal(&self.pidfd, libc::SIGKILL);
                }
                if !wait_fd(self.pidfd.as_raw_fd(), Some(Duration::from_secs(5))) {
                    return Err(fail(format!(
                        "chat launch {} did not exit after SIGKILL",
                        self.name
                    )));
                }
            }
        }
        if group {
            // Kill any same-group descendants that outlived the leader.
            signal_group(self, libc::SIGKILL);
        }
        self.status()
    }

    fn close(&mut self) {
        drop(self.activation.take());
        if !self.reaped {
            pidfd_signal(&self.pidfd, libc::SIGKILL);
            let _ = wait_fd(self.pidfd.as_raw_fd(), Some(Duration::from_secs(2)));
            let _ = self.status();
        }
    }
}

fn signal_group(child: &LaunchChild, signum: i32) {
    let observed = proc_identity(child.identity.pid);
    let valid = !child.reaped
        && child.group_leader
        && observed
            .as_ref()
            .map(|now| {
                now.starttime == child.identity.starttime
                    && now.pgrp == child.identity.pid
                    && now.session == child.identity.pid
                    && now.state != "X"
            })
            .unwrap_or(false);
    if !valid {
        pidfd_signal(&child.pidfd, libc::SIGKILL);
        return;
    }
    // SAFETY: killpg targets the validated process group of our own child.
    unsafe {
        libc::killpg(child.identity.pid as libc::pid_t, signum);
    }
}

fn spawn_launch_child(
    name: &'static str,
    command: &[String],
    group_leader: bool,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<LaunchChild> {
    // Gate descriptors are inherited by number; they are close-on-exec
    // everywhere except in the gate child itself, which clears the flag
    // on exactly the two descriptors named in its arguments.
    let mut descriptors = [0_i32; 4];
    // SAFETY: pipe2 writes two fresh descriptors into each pair.
    unsafe {
        if libc::pipe2(descriptors[0..2].as_mut_ptr(), libc::O_CLOEXEC) < 0
            || libc::pipe2(descriptors[2..4].as_mut_ptr(), libc::O_CLOEXEC) < 0
        {
            return Err(ChatServiceError::Io(std::io::Error::last_os_error()));
        }
    }
    let (activation_read, activation_write, readiness_read, readiness_write) = (
        descriptors[0],
        descriptors[1],
        descriptors[2],
        descriptors[3],
    );
    let parent_pid = std::process::id() as i64;
    let executable = std::env::current_exe().map_err(ChatServiceError::Io)?;
    let mut gate_command: Vec<String> = vec![
        executable.to_string_lossy().into_owned(),
        "--chat-launch-gate".to_string(),
        parent_pid.to_string(),
        activation_read.to_string(),
        readiness_write.to_string(),
        "--".to_string(),
    ];
    gate_command.extend(command.iter().cloned());
    let mut builder = Command::new(&gate_command[0]);
    builder
        .args(&gate_command[1..])
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr);
    {
        // SAFETY (pre_exec): the gate child clears close-on-exec only on
        // the two descriptors named in its arguments, and becomes a
        // session leader when asked. Everything else closes at exec, so
        // the gate never holds the launcher's writing ends.
        unsafe {
            builder.pre_exec(move || {
                for descriptor in [activation_read, readiness_write] {
                    if libc::fcntl(descriptor, libc::F_SETFD, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                if group_leader && libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let spawned = builder.spawn();
    // SAFETY: closing our copies of the child's pipe ends.
    unsafe {
        libc::close(activation_read);
        libc::close(readiness_write);
    }
    // SAFETY: the two remaining pipe ends are fresh descriptors owned here.
    let (activation_write_fd, readiness_read_fd) = unsafe {
        (
            OwnedFd::from_raw_fd(activation_write),
            OwnedFd::from_raw_fd(readiness_read),
        )
    };
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => return Err(ChatServiceError::Io(error)),
    };
    let pid = child.id() as i64;
    let cleanup = |child: &mut Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let expected = match proc_identity(pid) {
        Some(identity)
            if identity.ppid == parent_pid
                && !matches!(identity.state.as_str(), "X" | "Z")
                && (!group_leader || (identity.pgrp == pid && identity.session == pid)) =>
        {
            identity
        }
        _ => {
            cleanup(&mut child);
            return Err(fail(format!(
                "chat launch {name} gate did not publish a stable child identity"
            )));
        }
    };
    let pidfd = match pidfd_open(pid) {
        Ok(pidfd) => {
            // SAFETY: fcntl sets close-on-exec on our own descriptor so
            // later gate children do not inherit earlier pidfds.
            unsafe {
                libc::fcntl(pidfd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            }
            pidfd
        }
        Err(error) => {
            cleanup(&mut child);
            return Err(fail(format!(
                "chat launch could not obtain the {name} process descriptor: {error}"
            )));
        }
    };
    let stable = |identity: &ProcIdentity| {
        identity.starttime == expected.starttime
            && identity.ppid == parent_pid
            && !matches!(identity.state.as_str(), "X" | "Z")
            && (!group_leader || (identity.pgrp == pid && identity.session == pid))
    };
    match proc_identity(pid) {
        Some(identity) if stable(&identity) => {}
        _ => {
            cleanup(&mut child);
            return Err(fail(format!(
                "chat launch {name} identity changed before activation"
            )));
        }
    }
    // Wait for readiness, racing the gate's own exit.
    let ready = {
        let mut poll_descriptors = [
            libc::pollfd {
                fd: readiness_read_fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let deadline = Instant::now() + GATE_READY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break false;
            }
            let millis = i32::try_from(remaining.as_millis().clamp(1, 5_000)).unwrap_or(5_000);
            // SAFETY: two initialized pollfds retained for the call.
            let result = unsafe { libc::poll(poll_descriptors.as_mut_ptr(), 2, millis) };
            if result > 0 {
                if poll_descriptors[1].revents != 0 && poll_descriptors[0].revents == 0 {
                    cleanup(&mut child);
                    return Err(fail(format!(
                        "chat launch {name} gate exited before publishing readiness"
                    )));
                }
                break poll_descriptors[0].revents != 0;
            }
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                break false;
            }
        }
    };
    if !ready {
        cleanup(&mut child);
        return Err(fail(format!(
            "chat launch {name} gate did not publish readiness"
        )));
    }
    let mut readiness_file = File::from(readiness_read_fd);
    let mut byte = [0_u8; 1];
    let read = readiness_file.read_exact(&mut byte);
    drop(readiness_file);
    if read.is_err() || byte != *b"1" {
        cleanup(&mut child);
        return Err(fail(format!(
            "chat launch {name} gate published invalid readiness"
        )));
    }
    if wait_fd(pidfd.as_raw_fd(), Some(Duration::ZERO)) {
        cleanup(&mut child);
        return Err(fail(format!(
            "chat launch {name} gate exited after publishing readiness"
        )));
    }
    let identity = match proc_identity(pid) {
        Some(identity) if stable(&identity) => identity,
        _ => {
            cleanup(&mut child);
            return Err(fail(format!(
                "chat launch {name} identity changed after readiness"
            )));
        }
    };
    Ok(LaunchChild {
        name,
        child,
        pidfd,
        identity,
        activation: Some(activation_write_fd),
        group_leader,
        reaped: false,
        observed: None,
    })
}

fn activate(child: &mut LaunchChild) -> Result<()> {
    let Some(activation) = child.activation.take() else {
        return Err(fail(format!(
            "chat launch {} gate was already activated",
            child.name
        )));
    };
    let written = File::from(activation)
        .write(b"1")
        .map_err(ChatServiceError::Io)?;
    if written != 1 {
        return Err(fail(format!(
            "chat launch {} gate rejected activation",
            child.name
        )));
    }
    Ok(())
}

fn open_launch_log(path: &Path) -> Result<File> {
    let file = {
        use std::os::unix::fs::OpenOptionsExt as _;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
            .map_err(ChatServiceError::Io)?
    };
    let metadata = file.metadata().map_err(ChatServiceError::Io)?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::getuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.nlink() != 1
    {
        return Err(fail(format!("unsafe Chat launch log: {}", path.display())));
    }
    file.set_len(0).map_err(ChatServiceError::Io)?;
    Ok(file)
}

fn drain_launch_log(
    mut source: File,
    path: PathBuf,
    failures: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
) {
    let previous = path.with_extension("log.1");
    let mut destination: Option<File> = None;
    let mut size: u64 = 0;
    let setup = (|| -> Result<()> {
        let rotated = open_launch_log(&previous)?;
        drop(rotated);
        destination = Some(open_launch_log(&path)?);
        Ok(())
    })();
    if let Err(error) = setup {
        failures
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(error.to_string());
    }
    let mut buffer = [0_u8; 64 << 10];
    loop {
        match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                if destination.is_some() {
                    let mut offset = 0;
                    let mut failed: Option<String> = None;
                    while offset < count {
                        if size == LOG_SEGMENT_BYTES {
                            if let Some(done) = destination.take() {
                                let _ = done.sync_all();
                            }
                            let _ = std::fs::rename(&path, &previous);
                            match open_launch_log(&path) {
                                Ok(next) => destination = Some(next),
                                Err(error) => {
                                    failed = Some(error.to_string());
                                    break;
                                }
                            }
                            size = 0;
                        }
                        let writable = usize::try_from(LOG_SEGMENT_BYTES - size)
                            .unwrap_or(usize::MAX)
                            .min(count - offset);
                        match destination
                            .as_mut()
                            .expect("destination checked")
                            .write(&buffer[offset..offset + writable])
                        {
                            Ok(0) => {
                                failed = Some("short Chat launch log write".to_string());
                                break;
                            }
                            Ok(written) => {
                                offset += written;
                                size += written as u64;
                            }
                            Err(error) => {
                                failed = Some(error.to_string());
                                break;
                            }
                        }
                    }
                    if let Some(error) = failed {
                        failures
                            .lock()
                            .unwrap_or_else(|poison| poison.into_inner())
                            .push(error.chars().take(2000).collect());
                        destination = None;
                    }
                }
            }
            Err(_) => break,
        }
    }
    if let Some(done) = destination.take() {
        let _ = done.sync_all();
    }
}

fn launch_log_detail(path: &Path, limit: usize) -> String {
    fn suffix(path: &Path, limit: usize) -> Vec<u8> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)
        {
            Ok(file) => file,
            Err(_) => return Vec::new(),
        };
        let Ok(metadata) = file.metadata() else {
            return Vec::new();
        };
        if !metadata.is_file() || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Vec::new();
        }
        let count = usize::try_from(metadata.len().min(limit as u64)).unwrap_or(0);
        let _ = file.seek(std::io::SeekFrom::End(-(i64::try_from(count).unwrap_or(0))));
        let mut data = Vec::with_capacity(count);
        let _ = file.take(count as u64).read_to_end(&mut data);
        data
    }
    let mut current = suffix(path, limit);
    if current.len() < limit {
        let mut previous = suffix(&path.with_extension("log.1"), limit - current.len());
        previous.append(&mut current);
        current = previous;
    }
    String::from_utf8_lossy(&current).trim().to_string()
}

static TERMINATE_FLAG: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

fn terminated_flag() -> bool {
    TERMINATE_FLAG
        .get()
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
}

fn require_waitable_children() -> Result<()> {
    for _ in 0..2 {
        let pidfd = pidfd_open(std::process::id() as i64).map_err(|error| {
            fail(format!(
                "chat launch process descriptors are unavailable: {error}"
            ))
        })?;
        drop(pidfd);
    }
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    // SAFETY: sigaction with a null new action reads the current disposition.
    let result = unsafe { libc::sigaction(libc::SIGCHLD, std::ptr::null(), &raw mut action) };
    if result != 0 {
        return Err(ChatServiceError::Io(std::io::Error::last_os_error()));
    }
    let handler = action.sa_sigaction;
    let flags = action.sa_flags;
    if handler != 0 && handler != libc::SIG_DFL {
        return Err(fail(
            "chat launch requires SIGCHLD=SIG_DFL with SA_NOCLDWAIT clear",
        ));
    }
    if flags & libc::SA_NOCLDWAIT != 0 {
        return Err(fail(
            "chat launch requires SIGCHLD=SIG_DFL with SA_NOCLDWAIT clear",
        ));
    }
    Ok(())
}

fn on_path(executable: &str) -> Option<PathBuf> {
    if executable.contains('/') {
        let candidate = PathBuf::from(executable);
        return is_executable(&candidate).then_some(candidate);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(executable))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(candidate: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(candidate) else {
        return false;
    };
    metadata.is_file() && metadata.mode() & 0o111 != 0
}

// Keys of the retired Python polling configuration that the Rust
// bridge configuration does not have (`allowed_senders` exists in
// both and so cannot mark the old format on its own).
const PYTHON_CONFIG_KEYS: [&str; 7] = [
    "space",
    "target",
    "transport_command",
    "transport_socket",
    "token_env",
    "token_command",
    "event_command",
];

fn refuse_python_format_config(path: &Path, raw: &str) -> Option<String> {
    let document: Value = serde_json::from_str(raw).ok()?;
    let object = document.as_object()?;
    let python_keys: Vec<&str> = PYTHON_CONFIG_KEYS
        .iter()
        .copied()
        .filter(|key| object.contains_key(*key))
        .collect();
    if python_keys.is_empty() {
        return None;
    }
    Some(format!(
        "chat launch: {} is a Python-format chat config (keys: {}); the Python Google Chat polling bridge is retired and Rust launch does not convert its config or state. remedy: write a Rust bridge configuration and create fresh state with 'agentctl chat init', then run 'agentctl chat launch' from the coordinator's shell pane",
        path.display(),
        python_keys.join(", ")
    ))
}

fn refuse_python_format_state(state: &Path) -> Result<()> {
    let bridge_json = state.join("bridge.json");
    let Ok(raw) = std::fs::read_to_string(&bridge_json) else {
        return Ok(());
    };
    let Ok(document) = serde_json::from_str::<Value>(&raw) else {
        return Ok(());
    };
    let config = document.get("config");
    let is_python = config
        .map(|value| {
            PYTHON_CONFIG_KEYS
                .iter()
                .any(|key| value.get(*key).is_some())
        })
        .unwrap_or(false);
    if is_python {
        return Err(fail(format!(
            "chat launch: {} holds Python polling bridge state; Rust launch does not read or convert it. remedy: choose a fresh state directory, create Rust state with 'agentctl chat init', then run 'agentctl chat launch' from the coordinator's shell pane",
            state.display()
        )));
    }
    Ok(())
}

/// Options for one `chat launch` invocation.
pub struct LaunchOptions {
    /// Bridge state directory to create or reopen.
    pub state: PathBuf,
    /// Private Rust bridge configuration file.
    pub config: PathBuf,
    /// Native harness kind (`codex` or `claude`).
    pub harness: String,
    /// Optional native model name.
    pub model: Option<String>,
    /// Optional native session to resume.
    pub resume: Option<String>,
    /// Literal extra harness arguments.
    pub harness_args: Vec<String>,
    /// Optional reply label override.
    pub agent_label: Option<String>,
    /// Optional bridge subscription ready timeout (seconds, 1..=3600).
    pub ready_timeout: Option<f64>,
    /// Optional bridge reconcile interval (seconds, 1..=86400).
    pub reconcile_interval: Option<f64>,
}

/// Launch one coordinator and its bridge from the invoking shell pane.
pub fn launch(registry: &Path, herdr_bin: &Path, options: &LaunchOptions) -> Result<i32> {
    require_waitable_children()?;
    let pane_id = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let workspace_id = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
    if std::env::var("HERDR_ENV").as_deref() != Ok("1")
        || pane_id.is_empty()
        || workspace_id.is_empty()
    {
        return Err(fail(
            "chat launch must run inside the Herdr shell pane that will host the coordinator",
        ));
    }
    // Refuse retired Python-format authority before touching Herdr.
    let raw_config = std::fs::read_to_string(&options.config)
        .map_err(|error| fail(format!("cannot read {}: {error}", options.config.display())))?;
    if let Some(refusal) = refuse_python_format_config(&options.config, &raw_config) {
        return Err(fail(refusal));
    }
    let state = if options.state.is_absolute() {
        options.state.clone()
    } else {
        std::env::current_dir()
            .map_err(ChatServiceError::Io)?
            .join(&options.state)
    };
    refuse_python_format_state(&state)?;
    let mut configuration = BridgeConfiguration::load_private(&options.config)?;
    if let Some(label) = &options.agent_label {
        configuration.agent_label = label.clone();
    }
    let client = HerdrClient::with_executable("direct", herdr_bin)
        .map_err(|error| fail(error.to_string()))?;
    let info = client
        .pane_info(&pane_id)
        .map_err(|error| fail(error.to_string()))?;
    if info.workspace_id != workspace_id {
        return Err(fail(
            "current Herdr pane and workspace environment do not match",
        ));
    }
    if info.agent.is_some() {
        return Err(fail(
            "current Herdr pane already hosts an agent; run chat launch from a shell pane",
        ));
    }
    let workspace_label = client
        .workspace_label(&workspace_id)
        .map_err(|error| fail(error.to_string()))?;
    if on_path(&options.harness).is_none() {
        return Err(fail(format!("cannot find '{}' on PATH", options.harness)));
    }
    let harness_tail = crate::subagents::harness_arguments(
        &options.harness,
        options.model.as_deref(),
        options.resume.as_deref(),
        &options.harness_args,
    )
    .map_err(|error| fail(error.to_string()))?;
    let mut coordinator_command = vec![options.harness.clone()];
    coordinator_command.extend(harness_tail);

    let terminate_flag = TERMINATE_FLAG
        .get_or_init(|| std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)))
        .clone();
    signal_hook::flag::register(libc::SIGTERM, terminate_flag.clone())
        .map_err(ChatServiceError::Io)?;

    // Refuse doomed launches before any harness is spawned.
    crate::chat_service::validate_configuration(&configuration)?;
    let manager = ManagedAgents::new(&client, registry).map_err(|error| fail(error.to_string()))?;
    let agent_name = configuration.agent_name.clone();
    let registered = if registry.exists() {
        agent_queue::registry::names(registry).map_err(|error| fail(error.to_string()))?
    } else {
        Vec::new()
    };
    if registered.iter().any(|name| name == &agent_name) {
        return Err(fail(format!(
            "agent '{agent_name}' is already registered; stop or unregister it before launching again"
        )));
    }

    let mut bridge: Option<LaunchChild> = None;
    let mut coordinator: Option<LaunchChild> = None;
    let mut log_thread: Option<std::thread::JoinHandle<()>> = None;
    let log_failures = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let mut failure: Option<ChatServiceError> = None;
    let mut result: Option<i32> = None;
    let mut terminated = false;

    let run = (|| -> Result<()> {
        // Spawn the coordinator first: the bridge needs its registry
        // record, which adoption creates from the live pane.
        let coord = spawn_launch_child(
            "coordinator",
            &coordinator_command,
            false,
            Stdio::inherit(),
            Stdio::inherit(),
            Stdio::inherit(),
        )?;
        let coord_pidfd = coord.pidfd.as_raw_fd();
        coordinator = Some(coord);
        if terminated_flag() {
            terminated = true;
            return Ok(());
        }
        activate(coordinator.as_mut().expect("coordinator spawned"))?;
        // Wait for Herdr to see the harness, then bind it in the registry.
        let deadline = Instant::now() + AGENT_READY_TIMEOUT;
        loop {
            if terminated_flag() {
                terminated = true;
                return Ok(());
            }
            if wait_fd(coord_pidfd, Some(Duration::ZERO)) {
                result = Some(
                    coordinator
                        .as_mut()
                        .expect("coordinator spawned")
                        .status()?,
                );
                return Ok(());
            }
            match client.pane_info(&pane_id) {
                Ok(current) if current.agent.as_deref() == Some(options.harness.as_str()) => {
                    break;
                }
                Ok(_) => {}
                Err(error) => return Err(fail(error.to_string())),
            }
            if Instant::now() >= deadline {
                return Err(fail(format!(
                    "coordinator harness '{}' did not appear in pane {pane_id} within {} seconds",
                    options.harness,
                    AGENT_READY_TIMEOUT.as_secs()
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let current = client
            .pane_info(&pane_id)
            .map_err(|error| fail(error.to_string()))?;
        manager
            .adopt(
                &agent_name,
                crate::subagents::AdoptOptions {
                    pane_id: pane_id.clone(),
                    expected_workspace: workspace_label.clone(),
                    cwd: PathBuf::from(&current.cwd),
                    harness: options.harness.clone(),
                    session: current.session_value.clone(),
                    record_edges: true,
                },
            )
            .map_err(|error| fail(error.to_string()))?;
        // Create (or reopen) the bridge state now that authority exists.
        if state.join("bridge.json").exists() {
            let _status = crate::chat_service::status(&state)?;
        } else {
            let _status = crate::chat_service::initialize_configuration(
                &state,
                configuration.clone(),
                &manager,
            )?;
        }
        // The bridge child is this same executable running `chat run`.
        let executable = std::env::current_exe().map_err(ChatServiceError::Io)?;
        let mut bridge_command = vec![
            executable.to_string_lossy().into_owned(),
            "--registry".to_string(),
            registry.to_string_lossy().into_owned(),
            "--herdr-bin".to_string(),
            herdr_bin.to_string_lossy().into_owned(),
            "chat".to_string(),
            "run".to_string(),
            "--bridge-state".to_string(),
            state.to_string_lossy().into_owned(),
        ];
        if let Some(timeout) = options.ready_timeout {
            bridge_command.push("--ready-timeout".to_string());
            bridge_command.push(timeout.to_string());
        }
        if let Some(interval) = options.reconcile_interval {
            bridge_command.push("--reconcile-interval".to_string());
            bridge_command.push(interval.to_string());
        }
        let (log_read, log_write) = pipe_pair()?;
        let log_write = File::from(log_write);
        let log_write_dup = log_write.try_clone().map_err(ChatServiceError::Io)?;
        let bridge_child = spawn_launch_child(
            "bridge",
            &bridge_command,
            true,
            Stdio::null(),
            Stdio::from(log_write),
            Stdio::from(log_write_dup),
        )?;
        let log_path = state.join("bridge.log");
        let failures = log_failures.clone();
        let log_source = File::from(log_read);
        log_thread = Some(std::thread::spawn(move || {
            drain_launch_log(log_source, log_path, failures);
        }));
        bridge = Some(bridge_child);
        if terminated_flag() {
            terminated = true;
            return Ok(());
        }
        activate(bridge.as_mut().expect("bridge spawned"))?;
        eprintln!(
            "agentctl chat: bridge state {} (log {}); starting {}",
            state.display(),
            state.join("bridge.log").display(),
            coordinator_command.join(" ")
        );
        Ok(())
    })();
    if let Err(error) = run {
        failure = Some(error);
    }

    if failure.is_none() && result.is_none() && !terminated {
        // Supervise both children until one finishes or SIGTERM arrives.
        let bridge_fd = bridge.as_ref().map(|child| child.pidfd.as_raw_fd());
        let coord_fd = coordinator.as_ref().map(|child| child.pidfd.as_raw_fd());
        loop {
            if terminated_flag() {
                terminated = true;
                break;
            }
            let mut descriptors: Vec<libc::pollfd> = Vec::new();
            if let Some(fd) = bridge_fd {
                descriptors.push(libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            if let Some(fd) = coord_fd {
                descriptors.push(libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                });
            }
            // SAFETY: initialized pollfds retained for the call.
            let polled = unsafe {
                libc::poll(
                    descriptors.as_mut_ptr(),
                    descriptors.len() as libc::nfds_t,
                    100,
                )
            };
            if polled < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                failure = Some(ChatServiceError::Io(std::io::Error::last_os_error()));
                break;
            }
            if polled == 0 {
                continue;
            }
            let bridge_ready = descriptors
                .first()
                .map(|item| item.revents != 0)
                .unwrap_or(false);
            let coord_ready = descriptors
                .get(1)
                .map(|item| item.revents != 0)
                .unwrap_or(false);
            if coord_ready {
                match coordinator.as_mut().expect("coordinator").status() {
                    Ok(code) => result = Some(code),
                    Err(error) => failure = Some(error),
                }
                break;
            }
            if bridge_ready {
                // Preserve the coordinator-first coalescing envelope.
                std::thread::sleep(EXIT_COALESCE);
                if terminated_flag() {
                    terminated = true;
                    break;
                }
                if let Some(fd) = coord_fd {
                    if wait_fd(fd, Some(Duration::ZERO)) {
                        match coordinator.as_mut().expect("coordinator").status() {
                            Ok(code) => result = Some(code),
                            Err(error) => failure = Some(error),
                        }
                        break;
                    }
                }
                let bridge_code = match bridge.as_mut().expect("bridge").status() {
                    Ok(code) => code,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                };
                let detail = launch_log_detail(&state.join("bridge.log"), 2000);
                let detail = if detail.is_empty() {
                    log_failures
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner())
                        .last()
                        .map(|item| format!("launch log unavailable: {item}"))
                        .unwrap_or_default()
                } else {
                    detail
                };
                failure = Some(fail(format!(
                    "chat bridge exited {bridge_code} while the coordinator was running{}",
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(": {detail}")
                    }
                )));
                break;
            }
        }
    }

    // Cleanup: stop whichever child is still running.
    let mut cleanup_fault: Option<ChatServiceError> = None;
    if let Some(coord) = coordinator.as_mut() {
        if !coord.reaped {
            if let Err(error) = coord.stop(false) {
                cleanup_fault = Some(error);
            }
        }
        coord.close();
    }
    if let Some(bridge_child) = bridge.as_mut() {
        if !bridge_child.reaped {
            if let Err(error) = bridge_child.stop(true) {
                if cleanup_fault.is_none() {
                    cleanup_fault = Some(error);
                }
            }
        }
        bridge_child.close();
    }
    if let Some(thread) = log_thread.take() {
        // Bounded like the reference's 5s daemon join: a bridge descendant
        // that escaped its group while holding the log pipe must not hang
        // launch forever after both supervised children are dead.
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = thread.join();
            let _ = sender.send(());
        });
        let _ = receiver.recv_timeout(Duration::from_secs(5));
    }
    if let Some(error) = cleanup_fault {
        return Err(error);
    }
    if terminated {
        return Ok(0);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    match result {
        Some(code) => Ok(code),
        None => Err(fail(
            "chat launch completed without an authoritative coordinator status",
        )),
    }
}
