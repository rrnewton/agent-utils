//! Typed access to Herdr's command-line socket API.
//!
//! Herdr itself owns protocol negotiation, so this module invokes its JSON-producing CLI rather
//! than duplicating the socket wire format.  Production construction deliberately ignores the
//! caller's `PATH` and `HOME`: the executable is selected from fixed install locations rooted in
//! the current account's database home, and that same home is the only environment value passed
//! to `systemd-run`.
//!
//! This is a safety rail, not a same-UID security boundary.  A normal user installation is
//! necessarily writable by that user, and a same-UID process can usually reach Herdr's socket (and
//! often the user systemd bus) directly.  Enforceable isolation therefore also requires a broker
//! and sandbox policy that deny those raw control channels.

use std::collections::BTreeSet;
use std::ffi::{CStr, OsString};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::error::{HerdrRunError, Result};

/// Transient user-systemd unit used for a server started by `herdr-run`.
pub const SERVER_UNIT: &str = "herdr-run-server";

const SERVER_ATTEMPTS: usize = 30;
const SERVER_DELAY: Duration = Duration::from_millis(200);
pub(crate) const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);

// Linux `pid_t` is signed.  Parsing into i64 first lets us issue one clear protocol error for
// zero, negative, and otherwise representable values beyond the shared process-ID range.
const MAX_PROCESS_ID: i64 = i32::MAX as i64;

