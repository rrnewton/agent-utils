//! Direct typed access to Herdr's public CLI for interactive agent control.
//!
//! Herdr owns protocol negotiation and terminal processes. This adapter starts no
//! server and has no shell executor, broker, or allowlist dependency.
use crate::error::{AdapterError, Result};
use chat_subscription_plugin::process::ProcessPluginChild;
use serde_json::{Map, Value};
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
#[cfg(target_os = "linux")]
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_STDOUT_BYTES: usize = 8 * 1024 * 1024;
const CONTROL_STDERR_BYTES: usize = 64 * 1024;
const CAPTURE_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CAPTURE_READ_BURST: usize = 256 * 1024;
#[derive(Debug)]
pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Capture a subprocess while enforcing wall-clock and byte bounds on multiplexed output pipes.
#[cfg(test)]
pub(crate) fn bounded_output(command: Command, timeout: Duration) -> io::Result<BoundedOutput> {
    bounded_output_with_cancellation(command, timeout, &|| false)
}

fn bounded_output_with_cancellation(
    command: Command,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
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
            let _ = captured.child.shutdown(Duration::ZERO);
            return Err(if was_cancelled {
                io::Error::new(io::ErrorKind::Interrupted, "control command was cancelled")
            } else {
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("control command timed out after {}s", timeout.as_secs()),
                )
            });
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
            let status = captured.child.shutdown(Duration::ZERO)?;
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
    fn spawn(command: Command) -> io::Result<Self> {
        let _ = ProcessPluginChild::spawn(command)?;
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
        let mut arguments = strings(&["workspace", "create", "--label", label, "--cwd", cwd]);
        for entry in environment {
            arguments.extend(["--env".to_owned(), entry.clone()]);
        }
        arguments.push("--no-focus".to_owned());
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
        let mut arguments = strings(&[
            "tab",
            "create",
            "--workspace",
            workspace_id,
            "--label",
            label,
            "--cwd",
            cwd,
        ]);
        for entry in environment {
            arguments.extend(["--env".to_owned(), entry.clone()]);
        }
        arguments.push("--no-focus".to_owned());
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
        let pane = required_object(&result, "pane", "pane get")?;
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
        for entry in environment {
            arguments.extend(["--env".to_owned(), entry.clone()]);
        }
        arguments.push("--no-focus".to_owned());
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::process::Child;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    static EXECUTABLE_FIXTURE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct FakeExecutable {
        root: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }
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
            let script = format!("#!/usr/bin/python3\nimport json, pathlib, sys\npathlib.Path(__file__).with_name('args').write_text(json.dumps(sys.argv[1:]))\nprint({response:?})\n");
            fs::write(root.join("herdr"), script).unwrap();
            fs::set_permissions(root.join("herdr"), fs::Permissions::from_mode(0o700)).unwrap();
            Self {
                root,
                _guard: guard,
            }
        }
        fn client(&self) -> HerdrClient {
            HerdrClient::with_executable("direct", &self.root.join("herdr")).unwrap()
        }
        fn arguments(&self) -> Value {
            serde_json::from_slice(&fs::read(self.root.join("args")).unwrap()).unwrap()
        }
    }
    impl Drop for FakeExecutable {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct RecordedChild {
        root: PathBuf,
        pid_file: PathBuf,
    }

    impl RecordedChild {
        fn new(kind: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-{kind}-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            let pid_file = root.join("pid");
            Self { root, pid_file }
        }

        fn escaped_pipe_command(&self) -> Command {
            let mut command = Command::new("/bin/sh");
            command
                .args([
                    "-c",
                    "/usr/bin/setsid /bin/sleep 30 & printf '%s\\n' \"$!\" > \"$1\"; exit 0",
                    "agentctl-test",
                ])
                .arg(&self.pid_file);
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
                .arg(&self.pid_file)
                .arg((CONTROL_STDOUT_BYTES + 1).to_string());
            command
        }

        fn pid(&self) -> libc::pid_t {
            fs::read_to_string(&self.pid_file)
                .expect("setsid holder wrote its pid")
                .trim()
                .parse()
                .expect("setsid holder pid is numeric")
        }
    }

    impl Drop for RecordedChild {
        fn drop(&mut self) {
            if let Ok(value) = fs::read_to_string(&self.pid_file) {
                if let Ok(pid) = value.trim().parse::<libc::pid_t>() {
                    // SAFETY: this fixture recorded the exact process it created and is solely
                    // responsible for bounding that deliberately escaped test descendant.
                    let _ = unsafe { libc::kill(pid, libc::SIGKILL) };
                }
            }
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    struct ChildGuard(Child);

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

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

    #[test]
    fn control_timeout_includes_pipes_inherited_after_the_parent_exits() {
        let escaped = RecordedChild::new("setsid-holder");
        let mut unrelated = ChildGuard(Command::new("/bin/sleep").arg("30").spawn().unwrap());
        let started = Instant::now();
        let result = bounded_output(escaped.escaped_pipe_command(), Duration::from_millis(100));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        // The setsid child escaped the supervised group and really was retaining both capture
        // pipes, so returning promptly did not depend on receiving EOF from reader threads.
        assert_eq!(unsafe { libc::kill(escaped.pid(), 0) }, 0);
        // The pinned private group must not be confused with any unrelated numeric identity.
        assert!(unrelated.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn control_cancellation_kills_command_group_and_inherited_pipes_promptly() {
        let escaped = RecordedChild::new("setsid-holder");
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = Arc::clone(&cancelled);
        let pid_file = escaped.pid_file.clone();
        let worker = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            loop {
                let marker_is_complete = fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|value| value.trim().parse::<libc::pid_t>().ok())
                    .is_some();
                if marker_is_complete || Instant::now() >= deadline {
                    trigger.store(true, Ordering::SeqCst);
                    return marker_is_complete;
                }
                thread::sleep(Duration::from_millis(5));
            }
        });
        let started = Instant::now();
        let result = bounded_output_with_cancellation(
            escaped.escaped_pipe_command(),
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
        assert_eq!(unsafe { libc::kill(escaped.pid(), 0) }, 0);
    }

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
            wait_until_process_is_gone(group_child.pid(), Duration::from_secs(2)),
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