pub(crate) struct BoundedOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Capture a subprocess while enforcing a wall-clock bound and draining both pipes concurrently.
pub(crate) fn bounded_output(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<BoundedOutput> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn()?;
    let child_pid = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("child stdout pipe was not captured"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("child stderr pipe was not captured"))?;
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now().checked_add(timeout);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if deadline.is_some_and(|value| Instant::now() >= value) {
            // The child owns a fresh process group. Killing the group also closes pipes inherited
            // by descendants, so reader threads cannot keep this timeout path blocked forever.
            let _ = unsafe { libc::kill(-(child_pid as i32), libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("control command timed out after {}s", timeout.as_secs()),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| io::Error::other("stdout reader thread panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| io::Error::other("stderr reader thread panicked"))??;
    Ok(BoundedOutput {
        status,
        stdout,
        stderr,
    })
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

/// A pane's foreground-process state as reported by Herdr.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessInfo {
    /// Herdr pane identifier to which the reading belongs.
    pub pane_id: String,
    /// PID of the pane's interactive shell.
    pub shell_pid: i64,
    /// Foreground process-group ID associated with the terminal.
    pub foreground_pgid: i64,
    /// `(pid, name, command line)` for each foreground process.
    pub foreground: Vec<(i64, String, String)>,
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

/// Injectable external-command transport used by [`HerdrClient`].
pub trait CommandRunner: Send + Sync {
    /// Run exactly `argv`, capturing its status and both output streams.
    fn run(&self, argv: &[String]) -> io::Result<CommandOutput>;
}

/// Real [`CommandRunner`] backed by [`std::process::Command`].
///
/// It removes caller-controlled `PATH` and replaces `HOME` with the account-database value. All
/// programs it receives from [`HerdrClient`] are absolute, so executable search is unnecessary.
#[derive(Clone, Debug)]
pub struct SystemCommandRunner {
    account_home: OsString,
    timeout: Duration,
}

impl SystemCommandRunner {
    /// Construct a sanitized process runner for `account_home`.
    #[must_use]
    pub fn new(account_home: &Path) -> Self {
        Self {
            account_home: account_home.as_os_str().to_owned(),
            timeout: CONTROL_TIMEOUT,
        }
    }

    fn command(&self, argv: &[String]) -> io::Result<Command> {
        let (program, arguments) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty command argv"))?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .env_remove("PATH")
            .env("HOME", &self.account_home);
        Ok(command)
    }
}

impl CommandRunner for SystemCommandRunner {
    fn run(&self, argv: &[String]) -> io::Result<CommandOutput> {
        let output = bounded_output(&mut self.command(argv)?, self.timeout)?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Injectable delay mechanism used while polling server startup.
pub trait Sleeper: Send + Sync {
    /// Pause for `duration`.
    fn sleep(&self, duration: Duration);
}

/// Real [`Sleeper`] backed by [`std::thread::sleep`].
#[derive(Clone, Copy, Debug, Default)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Operations needed by session resolution, readiness checks, and command execution.
///
/// The trait keeps those layers independently testable without spawning Herdr.  The production
/// implementation is [`HerdrClient`].
pub trait HerdrApi {
    /// Ensure a compatible server is running; return whether this call started it.
    fn ensure_server(&self) -> Result<bool>;
    /// Resolve a workspace label, refusing ambiguous duplicate labels.
    fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>>;
    /// Return the live label for a workspace ID, if that ID exists.
    fn workspace_label_for_id(&self, workspace_id: &str) -> Result<Option<String>>;
    /// Create a workspace and return `(workspace ID, root tab ID, root pane ID)`.
    fn create_workspace(&self, label: &str, cwd: &str) -> Result<(String, String, String)>;
    /// Resolve a tab label inside a workspace, refusing ambiguous duplicate labels.
    fn tab_id_for_label(&self, workspace_id: &str, label: &str) -> Result<Option<String>>;
    /// Create a tab without changing the user's focused tab.
    fn create_tab(&self, workspace_id: &str, label: &str, cwd: &str) -> Result<String>;
    /// Rename a tab.
    fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()>;
    /// List panes, optionally restricted to a workspace.
    fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>>;
    /// Report whether a pane ID is currently live.
    fn pane_exists(&self, pane_id: &str) -> bool;
    /// Read live foreground-process information for a pane.
    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo>;
    /// Read rendered pane text.
    fn read(&self, pane_id: &str, source: &str, lines: Option<usize>) -> Result<String>;
    /// Type and submit a command in a pane.
    fn run(&self, pane_id: &str, command: &str) -> Result<()>;
    /// Send named key presses to a pane.
    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Broker {
    Direct,
    SystemdRun,
}

impl Broker {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "direct" => Ok(Self::Direct),
            "systemd-run" => Ok(Self::SystemdRun),
            other => Err(HerdrRunError::config(format!(
                "broker must be 'direct' or 'systemd-run', got {other:?}"
            ))),
        }
    }
}

/// Production command-level client for a Herdr session.
pub struct HerdrClient {
    herdr_bin: String,
    systemd_run_bin: String,
    account_home: String,
    broker: Broker,
    runner: Arc<dyn CommandRunner>,
    sleeper: Arc<dyn Sleeper>,
}

impl fmt::Debug for HerdrClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HerdrClient")
            .field("herdr_bin", &self.herdr_bin)
            .field("systemd_run_bin", &self.systemd_run_bin)
            .field("account_home", &self.account_home)
            .field("broker", &self.broker)
            .finish_non_exhaustive()
    }
}

impl HerdrClient {
    /// Construct a production client without consulting caller-controlled `HOME` or `PATH`.
    pub fn new(broker: &str) -> Result<Self> {
        let account_home = current_account_home()?;
        let herdr_bin = resolve_herdr_executable(&account_home)?;
        let systemd_run_bin = resolve_fixed_executable(
            "systemd-run",
            &[
                PathBuf::from("/usr/bin/systemd-run"),
                PathBuf::from("/bin/systemd-run"),
            ],
        )?;
        let runner = Arc::new(SystemCommandRunner::new(&account_home));
        Self::from_parts(
            broker,
            herdr_bin,
            systemd_run_bin,
            account_home,
            runner,
            Arc::new(ThreadSleeper),
        )
    }

    fn from_parts(
        broker: &str,
        herdr_bin: PathBuf,
        systemd_run_bin: PathBuf,
        account_home: PathBuf,
        runner: Arc<dyn CommandRunner>,
        sleeper: Arc<dyn Sleeper>,
    ) -> Result<Self> {
        let herdr_bin = utf8_absolute(&herdr_bin, "Herdr executable")?;
        let systemd_run_bin = utf8_absolute(&systemd_run_bin, "systemd-run executable")?;
        let account_home = utf8_absolute(&account_home, "account home")?;
        Ok(Self {
            herdr_bin,
            systemd_run_bin,
            account_home,
            broker: Broker::parse(broker)?,
            runner,
            sleeper,
        })
    }

    /// Ensure a compatible server is available, using production polling limits.
    pub fn ensure_server(&self) -> Result<bool> {
        self.ensure_server_with(SERVER_ATTEMPTS, SERVER_DELAY)
    }

    /// Ensure a server is available with explicit polling controls.
    ///
    /// This is primarily useful to deterministic test harnesses and embedders with their own
    /// latency budget.  The first readiness check happens before any server launch.
    pub fn ensure_server_with(&self, attempts: usize, delay: Duration) -> Result<bool> {
        if self.server_running() {
            return Ok(false);
        }

        let launch = vec![
            self.systemd_run_bin.clone(),
            "--user".to_owned(),
            "--collect".to_owned(),
            "--unit".to_owned(),
            SERVER_UNIT.to_owned(),
            "--description".to_owned(),
            "herdr-run Herdr server (outside the agent sandbox)".to_owned(),
            "--setenv".to_owned(),
            format!("HOME={}", self.account_home),
            self.herdr_bin.clone(),
            "server".to_owned(),
        ];
        let completed = self.runner.run(&launch).map_err(|error| {
            HerdrRunError::unavailable(format!("cannot start the Herdr server: {error}"))
        })?;
        let combined = format!("{}{}", completed.stdout, completed.stderr);
        if completed.status != 0 && !combined.contains("already exists") {
            return Err(HerdrRunError::unavailable(format!(
                "cannot start the Herdr server: {}",
                detail(&completed)
            )));
        }

        for _ in 0..attempts {
            self.sleeper.sleep(delay);
            if self.server_running() {
                return Ok(true);
            }
        }
        Err(HerdrRunError::unavailable(format!(
            "the Herdr server did not become ready after {attempts} attempts; check 'systemctl --user status {SERVER_UNIT}'"
        )))
    }

    /// Resolve a workspace label, refusing ambiguous duplicate labels.
    pub fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>> {
        let workspaces = self.workspace_entries()?;
        unique_label_id(&workspaces, "workspace_id", label, "workspace")
    }

    /// Return the current label for `workspace_id`, or `None` if the ID is absent.
    pub fn workspace_label_for_id(&self, workspace_id: &str) -> Result<Option<String>> {
        let workspaces = self.workspace_entries()?;
        let mut labels = BTreeSet::new();
        for workspace in workspaces {
            if optional_string(&workspace, "workspace_id", "workspace list entry")?.as_deref()
                == Some(workspace_id)
            {
                labels.insert(required_string(
                    &workspace,
                    "label",
                    "workspace list entry",
                )?);
            }
        }
        if labels.len() > 1 {
            return Err(HerdrRunError::unavailable(format!(
                "workspace id {workspace_id:?} appears with conflicting labels"
            )));
        }
        Ok(labels.into_iter().next())
    }

    /// Create a labelled workspace without changing focus.
    pub fn create_workspace(&self, label: &str, cwd: &str) -> Result<(String, String, String)> {
        let result = self.call(
            &strings(&[
                "workspace",
                "create",
                "--label",
                label,
                "--cwd",
                cwd,
                "--no-focus",
            ]),
            &format!("workspace create {label:?}"),
        )?;
        let workspace = required_object(&result, "workspace", "workspace create")?;
        let tab = required_object(&result, "tab", "workspace create")?;
        let pane = required_object(&result, "root_pane", "workspace create")?;
        Ok((
            required_string(workspace, "workspace_id", "workspace create")?,
            required_string(tab, "tab_id", "workspace create")?,
            required_string(pane, "pane_id", "workspace create")?,
        ))
    }

    /// Resolve a tab label inside `workspace_id`, refusing ambiguous duplicates.
    pub fn tab_id_for_label(&self, workspace_id: &str, label: &str) -> Result<Option<String>> {
        let result = self.call(
            &strings(&["tab", "list", "--workspace", workspace_id]),
            "tab list",
        )?;
        let tabs = value_array(result.get("tabs"), "tab list.tabs")?;
        let entries = object_entries(tabs, "tab list entry")?;
        unique_label_id(&entries, "tab_id", label, "tab")
    }

    /// Create a labelled tab without changing focus and return its ID.
    pub fn create_tab(&self, workspace_id: &str, label: &str, cwd: &str) -> Result<String> {
        let result = self.call(
            &strings(&[
                "tab",
                "create",
                "--workspace",
                workspace_id,
                "--label",
                label,
                "--cwd",
                cwd,
                "--no-focus",
            ]),
            &format!("tab create {label:?}"),
        )?;
        let tab = match result.get("tab") {
            Some(value) => value
                .as_object()
                .ok_or_else(|| HerdrRunError::unavailable("tab create: 'tab' is not an object"))?,
            None => &result,
        };
        required_string(tab, "tab_id", "tab create")
    }

    /// Rename `tab_id` to `label`.
    pub fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
        self.call(
            &strings(&["tab", "rename", tab_id, label]),
            &format!("tab rename {tab_id}"),
        )?;
        Ok(())
    }

    /// List all panes, optionally restricted to `workspace_id`.
    pub fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>> {
        let mut args = strings(&["pane", "list"]);
        if let Some(workspace_id) = workspace_id {
            args.extend(strings(&["--workspace", workspace_id]));
        }
        let result = self.call(&args, "pane list")?;
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
                return Err(HerdrRunError::unavailable(format!(
                    "pane list: returned pane {:?} from workspace {:?}, expected {expected:?}",
                    pane.pane_id, pane.workspace_id
                )));
            }
        }
        Ok(panes)
    }

    /// Report whether a pane ID is currently live.
    #[must_use]
    pub fn pane_exists(&self, pane_id: &str) -> bool {
        self.call(
            &strings(&["pane", "get", pane_id]),
            &format!("pane get {pane_id}"),
        )
        .is_ok()
    }

    /// Read foreground-process information for `pane_id`.
    pub fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
        let result = self.call(
            &strings(&["pane", "process-info", "--pane", pane_id]),
            &format!("pane process-info {pane_id}"),
        )?;
        let info = required_object(&result, "process_info", "pane process-info")?;
        let processes = value_array(info.get("foreground_processes"), "foreground_processes")?;
        let foreground = object_entries(processes, "foreground process")?
            .iter()
            .map(|process| {
                Ok((
                    required_process_id(process, "pid", "foreground process")?,
                    optional_string(process, "name", "foreground process")?.unwrap_or_default(),
                    optional_string(process, "cmdline", "foreground process")?.unwrap_or_default(),
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let parsed = ProcessInfo {
            pane_id: required_string(info, "pane_id", "pane process-info")?,
            shell_pid: required_process_id(info, "shell_pid", "pane process-info")?,
            foreground_pgid: required_process_id(
                info,
                "foreground_process_group_id",
                "pane process-info",
            )?,
            foreground,
        };
        if parsed.pane_id != pane_id {
            return Err(HerdrRunError::unavailable(format!(
                "pane process-info: returned pane {:?}, expected {pane_id:?}",
                parsed.pane_id
            )));
        }
        Ok(parsed)
    }

    /// Read rendered text from `pane_id`.
    pub fn read(&self, pane_id: &str, source: &str, lines: Option<usize>) -> Result<String> {
        let mut args = strings(&["pane", "read", pane_id, "--source", source]);
        if let Some(lines) = lines {
            args.extend(["--lines".to_owned(), lines.to_string()]);
        }
        let completed = self.invoke(&args)?;
        if completed.status != 0 {
            return Err(HerdrRunError::unavailable(format!(
                "pane read {pane_id}: {}",
                stderr_detail(&completed)
            )));
        }
        Ok(completed.stdout)
    }

    /// Type and submit `command` in `pane_id`.
    pub fn run(&self, pane_id: &str, command: &str) -> Result<()> {
        self.call_ok(
            &strings(&["pane", "run", pane_id, command]),
            &format!("pane run {pane_id}"),
        )
    }

    /// Send named `keys` to `pane_id`.
    pub fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
        self.call_ok(
            &strings(&["pane", "send-keys", pane_id, keys]),
            &format!("pane send-keys {pane_id}"),
        )
    }

    fn workspace_entries(&self) -> Result<Vec<Map<String, Value>>> {
        let result = self.call(&strings(&["workspace", "list"]), "workspace list")?;
        let values = value_array(result.get("workspaces"), "workspace list.workspaces")?;
        object_entries(values, "workspace list entry")
    }

    fn server_running(&self) -> bool {
        let completed = match self.invoke(&strings(&["status", "--json"])) {
            Ok(completed) if completed.status == 0 => completed,
            _ => return false,
        };
        let Ok(document) = serde_json::from_str::<Value>(&completed.stdout) else {
            return false;
        };
        document
            .as_object()
            .and_then(|object| object.get("server"))
            .and_then(Value::as_object)
            .and_then(|server| server.get("running"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    fn invoke(&self, args: &[String]) -> Result<CommandOutput> {
        let mut command = Vec::with_capacity(args.len() + 1);
        command.push(self.herdr_bin.clone());
        command.extend_from_slice(args);
        if self.broker == Broker::SystemdRun {
            let mut wrapped = vec![
                self.systemd_run_bin.clone(),
                "--user".to_owned(),
                "--wait".to_owned(),
                "--pipe".to_owned(),
                "--collect".to_owned(),
                "--quiet".to_owned(),
                "--setenv".to_owned(),
                format!("HOME={}", self.account_home),
            ];
            wrapped.extend(command);
            command = wrapped;
        }
        self.runner
            .run(&command)
            .map_err(|error| HerdrRunError::unavailable(format!("cannot invoke Herdr: {error}")))
    }

    fn call(&self, args: &[String], purpose: &str) -> Result<Map<String, Value>> {
        let completed = self.invoke(args)?;
        if completed.status != 0 {
            return Err(HerdrRunError::unavailable(format!(
                "{purpose}: {}",
                detail(&completed)
            )));
        }
        let document = serde_json::from_str::<Value>(&completed.stdout).map_err(|_| {
            let preview: String = completed.stdout.trim().chars().take(200).collect();
            HerdrRunError::unavailable(format!(
                "{purpose}: herdr returned non-JSON output: {preview:?}"
            ))
        })?;
        let envelope = document.as_object().ok_or_else(|| {
            HerdrRunError::unavailable(format!("{purpose}: Herdr response is not an object"))
        })?;
        envelope
            .get("result")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                HerdrRunError::unavailable(format!(
                    "{purpose}: Herdr response has no result object"
                ))
            })
    }

    fn call_ok(&self, args: &[String], purpose: &str) -> Result<()> {
        let completed = self.invoke(args)?;
        if completed.status == 0 {
            Ok(())
        } else {
            Err(HerdrRunError::unavailable(format!(
                "{purpose}: {}",
                detail(&completed)
            )))
        }
    }
}

impl HerdrApi for HerdrClient {
    fn ensure_server(&self) -> Result<bool> {
        HerdrClient::ensure_server(self)
    }

    fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>> {
        HerdrClient::workspace_id_for_label(self, label)
    }

    fn workspace_label_for_id(&self, workspace_id: &str) -> Result<Option<String>> {
        HerdrClient::workspace_label_for_id(self, workspace_id)
    }

    fn create_workspace(&self, label: &str, cwd: &str) -> Result<(String, String, String)> {
        HerdrClient::create_workspace(self, label, cwd)
    }

    fn tab_id_for_label(&self, workspace_id: &str, label: &str) -> Result<Option<String>> {
        HerdrClient::tab_id_for_label(self, workspace_id, label)
    }

    fn create_tab(&self, workspace_id: &str, label: &str, cwd: &str) -> Result<String> {
        HerdrClient::create_tab(self, workspace_id, label, cwd)
    }

    fn rename_tab(&self, tab_id: &str, label: &str) -> Result<()> {
        HerdrClient::rename_tab(self, tab_id, label)
    }

    fn panes(&self, workspace_id: Option<&str>) -> Result<Vec<Pane>> {
        HerdrClient::panes(self, workspace_id)
    }

    fn pane_exists(&self, pane_id: &str) -> bool {
        HerdrClient::pane_exists(self, pane_id)
    }

    fn process_info(&self, pane_id: &str) -> Result<ProcessInfo> {
        HerdrClient::process_info(self, pane_id)
    }

    fn read(&self, pane_id: &str, source: &str, lines: Option<usize>) -> Result<String> {
        HerdrClient::read(self, pane_id, source, lines)
    }

    fn run(&self, pane_id: &str, command: &str) -> Result<()> {
        HerdrClient::run(self, pane_id, command)
    }

    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
        HerdrClient::send_keys(self, pane_id, keys)
    }
}

/// Return the current account's home directory from the account database.
pub(crate) fn current_account_home() -> Result<PathBuf> {
    // SAFETY: `getuid` has no preconditions. `getpwuid_r` writes only into the supplied `passwd`
    // and byte buffer; pointers are inspected only after a zero return and non-null result, and
    // the C string is copied before either backing allocation is dropped. This small FFI wrapper
    // is necessary because Rust's standard library has no account-database lookup API.
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
                return Err(HerdrRunError::unavailable(
                    "cannot resolve current account home: account record is too large",
                ));
            }
            continue;
        }
        if code != 0 {
            return Err(HerdrRunError::unavailable(format!(
                "cannot resolve current account home: {}",
                io::Error::from_raw_os_error(code)
            )));
        }
        if result.is_null() {
            return Err(HerdrRunError::unavailable(
                "cannot resolve current account home: no account record",
            ));
        }
        let record = unsafe { record.assume_init() };
        if record.pw_dir.is_null() {
            return Err(HerdrRunError::unavailable(
                "the current account has no home directory",
            ));
        }
        let bytes = unsafe { CStr::from_ptr(record.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            return Err(HerdrRunError::unavailable(
                "the current account has no home directory",
            ));
        }
        return Ok(PathBuf::from(OsString::from_vec(bytes.to_vec())));
    }
}

fn resolve_herdr_executable(home: &Path) -> Result<PathBuf> {
    resolve_fixed_executable(
        "Herdr",
        &[
            PathBuf::from("/usr/local/bin/herdr"),
            PathBuf::from("/usr/bin/herdr"),
            home.join(".local/bin/herdr"),
            home.join("bin/herdr"),
            home.join(".cargo/bin/herdr"),
        ],
    )
}

fn resolve_fixed_executable(name: &str, candidates: &[PathBuf]) -> Result<PathBuf> {
    for candidate in candidates {
        let Ok(metadata) = fs::metadata(candidate) else {
            continue;
        };
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }
        let canonical = fs::canonicalize(candidate).map_err(|error| {
            HerdrRunError::unavailable(format!(
                "cannot canonicalize {name} executable {}: {error}",
                candidate.display()
            ))
        })?;
        let metadata = fs::metadata(&canonical).map_err(|error| {
            HerdrRunError::unavailable(format!(
                "cannot inspect {name} executable {}: {error}",
                canonical.display()
            ))
        })?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            continue;
        }
        if metadata.permissions().mode() & 0o022 != 0 {
            return Err(HerdrRunError::unavailable(format!(
                "refusing group/world-writable {name} executable outside the jail: {}",
                canonical.display()
            )));
        }
        return Ok(canonical);
    }
    let searched = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(HerdrRunError::unavailable(format!(
        "{name} executable not found in fixed install locations: {searched}"
    )))
}

fn utf8_absolute(path: &Path, what: &str) -> Result<String> {
    if !path.is_absolute() {
        return Err(HerdrRunError::unavailable(format!(
            "{what} must be absolute: {}",
            path.display()
        )));
    }
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| HerdrRunError::unavailable(format!("{what} is not valid UTF-8")))
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
        None | Some(_) => Err(HerdrRunError::unavailable(format!(
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
                .ok_or_else(|| HerdrRunError::unavailable(format!("{what}: expected an object")))
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
        .ok_or_else(|| HerdrRunError::unavailable(format!("{what}: {key:?} is not an object")))
}

fn required_string(object: &Map<String, Value>, key: &str, what: &str) -> Result<String> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| HerdrRunError::unavailable(format!("{what}: {key:?} is not a string")))
}

fn optional_string(object: &Map<String, Value>, key: &str, what: &str) -> Result<Option<String>> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(HerdrRunError::unavailable(format!(
            "{what}: {key:?} is not a string"
        ))),
    }
}

fn required_integer(object: &Map<String, Value>, key: &str, what: &str) -> Result<i64> {
    object
        .get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| HerdrRunError::unavailable(format!("{what}: {key:?} is not an integer")))
}

fn required_process_id(object: &Map<String, Value>, key: &str, what: &str) -> Result<i64> {
    let value = required_integer(object, key, what)?;
    if !(1..=MAX_PROCESS_ID).contains(&value) {
        return Err(HerdrRunError::unavailable(format!(
            "{what}: {key:?} is outside the positive Linux pid_t range"
        )));
    }
    Ok(value)
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
        return Err(HerdrRunError::unavailable(format!(
            "{kind} label {label:?} is ambiguous: {} matching IDs",
            matches.len()
        )));
    }
    Ok(matches.into_iter().next())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    use super::*;

    static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    #[derive(Default)]
    struct FakeRunner {
        calls: Mutex<Vec<Vec<String>>>,
        outputs: Mutex<VecDeque<io::Result<CommandOutput>>>,
    }

    impl FakeRunner {
        fn with_outputs(outputs: Vec<CommandOutput>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                outputs: Mutex::new(outputs.into_iter().map(Ok).collect()),
            })
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().expect("calls lock").clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, argv: &[String]) -> io::Result<CommandOutput> {
            self.calls.lock().expect("calls lock").push(argv.to_vec());
            self.outputs
                .lock()
                .expect("outputs lock")
                .pop_front()
                .unwrap_or_else(|| panic!("unexpected command: {argv:?}"))
        }
    }

    #[derive(Default)]
    struct NoSleep;

    impl Sleeper for NoSleep {
        fn sleep(&self, _duration: Duration) {}
    }

    fn output(status: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            status,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        }
    }

    fn client(broker: &str, runner: Arc<dyn CommandRunner>) -> HerdrClient {
        HerdrClient::from_parts(
            broker,
            PathBuf::from("/opt/herdr/bin/herdr"),
            PathBuf::from("/usr/bin/systemd-run"),
            PathBuf::from("/home/account"),
            runner,
            Arc::new(NoSleep),
        )
        .expect("client")
    }

    #[test]
    fn real_runner_removes_path_and_pins_account_home() {
        let runner = SystemCommandRunner::new(Path::new("/home/account"));
        let command = runner
            .command(&strings(&["/usr/bin/true"]))
            .expect("command");
        let environment = command.get_envs().collect::<Vec<_>>();
        assert!(environment
            .iter()
            .any(|(key, value)| *key == "PATH" && value.is_none()));
        assert!(environment.iter().any(|(key, value)| {
            *key == "HOME" && value.is_some_and(|value| value == "/home/account")
        }));
    }

    #[test]
    fn real_runner_kills_a_hung_control_command_at_its_bound() {
        let mut runner = SystemCommandRunner::new(Path::new("/home/account"));
        runner.timeout = Duration::from_millis(30);
        let error = runner
            .run(&strings(&["/bin/sh", "-c", "while :; do :; done"]))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn herdr_resolution_uses_fixed_home_candidates_and_rejects_loose_mode() {
        let root = std::env::temp_dir().join(format!(
            "herdr-client-resolution-{}-{}",
            std::process::id(),
            TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let candidate = root.join(".local/bin/herdr");
        fs::create_dir_all(candidate.parent().expect("candidate parent")).unwrap();
        fs::write(&candidate, b"#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&candidate).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&candidate, permissions).unwrap();
        assert_eq!(
            resolve_herdr_executable(&root).unwrap(),
            fs::canonicalize(&candidate).unwrap()
        );

        let mut permissions = fs::metadata(&candidate).unwrap().permissions();
        permissions.set_mode(0o720);
        fs::set_permissions(&candidate, permissions).unwrap();
        let error = resolve_herdr_executable(&root).unwrap_err();
        assert!(error.to_string().contains("group/world-writable"));

        fs::remove_file(&candidate).unwrap();
        let cargo_candidate = root.join(".cargo/bin/herdr");
        fs::create_dir_all(cargo_candidate.parent().expect("cargo candidate parent")).unwrap();
        fs::write(&cargo_candidate, b"#!/bin/sh\n").unwrap();
        let mut permissions = fs::metadata(&cargo_candidate).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&cargo_candidate, permissions).unwrap();
        assert_eq!(
            resolve_herdr_executable(&root).unwrap(),
            fs::canonicalize(&cargo_candidate).unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn systemd_broker_uses_absolute_paths_and_only_account_home() {
        let runner =
            FakeRunner::with_outputs(vec![output(0, r#"{"result":{"workspaces":[]}}"#, "")]);
        let client = client("systemd-run", runner.clone());
        assert_eq!(client.workspace_id_for_label("wanted").unwrap(), None);
        assert_eq!(
            runner.calls()[0],
            strings(&[
                "/usr/bin/systemd-run",
                "--user",
                "--wait",
                "--pipe",
                "--collect",
                "--quiet",
                "--setenv",
                "HOME=/home/account",
                "/opt/herdr/bin/herdr",
                "workspace",
                "list",
            ])
        );
        assert!(!runner.calls()[0].iter().any(|arg| arg.starts_with("PATH=")));
    }

    #[test]
    fn server_launch_is_always_outside_and_tolerates_unit_collision() {
        let runner = FakeRunner::with_outputs(vec![
            output(1, "", "not running"),
            output(1, "", "Unit herdr-run-server already exists"),
            output(0, r#"{"server":{"running":true}}"#, ""),
        ]);
        let client = client("direct", runner.clone());
        assert!(client.ensure_server_with(1, Duration::ZERO).unwrap());
        let calls = runner.calls();
        assert_eq!(
            calls[0],
            strings(&["/opt/herdr/bin/herdr", "status", "--json"])
        );
        assert_eq!(calls[1][0], "/usr/bin/systemd-run");
        assert_eq!(calls[1][8], "HOME=/home/account");
        assert_eq!(calls[1][9], "/opt/herdr/bin/herdr");
        assert!(!calls[1].iter().any(|arg| arg.starts_with("PATH=")));
    }

    #[test]
    fn duplicate_workspace_labels_fail_closed() {
        let runner = FakeRunner::with_outputs(vec![output(
            0,
            r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"x"},{"workspace_id":"w2","label":"x"}]}}"#,
            "",
        )]);
        let error = client("direct", runner)
            .workspace_id_for_label("x")
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }

    #[test]
    fn missing_required_protocol_list_fails_closed() {
        let runner = FakeRunner::with_outputs(vec![output(0, r#"{"result":{}}"#, "")]);
        let error = client("direct", runner)
            .workspace_id_for_label("x")
            .unwrap_err();
        assert!(error.to_string().contains("expected an array"));
    }

    #[test]
    fn process_info_refuses_a_response_for_another_pane() {
        let runner = FakeRunner::with_outputs(vec![output(
            0,
            r#"{"result":{"process_info":{"pane_id":"other","shell_pid":7,"foreground_process_group_id":7,"foreground_processes":[]}}}"#,
            "",
        )]);
        let error = client("direct", runner).process_info("wanted").unwrap_err();
        assert!(error.to_string().contains("expected \"wanted\""));
    }

    #[test]
    fn process_info_refuses_out_of_range_process_ids() {
        for field in ["shell_pid", "foreground_process_group_id", "foreground_pid"] {
            for value in [-1_i64, 0, MAX_PROCESS_ID + 1] {
                let mut info = serde_json::json!({
                    "pane_id": "p",
                    "shell_pid": 7,
                    "foreground_process_group_id": 7,
                    "foreground_processes": [{"pid": 7, "name": "sh", "cmdline": "sh"}],
                });
                if field == "foreground_pid" {
                    info["foreground_processes"][0]["pid"] = Value::from(value);
                } else {
                    info[field] = Value::from(value);
                }
                let response = serde_json::json!({"result": {"process_info": info}}).to_string();
                let runner = FakeRunner::with_outputs(vec![output(0, &response, "")]);
                let error = client("direct", runner).process_info("p").unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("outside the positive Linux pid_t range"),
                    "field={field} value={value}: {error}"
                );
            }
        }
    }

    #[test]
    fn process_info_refuses_boolean_process_ids() {
        for field in ["shell_pid", "foreground_process_group_id", "foreground_pid"] {
            let mut info = serde_json::json!({
                "pane_id": "p",
                "shell_pid": 7,
                "foreground_process_group_id": 7,
                "foreground_processes": [{"pid": 7, "name": "sh", "cmdline": "sh"}],
            });
            if field == "foreground_pid" {
                info["foreground_processes"][0]["pid"] = Value::Bool(true);
            } else {
                info[field] = Value::Bool(true);
            }
            let response = serde_json::json!({"result": {"process_info": info}}).to_string();
            let runner = FakeRunner::with_outputs(vec![output(0, &response, "")]);
            let error = client("direct", runner).process_info("p").unwrap_err();
            assert!(
                error.to_string().contains("not an integer"),
                "field={field}: {error}"
            );
        }
    }

    #[test]
    fn embedded_nul_identifier_is_typed_unavailable() {
        struct NulRejectingRunner;

        impl CommandRunner for NulRejectingRunner {
            fn run(&self, argv: &[String]) -> io::Result<CommandOutput> {
                assert!(argv.iter().any(|argument| argument.contains('\0')));
                Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "nul byte found in provided data",
                ))
            }
        }

        let error = client("direct", Arc::new(NulRejectingRunner))
            .read("bad\0pane", "recent-unwrapped", None)
            .unwrap_err();
        assert!(error.to_string().contains("cannot invoke Herdr"));
    }

    #[test]
    fn filtered_pane_list_refuses_another_workspace() {
        let runner = FakeRunner::with_outputs(vec![output(
            0,
            r#"{"result":{"panes":[{"pane_id":"p1","tab_id":"t1","workspace_id":"other"}]}}"#,
            "",
        )]);
        let error = client("direct", runner).panes(Some("wanted")).unwrap_err();
        assert!(error.to_string().contains("expected \"wanted\""));
    }

    #[test]
    fn create_commands_never_steal_focus_and_narrow_ids() {
        let runner = FakeRunner::with_outputs(vec![
            output(
                0,
                r#"{"result":{"workspace":{"workspace_id":"w"},"tab":{"tab_id":"t"},"root_pane":{"pane_id":"p"}}}"#,
                "",
            ),
            output(0, r#"{"result":{"tab":{"tab_id":"t2"}}}"#, ""),
        ]);
        let client = client("direct", runner.clone());
        assert_eq!(
            client.create_workspace("project", "/work").unwrap(),
            ("w".to_owned(), "t".to_owned(), "p".to_owned())
        );
        assert_eq!(client.create_tab("w", "agent", "/work").unwrap(), "t2");
        let calls = runner.calls();
        assert_eq!(calls[0].last().map(String::as_str), Some("--no-focus"));
        assert_eq!(calls[1].last().map(String::as_str), Some("--no-focus"));
    }

    #[test]
    fn malformed_json_and_wrong_integer_types_are_unavailable() {
        let malformed = FakeRunner::with_outputs(vec![output(0, "not-json", "")]);
        let error = client("direct", malformed)
            .workspace_id_for_label("x")
            .unwrap_err();
        assert!(error.to_string().contains("non-JSON"));

        let wrong_type = FakeRunner::with_outputs(vec![output(
            0,
            r#"{"result":{"process_info":{"pane_id":"p","shell_pid":"7","foreground_process_group_id":7,"foreground_processes":[]}}}"#,
            "",
        )]);
        let error = client("direct", wrong_type).process_info("p").unwrap_err();
        assert!(error.to_string().contains("not an integer"));
    }

    #[test]
    fn pane_read_is_plain_text_but_run_success_requires_no_json() {
        let runner =
            FakeRunner::with_outputs(vec![output(0, "$ trailing text\n", ""), output(0, "", "")]);
        let client = client("direct", runner.clone());
        assert_eq!(
            client.read("p", "recent-unwrapped", Some(4)).unwrap(),
            "$ trailing text\n"
        );
        client.run("p", "git status").unwrap();
        assert_eq!(
            runner.calls()[0],
            strings(&[
                "/opt/herdr/bin/herdr",
                "pane",
                "read",
                "p",
                "--source",
                "recent-unwrapped",
                "--lines",
                "4",
            ])
        );
    }
}
