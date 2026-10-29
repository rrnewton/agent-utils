//! Terminal-backend selection and the bridges between the
//! backend-neutral multiplexer traits and this crate's own transport traits.
//!
//! Herdr remains the default backend. Its existing client keeps its exact
//! Herdr CLI behaviour; this module only re-expresses it through the
//! backend-neutral multiplexer and pane-terminal traits, and gives the tmux
//! backend both an [`AgentApi`] (the durable direct-pane transport:
//! send/drain/status/read against an explicit pane) and a
//! [`ManagedApi`](crate::subagents::ManagedApi) (agent lifecycle:
//! start/adopt/stop/revive and the process proofs behind them), so a
//! registry can run entirely under tmux with no Herdr installed.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use term_backend::{
    AgentStatus, BackendConfig, BackendKind, Capabilities, CreatedPane, Multiplexer, PaneId,
    PaneInfo as BackendPaneInfo, PaneTerminal, ScreenRead, TabId, TabInfo, TermError, TmuxBackend,
    WorkspaceId, WorkspaceInfo,
};

use crate::agent::{AgentApi, AgentRuntime};
use crate::client::{
    live_custom_process, process_has_no_descendants, resolve_harness_executable, shell_join,
    supported_shell_process, AgentIdentity, AgentPaneInfo, CustomProcessIdentity, HerdrClient,
    Pane, PaneShellProof, ProcessLiveness,
};
use crate::error::{AdapterError, Result};
use crate::submission::{GuardedInput, GuardedPrompt, PromptTerminal, Submission, SubmitTimeouts};

/// Environment variable selecting the terminal backend when no flag or
/// profile configuration names one.
pub const BACKEND_ENV: &str = "AGENTCTL_TERM_BACKEND";

fn term_err<T>(result: term_backend::Result<T>) -> Result<T> {
    result.map_err(|error| AdapterError::unavailable(error.to_string()))
}

fn client_err<T>(result: Result<T>) -> term_backend::Result<T> {
    result.map_err(|error| TermError::failed(error.to_string()))
}

/// Resolve which terminal backend this invocation uses, and whether an
/// operator selected it (flag, environment, or profile key) rather than
/// detection picking it.
///
/// Precedence: an explicit flag value, then `$AGENTCTL_TERM_BACKEND`, then
/// the project profile configuration beside the registry, then detection
/// (Herdr when its executable is available, else tmux). The profile read
/// is best-effort: commands that never loaded profile policy before must
/// not start failing on its file permissions just to pick a backend, so
/// an unreadable profile config simply selects nothing here (commands
/// that do load profiles still enforce that policy in full). Detection
/// failure falls back to Herdr so the Herdr client's own "not found"
/// error fires, exactly as before backends were selectable.
pub fn select_backend(
    explicit: Option<&str>,
    environment: &dyn Fn(&str) -> Option<String>,
    registry: &Path,
    herdr_bin: &Path,
    tmux_bin: &Path,
) -> std::result::Result<(BackendKind, bool), AdapterError> {
    if let Some(raw) = explicit {
        return BackendKind::parse(raw)
            .map(|kind| (kind, true))
            .map_err(|error| AdapterError::unavailable(error.to_string()));
    }
    if let Some(raw) = environment(BACKEND_ENV).filter(|value| !value.is_empty()) {
        return BackendKind::parse(&raw)
            .map(|kind| (kind, true))
            .map_err(|error| AdapterError::unavailable(format!("${BACKEND_ENV}: {error}")));
    }
    if let Ok(Some(kind)) = crate::profiles::backend_for_registry(registry) {
        return Ok((kind, true));
    }
    let config = BackendConfig {
        kind: None,
        herdr_bin: herdr_bin.to_path_buf(),
        tmux_bin: tmux_bin.to_path_buf(),
        tmux_socket: None,
    };
    Ok((
        term_backend::detect_kind(&config).unwrap_or(BackendKind::Herdr),
        false,
    ))
}

// ---------------------------------------------------------------------------
// HerdrClient expressed through the backend-neutral traits. Every method
// delegates to the client's existing operation, so Herdr behaviour (CLI
// argument shapes, validation, error text) is unchanged.

impl Multiplexer for HerdrClient {
    fn backend_name(&self) -> &'static str {
        "herdr"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            native_agent_status: true,
            native_prompt: true,
        }
    }

    fn list_workspaces(&self) -> term_backend::Result<Vec<WorkspaceInfo>> {
        // No existing client operation lists workspace labels wholesale;
        // derive them from the pane census plus per-workspace label reads.
        let panes = client_err(HerdrClient::panes(self, None))?;
        let mut seen: Vec<WorkspaceInfo> = Vec::new();
        for pane in &panes {
            if seen
                .iter()
                .any(|entry| entry.id.as_str() == pane.workspace_id)
            {
                continue;
            }
            let label = client_err(HerdrClient::workspace_label(self, &pane.workspace_id))?;
            seen.push(WorkspaceInfo {
                id: pane.workspace_id.clone().into(),
                label,
            });
        }
        Ok(seen)
    }

    fn create_workspace(
        &self,
        label: &str,
        cwd: &Path,
        environment: &[String],
    ) -> term_backend::Result<CreatedPane> {
        let (workspace, tab, pane) = client_err(HerdrClient::create_workspace(
            self,
            label,
            &cwd.display().to_string(),
            environment,
        ))?;
        Ok(CreatedPane {
            workspace_id: workspace.into(),
            tab_id: tab.into(),
            pane_id: pane.into(),
        })
    }

    fn focus_workspace(&self, _workspace: &WorkspaceId) -> term_backend::Result<()> {
        Err(TermError::unsupported(
            "agentctl's Herdr client has no workspace-focus operation; focus a pane instead",
        ))
    }

    fn rename_workspace(&self, _workspace: &WorkspaceId, _label: &str) -> term_backend::Result<()> {
        Err(TermError::unsupported(
            "agentctl's Herdr client has no workspace-rename operation",
        ))
    }

    fn close_workspace(&self, _workspace: &WorkspaceId) -> term_backend::Result<()> {
        Err(TermError::unsupported(
            "agentctl's Herdr client has no workspace-close operation",
        ))
    }

    fn list_tabs(&self, workspace: &WorkspaceId) -> term_backend::Result<Vec<TabInfo>> {
        let labels = client_err(HerdrClient::tab_labels(self, workspace.as_str()))?;
        Ok(labels
            .into_iter()
            .map(|(id, label)| TabInfo {
                id: id.into(),
                workspace_id: workspace.clone(),
                label,
            })
            .collect())
    }

    fn create_tab(
        &self,
        workspace: &WorkspaceId,
        label: &str,
        cwd: &Path,
        environment: &[String],
    ) -> term_backend::Result<CreatedPane> {
        let (tab, pane) = client_err(HerdrClient::create_tab_with_pane(
            self,
            workspace.as_str(),
            label,
            &cwd.display().to_string(),
            environment,
        ))?;
        Ok(CreatedPane {
            workspace_id: workspace.clone(),
            tab_id: tab.into(),
            pane_id: pane.into(),
        })
    }

    fn focus_tab(&self, _tab: &TabId) -> term_backend::Result<()> {
        Err(TermError::unsupported(
            "agentctl's Herdr client has no tab-focus operation; focus a pane instead",
        ))
    }

    fn rename_tab(&self, tab: &TabId, label: &str) -> term_backend::Result<()> {
        client_err(HerdrClient::rename_tab(self, tab.as_str(), label))
    }

    fn close_tab(&self, tab: &TabId) -> term_backend::Result<()> {
        client_err(HerdrClient::close_tab(self, tab.as_str()))
    }

    fn list_panes(
        &self,
        workspace: Option<&WorkspaceId>,
    ) -> term_backend::Result<Vec<BackendPaneInfo>> {
        let panes = client_err(HerdrClient::panes(self, workspace.map(WorkspaceId::as_str)))?;
        Ok(panes
            .into_iter()
            .map(|pane| BackendPaneInfo {
                id: pane.pane_id.into(),
                tab_id: Some(pane.tab_id.into()),
                workspace_id: Some(pane.workspace_id.into()),
                cwd: None,
                title: None,
                agent: None,
                agent_status: None,
                pid: None,
                terminal_id: None,
            })
            .collect())
    }

    fn focus_pane(&self, pane: &PaneId) -> term_backend::Result<()> {
        client_err(HerdrClient::focus_pane(self, pane.as_str()))
    }

    fn close_pane(&self, pane: &PaneId) -> term_backend::Result<()> {
        client_err(HerdrClient::close_pane(self, pane.as_str()))
    }
}

impl PaneTerminal for HerdrClient {
    fn pane_info(&self, pane: &PaneId) -> term_backend::Result<BackendPaneInfo> {
        let info = client_err(HerdrClient::pane_info(self, pane.as_str()))?;
        Ok(BackendPaneInfo {
            id: info.pane_id.into(),
            tab_id: info.tab_id.map(TabId::from),
            workspace_id: Some(info.workspace_id.into()),
            cwd: Some(PathBuf::from(info.cwd)),
            title: None,
            agent: info.agent,
            agent_status: Some(AgentStatus::parse(&info.status)),
            pid: None,
            terminal_id: info.terminal_id,
        })
    }

    fn read_screen(&self, pane: &PaneId, read: ScreenRead) -> term_backend::Result<String> {
        let (source, lines) = match read.scrollback_lines {
            Some(0) => ("visible", None),
            other => ("recent", other),
        };
        if read.ansi && source != "visible" {
            return Err(TermError::unsupported(
                "styled (ansi) reads are visible-screen only through this Herdr client view",
            ));
        }
        if read.ansi {
            return client_err(self.read_screen_with_cancellation(pane.as_str(), &|| false));
        }
        client_err(HerdrClient::read(self, pane.as_str(), source, lines))
    }

    fn paste_text(&self, pane: &PaneId, text: &str) -> term_backend::Result<()> {
        client_err(HerdrClient::send_text(self, pane.as_str(), text))
    }

    fn send_keys(&self, pane: &PaneId, keys: &str) -> term_backend::Result<()> {
        client_err(HerdrClient::send_keys(self, pane.as_str(), keys))
    }

    fn prompt(&self, pane: &PaneId, text: &str) -> term_backend::Result<()> {
        client_err(HerdrClient::prompt_agent(self, pane.as_str(), text))
    }

    fn wait_agent_status(
        &self,
        pane: &PaneId,
        status: &AgentStatus,
        timeout_ms: u64,
    ) -> term_backend::Result<()> {
        client_err(HerdrClient::wait_agent_status(
            self,
            pane.as_str(),
            status.as_str(),
            timeout_ms,
        ))
    }
}

// ---------------------------------------------------------------------------
// The tmux backend behind agentctl's durable direct-pane transport.

/// Infer a wire agent status from a tmux pane's screen: tmux tracks none,
/// so the same TUI classification used for status-less panes decides.
fn infer_status(backend: &TmuxBackend, info: &BackendPaneInfo) -> String {
    // `followup_tui` is `harness_tui` plus Muse, whose editor forms the
    // status classification also reads.
    let Some(adapter) = crate::harness_tui::followup_tui(info.agent.as_deref()) else {
        return "unknown".to_owned();
    };
    let Ok(screen) = backend.read_screen(&info.id, ScreenRead::visible()) else {
        return "unknown".to_owned();
    };
    match adapter.classify(&screen).state {
        crate::harness_tui::State::IdleComposer => "idle".to_owned(),
        crate::harness_tui::State::Working => "working".to_owned(),
        crate::harness_tui::State::BlockingQuestion | crate::harness_tui::State::MenuOrPicker => {
            "blocked".to_owned()
        }
        crate::harness_tui::State::Unknown => "unknown".to_owned(),
    }
}

/// The terminal identity agentctl records for a tmux pane.
///
/// tmux has no terminal-inside-a-pane identity separate from the pane, but
/// a pane can be respawned in place with a fresh shell. Naming the pane
/// plus its shell's full kernel generation (boot, pid, start time)
/// changes exactly then, which is what the recorded identity must
/// detect. When the generation cannot be read the id falls back to the
/// pane and pid alone; a later successful read then reports a different
/// id and recipient verification refuses, which is the safe direction.
fn tmux_terminal_id(info: &BackendPaneInfo) -> Option<String> {
    if let Some(terminal) = &info.terminal_id {
        return Some(terminal.clone());
    }
    let pid = info.pid?;
    match agent_queue::identity::observe(u64::from(pid)) {
        Ok(observed) => Some(format!(
            "tmux:{}:{pid}:{}:{}",
            info.id, observed.identity.boot_id, observed.identity.starttime_ticks
        )),
        Err(_) => Some(format!("tmux:{}:{pid}", info.id)),
    }
}

fn to_agent_pane_info(backend: &TmuxBackend, info: BackendPaneInfo) -> AgentPaneInfo {
    let status = infer_status(backend, &info);
    let terminal_id = tmux_terminal_id(&info);
    AgentPaneInfo {
        pane_id: info.id.to_string(),
        workspace_id: info
            .workspace_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        cwd: info
            .cwd
            .map(|cwd| cwd.display().to_string())
            .unwrap_or_default(),
        agent: info.agent,
        status,
        session_agent: None,
        session_value: None,
        scroll: None,
        terminal_id,
        tab_id: info.tab_id.map(|id| id.to_string()),
    }
}

/// A [`PaneTerminal`] read as the verified-submission layer's
/// [`PromptTerminal`]: visible rows with SGR styling retained.
struct BackendPromptTerminal<'a>(&'a dyn PaneTerminal);

impl PromptTerminal for BackendPromptTerminal<'_> {
    fn read_screen(&self, pane_id: &str) -> Result<String> {
        term_err(self.0.read_screen(
            &PaneId::from(pane_id),
            ScreenRead {
                scrollback_lines: Some(0),
                ansi: true,
            },
        ))
    }

    fn send_text(&self, pane_id: &str, text: &str) -> Result<()> {
        term_err(self.0.paste_text(&PaneId::from(pane_id), text))
    }

    fn send_keys(&self, pane_id: &str, keys: &str) -> Result<()> {
        term_err(self.0.send_keys(&PaneId::from(pane_id), keys))
    }
}

/// Poll a tmux pane's screen-inferred status until it matches, the
/// deadline passes, or the caller cancels. tmux has no native status.
fn wait_inferred_status(
    backend: &TmuxBackend,
    pane_id: &str,
    status: &str,
    timeout_ms: u64,
    cancelled: &dyn Fn() -> bool,
    sleep: &dyn Fn(Duration),
) -> Result<()> {
    let deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout_ms))
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400));
    loop {
        if cancelled() {
            return Err(AdapterError::cancelled(
                "terminal control operation was cancelled",
            ));
        }
        let info = AgentApi::pane_info(backend, pane_id)?;
        if info.status == status {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AdapterError::unavailable(format!(
                "wait for pane {pane_id} status {status}: timed out; last inferred status={}",
                info.status
            )));
        }
        sleep(Duration::from_millis(100));
    }
}

impl AgentApi for TmuxBackend {
    fn panes(&self) -> Result<Vec<Pane>> {
        Ok(term_err(Multiplexer::list_panes(self, None))?
            .into_iter()
            .map(|info| Pane {
                pane_id: info.id.to_string(),
                tab_id: info.tab_id.map(|id| id.to_string()).unwrap_or_default(),
                workspace_id: info
                    .workspace_id
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
            })
            .collect())
    }

    fn pane_info(&self, pane_id: &str) -> Result<AgentPaneInfo> {
        let info = term_err(PaneTerminal::pane_info(self, &PaneId::from(pane_id)))?;
        Ok(to_agent_pane_info(self, info))
    }

    fn workspace_label(&self, workspace_id: &str) -> Result<String> {
        term_err(self.list_workspaces())?
            .into_iter()
            .find(|entry| entry.id.as_str() == workspace_id)
            .map(|entry| entry.label)
            .ok_or_else(|| {
                AdapterError::unavailable(format!("tmux session {workspace_id:?} not found"))
            })
    }

    fn run(&self, pane_id: &str, text: &str) -> Result<()> {
        term_err(PaneTerminal::prompt(self, &PaneId::from(pane_id), text))
    }

    fn submit_with_runtime(
        &self,
        pane_id: &str,
        text: &str,
        runtime: &dyn AgentRuntime,
    ) -> Result<Submission> {
        let harness = match PaneTerminal::pane_info(self, &PaneId::from(pane_id)) {
            Ok(info) => info.agent.unwrap_or_default(),
            Err(error) => {
                return Ok(Submission::NotStaged(format!(
                    "pane {pane_id}: harness lookup failed before typing: {error}"
                )))
            }
        };
        if crate::submission::verifies(&harness, text) {
            let terminal = BackendPromptTerminal(self);
            return crate::submission::submit_verified(
                &terminal,
                pane_id,
                &harness,
                text,
                SubmitTimeouts::default(),
                runtime,
            );
        }
        runtime.input_started();
        self.run(pane_id, text).map(|()| Submission::Unconfirmed)
    }

    fn wait_agent_status(&self, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()> {
        wait_inferred_status(self, pane_id, status, timeout_ms, &|| false, &|pause| {
            std::thread::sleep(pause)
        })
    }

    fn wait_agent_status_with_runtime(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
        runtime: &dyn AgentRuntime,
    ) -> Result<()> {
        wait_inferred_status(
            self,
            pane_id,
            status,
            timeout_ms,
            &|| runtime.cancelled(),
            &|pause| runtime.sleep(pause),
        )
    }

    fn read(&self, pane_id: &str, source: &str, lines: Option<usize>) -> Result<String> {
        let read = if source == "visible" {
            ScreenRead::visible()
        } else {
            ScreenRead::recent(lines.unwrap_or(500))
        };
        term_err(PaneTerminal::read_screen(
            self,
            &PaneId::from(pane_id),
            read,
        ))
    }
}

// ---------------------------------------------------------------------------
// The tmux backend as a full agent lifecycle client (`ManagedApi`).
//
// Herdr reports a pane's shell pid, foreground process group, and
// foreground processes; tmux reports only the pane's shell pid
// (`#{pane_pid}`). The same facts are in `/proc`: the shell's stat names
// its terminal's foreground group, and the process table names which of
// the shell's descendants belong to that group. Every proof below is the
// Herdr path's proof re-derived from those two sources, with one widening:
// where Herdr pins only a *sole* foreground process, tmux pins the unique
// harness-matching process in the foreground group (or the one such
// process the others descend from), because a wrapped launch — the case
// this backend exists for — always has the wrapper in the group too.

/// One visible process, as the pane proofs need it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TmuxProc {
    pid: u64,
    ppid: u64,
    pgrp: u64,
    tpgid: i64,
    argv: Vec<String>,
}

/// A tmux pane's process state, the analogue of Herdr's process-info.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TmuxPaneProcesses {
    shell_pid: u64,
    foreground_pgid: u64,
    /// The shell and its descendants in the foreground group, ascending.
    foreground: Vec<u64>,
    /// Every descendant of the shell, foreground or not.
    descendants: HashSet<u64>,
}

/// `(ppid, pgrp, tpgid)` from a `/proc/PID/stat` record.
fn parse_tmux_stat(stat: &[u8]) -> Option<(u64, u64, i64)> {
    let close = stat.iter().rposition(|byte| *byte == b')')?;
    let fields: Vec<&[u8]> = stat[close + 1..]
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|field| !field.is_empty())
        .collect();
    let number = |index: usize| {
        fields
            .get(index)
            .and_then(|field| std::str::from_utf8(field).ok())
            .and_then(|field| field.parse::<i64>().ok())
    };
    Some((
        u64::try_from(number(1)?).ok()?,
        u64::try_from(number(2)?).ok()?,
        number(5)?,
    ))
}

fn scan_tmux_processes() -> Result<Vec<TmuxProc>> {
    #[cfg(not(target_os = "linux"))]
    {
        Err(AdapterError::unavailable(
            "tmux process proofs require Linux /proc",
        ))
    }
    #[cfg(target_os = "linux")]
    {
        let entries = fs::read_dir("/proc").map_err(|error| {
            AdapterError::unavailable(format!("cannot enumerate process table: {error}"))
        })?;
        let mut processes = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| {
                AdapterError::unavailable(format!("cannot inspect process table: {error}"))
            })?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
                .filter(|pid| *pid > 0 && *pid <= i32::MAX as u64)
            else {
                continue;
            };
            let root = entry.path();
            // A process that exits mid-scan is simply absent from it; the
            // callers re-observe the exact pids they rely on.
            let Ok(stat) = fs::read(root.join("stat")) else {
                continue;
            };
            let Some((ppid, pgrp, tpgid)) = parse_tmux_stat(&stat) else {
                continue;
            };
            let argv = fs::read(root.join("cmdline"))
                .map(|raw| {
                    raw.split(|byte| *byte == 0)
                        .filter(|part| !part.is_empty())
                        .take(3)
                        .map(|part| String::from_utf8_lossy(part).into_owned())
                        .collect()
                })
                .unwrap_or_default();
            processes.push(TmuxProc {
                pid,
                ppid,
                pgrp,
                tpgid,
                argv,
            });
        }
        Ok(processes)
    }
}

/// The pane process state for `shell_pid` within one process-table scan.
fn tmux_pane_processes(shell_pid: u64, processes: &[TmuxProc]) -> Option<TmuxPaneProcesses> {
    let shell = processes.iter().find(|entry| entry.pid == shell_pid)?;
    let mut children: HashMap<u64, Vec<u64>> = HashMap::new();
    for entry in processes {
        children.entry(entry.ppid).or_default().push(entry.pid);
    }
    let mut descendants = HashSet::new();
    let mut queue = VecDeque::from([shell_pid]);
    while let Some(pid) = queue.pop_front() {
        for child in children.get(&pid).into_iter().flatten() {
            if descendants.insert(*child) {
                queue.push_back(*child);
            }
        }
    }
    let foreground_pgid = if shell.tpgid > 0 {
        shell.tpgid as u64
    } else {
        shell.pgrp
    };
    let mut foreground: Vec<u64> = processes
        .iter()
        .filter(|entry| {
            entry.pgrp == foreground_pgid
                && (entry.pid == shell_pid || descendants.contains(&entry.pid))
        })
        .map(|entry| entry.pid)
        .collect();
    foreground.sort_unstable();
    Some(TmuxPaneProcesses {
        shell_pid,
        foreground_pgid,
        foreground,
        descendants,
    })
}

fn pane_processes(backend: &TmuxBackend, pane_id: &str) -> Result<TmuxPaneProcesses> {
    Ok(pane_state_and_processes(backend, pane_id)?.0)
}

fn pane_state_and_processes(
    backend: &TmuxBackend,
    pane_id: &str,
) -> Result<(TmuxPaneProcesses, Vec<TmuxProc>)> {
    let info = term_err(PaneTerminal::pane_info(backend, &PaneId::from(pane_id)))?;
    let shell_pid = info.pid.map(u64::from).ok_or_else(|| {
        AdapterError::unavailable(format!("tmux pane {pane_id} reports no pane_pid"))
    })?;
    let processes = scan_tmux_processes()?;
    let state = tmux_pane_processes(shell_pid, &processes).ok_or_else(|| {
        AdapterError::unavailable(format!(
            "tmux pane {pane_id} shell process {shell_pid} is not in the process table"
        ))
    })?;
    Ok((state, processes))
}

/// A harness program held open so its image identity cannot change under
/// the comparison: the resolved path plus its device and inode.
struct TmuxProgram {
    path: PathBuf,
    device: u64,
    inode: u64,
    /// Shebang interpreter basename for a script program.
    interpreter: Option<String>,
    _file: fs::File,
}

fn pin_tmux_program(path: PathBuf) -> Result<TmuxProgram> {
    use std::io::Read as _;
    let mut file = fs::File::open(&path).map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot pin harness executable {}: {error}",
            path.display()
        ))
    })?;
    let metadata = file.metadata().map_err(|error| {
        AdapterError::unavailable(format!(
            "cannot inspect harness executable {}: {error}",
            path.display()
        ))
    })?;
    #[cfg(target_os = "linux")]
    let (device, inode) = (metadata.dev(), metadata.ino());
    #[cfg(not(target_os = "linux"))]
    let (device, inode) = (0, 0);
    if !metadata.is_file() || device == 0 || inode == 0 {
        return Err(AdapterError::unavailable(format!(
            "harness executable {} has no pin-able image identity",
            path.display()
        )));
    }
    // A script's kernel image is its interpreter's, so matching a process
    // to this program needs the interpreter the shebang names — otherwise
    // any program merely holding the script path as an argument (an
    // editor, a pager) would look like the harness.
    let mut head = [0_u8; 256];
    let read = file.read(&mut head).unwrap_or(0);
    let interpreter = shebang_interpreter(&head[..read]);
    Ok(TmuxProgram {
        path,
        device,
        inode,
        interpreter,
        _file: file,
    })
}

/// The interpreter basename a `#!` line names, resolving `env NAME`.
fn shebang_interpreter(head: &[u8]) -> Option<String> {
    let line = head.strip_prefix(b"#!")?;
    let line = &line[..line
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(line.len())];
    let mut tokens = line
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|token| !token.is_empty())
        .map(|token| String::from_utf8_lossy(token).into_owned());
    let first = tokens.next()?;
    let base = |token: &str| {
        token
            .rsplit('/')
            .next()
            .filter(|base| !base.is_empty())
            .map(str::to_owned)
    };
    if base(&first).as_deref() == Some("env") {
        tokens
            .find(|token| !token.starts_with('-'))
            .and_then(|token| base(&token))
    } else {
        base(&first)
    }
}

/// The pinned program for `kind`, when its fixed install location resolves.
fn program_for_kind(kind: &str) -> Option<TmuxProgram> {
    resolve_harness_executable(kind)
        .ok()
        .and_then(|path| pin_tmux_program(path).ok())
}

fn program_basename_matches(path: &Path, kind: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|base| base.strip_suffix(".real").unwrap_or(base) == kind)
}

/// How a process matched the harness program, strongest first. An exact
/// image (same device and inode as the resolved program) outranks the
/// shebang interpreter running the resolved script, which outranks a
/// mere name match used only when no fixed program resolves.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum MatchClass {
    Name,
    Script,
    Image,
}

fn exe_basename(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// The class at which one observed process runs `program` (or, with no
/// resolved program, a program named `kind`).
///
/// With a resolved program there is no name fallthrough: a different
/// binary that happens to share the kind's name, a process that set its
/// argv\[0\] to the kind, or a pager holding the script as an argument
/// must not pin as the harness. The script case additionally requires
/// the process image to be the shebang's interpreter.
fn process_match_class(
    observed: &agent_queue::identity::LiveProcess,
    argv: &[String],
    program: Option<&TmuxProgram>,
    kind: &str,
) -> Option<MatchClass> {
    if let Some(program) = program {
        if observed.identity.executable_device == program.device
            && observed.identity.executable_inode == program.inode
        {
            return Some(MatchClass::Image);
        }
        if let Some(interpreter) = &program.interpreter {
            // The shebang names a command (`python3`); the kernel image
            // is its resolved target, often versioned (`python3.12`).
            let runs_script = exe_basename(&observed.executable_path).is_some_and(|base| {
                base == interpreter
                    || base.strip_prefix(interpreter.as_str()).is_some_and(|rest| {
                        rest.starts_with('.') || rest.starts_with(|c: char| c.is_ascii_digit())
                    })
            }) && argv
                .iter()
                .take(2)
                .any(|argument| fs::canonicalize(argument).is_ok_and(|path| path == program.path));
            if runs_script {
                return Some(MatchClass::Script);
            }
        }
        return None;
    }
    if program_basename_matches(&observed.executable_path, kind) {
        return Some(MatchClass::Name);
    }
    // An interpreter whose first file argument is named like the kind,
    // for a wrapped install no fixed location resolves. argv[0] alone
    // never counts: it is caller-chosen text.
    let interpreter_like = exe_basename(&observed.executable_path).is_some_and(|base| {
        base.starts_with("python")
            || matches!(
                base,
                "node" | "ruby" | "perl" | "bash" | "sh" | "deno" | "bun"
            )
    });
    if interpreter_like
        && argv
            .get(1)
            .is_some_and(|script| program_basename_matches(Path::new(script), kind))
    {
        return Some(MatchClass::Name);
    }
    None
}

/// Pick the harness to pin among foreground candidates: the only one, or
/// the only one no other candidate descends from (the launched program,
/// not a same-image child it started).
fn choose_candidate(candidates: &[u64], processes: &[TmuxProc]) -> Option<u64> {
    match candidates {
        [] => None,
        [only] => Some(*only),
        _ => {
            let parents: HashMap<u64, u64> = processes
                .iter()
                .map(|entry| (entry.pid, entry.ppid))
                .collect();
            let is_descendant = |pid: u64, ancestor: u64| {
                let mut current = pid;
                for _ in 0..processes.len() + 1 {
                    match parents.get(&current) {
                        Some(&parent) if parent == ancestor => return true,
                        Some(&parent) if parent != current && parent != 0 => current = parent,
                        _ => return false,
                    }
                }
                false
            };
            let roots: Vec<u64> = candidates
                .iter()
                .copied()
                .filter(|candidate| {
                    !candidates
                        .iter()
                        .any(|other| other != candidate && is_descendant(*candidate, *other))
                })
                .collect();
            match roots.as_slice() {
                [root] => Some(*root),
                _ => None,
            }
        }
    }
}

/// The command line of one pinned process generation, read between two
/// identity proofs so a recycled pid cannot lend its argv to the pin.
fn proved_argv(expected: &CustomProcessIdentity) -> Option<Vec<String>> {
    const LIMIT: usize = 256 << 10;
    let same =
        || live_custom_process(expected.pid).is_ok_and(|observed| observed.identity == *expected);
    if !same() {
        return None;
    }
    let mut raw = fs::read(format!("/proc/{}/cmdline", expected.pid)).ok()?;
    if !same() || raw.is_empty() || raw.len() > LIMIT {
        return None;
    }
    if raw.last() == Some(&0) {
        raw.pop();
    }
    let argv = raw
        .split(|byte| *byte == 0)
        .map(|item| String::from_utf8(item.to_vec()).ok())
        .collect::<Option<Vec<_>>>()?;
    (!argv.is_empty() && argv.iter().all(|item| !item.is_empty())).then_some(argv)
}

/// The pinned harness process in one pane state, when it can be pinned.
///
/// Candidates are chosen within the strongest match class present, so a
/// launcher script that merely started the real binary never outranks
/// that binary. The chosen process is then re-proved against a fresh,
/// identity-bracketed argv read: the argv that elected it and the
/// identity being pinned must still belong to the same generation.
fn harness_candidate(
    state: &TmuxPaneProcesses,
    processes: &[TmuxProc],
    kind: &str,
    program: Option<&TmuxProgram>,
) -> Result<Option<CustomProcessIdentity>> {
    let argv_of: HashMap<u64, &[String]> = processes
        .iter()
        .map(|entry| (entry.pid, entry.argv.as_slice()))
        .collect();
    let mut by_class: HashMap<MatchClass, Vec<(u64, CustomProcessIdentity)>> = HashMap::new();
    for pid in &state.foreground {
        // The shell itself is a candidate only when it exec'd the harness:
        // its image then matches, which the program check decides.
        let Ok(observed) = live_custom_process(*pid) else {
            continue;
        };
        if observed.process_group_id != state.foreground_pgid {
            continue;
        }
        let argv = argv_of.get(pid).copied().unwrap_or_default();
        if let Some(class) = process_match_class(&observed, argv, program, kind) {
            by_class
                .entry(class)
                .or_default()
                .push((*pid, observed.identity));
        }
    }
    for class in [MatchClass::Image, MatchClass::Script, MatchClass::Name] {
        let Some(candidates) = by_class.get(&class) else {
            continue;
        };
        let pids: Vec<u64> = candidates.iter().map(|(pid, _)| *pid).collect();
        let Some(pid) = choose_candidate(&pids, processes) else {
            continue;
        };
        let identity = candidates
            .iter()
            .find(|(candidate, _)| *candidate == pid)
            .map(|(_, identity)| identity.clone())
            .expect("chosen pid came from candidates");
        let Ok(observed) = live_custom_process(pid) else {
            continue;
        };
        if observed.identity != identity || observed.process_group_id != state.foreground_pgid {
            continue;
        }
        let Some(argv) = proved_argv(&identity) else {
            continue;
        };
        if process_match_class(&observed, &argv, program, kind) == Some(class) {
            return Ok(Some(identity));
        }
    }
    Ok(None)
}

/// Whether `expected` is still a foreground process of the pane, in the
/// same kernel generation and image.
fn verify_foreground_identity(
    state: &TmuxPaneProcesses,
    expected: &CustomProcessIdentity,
) -> Result<bool> {
    if !expected.valid() || !state.foreground.contains(&expected.pid) {
        return Ok(false);
    }
    let Ok(observed) = live_custom_process(expected.pid) else {
        return Ok(false);
    };
    Ok(observed.process_group_id == state.foreground_pgid && observed.identity == *expected)
}

fn idle_shell_proof(state: &TmuxPaneProcesses) -> Result<Option<PaneShellProof>> {
    if state.foreground_pgid != state.shell_pid || state.foreground != [state.shell_pid] {
        return Ok(None);
    }
    let Some(observed) = supported_shell_process(state.shell_pid)? else {
        return Ok(None);
    };
    if observed.process_group_id != state.shell_pid || !process_has_no_descendants(state.shell_pid)?
    {
        return Ok(None);
    }
    let Some(confirmed) = supported_shell_process(state.shell_pid)? else {
        return Ok(None);
    };
    if confirmed.identity != observed.identity
        || confirmed.process_group_id != observed.process_group_id
        || confirmed.executable_path != observed.executable_path
    {
        return Ok(None);
    }
    // The shell's argv[0] must name the same program as its image: a
    // renamed or substituted argv[0] (a login shell's leading `-` aside)
    // is not the idle shell the proof claims, matching the Herdr path's
    // argv-to-image coherence check.
    let Some(argv) = proved_argv(&observed.identity) else {
        return Ok(None);
    };
    let argv0 = argv[0].trim_start_matches('-');
    let coherent = if argv0.contains('/') {
        fs::canonicalize(argv0).is_ok_and(|path| path == observed.executable_path)
    } else {
        exe_basename(&observed.executable_path) == Some(argv0)
    };
    if !coherent {
        return Ok(None);
    }
    Ok(Some(PaneShellProof {
        identity: observed.identity,
        executable_path: observed.executable_path,
    }))
}

fn pane_idle_shell_proof(backend: &TmuxBackend, pane_id: &str) -> Result<Option<PaneShellProof>> {
    let before = pane_processes(backend, pane_id)?;
    let Some(proof) = idle_shell_proof(&before)? else {
        return Ok(None);
    };
    let after = pane_processes(backend, pane_id)?;
    if after != before || idle_shell_proof(&after)?.as_ref() != Some(&proof) {
        return Ok(None);
    }
    Ok(Some(proof))
}

/// Run one program in a fresh shell pane and pin its foreground process.
///
/// Readiness is the pinned process plus, for a harness with a screen
/// model, a recognisable composer screen (idle, working, or blocked all
/// prove the TUI is up). A workspace-trust prompt is refused, never
/// answered, exactly as on the Herdr custom-pane path.
fn tmux_run_program(
    backend: &TmuxBackend,
    pane_id: &str,
    kind: &str,
    program: &TmuxProgram,
    arguments: &[String],
    timeout: Duration,
) -> Result<CustomProcessIdentity> {
    if timeout.is_zero() || timeout > Duration::from_secs(300) {
        return Err(AdapterError::unavailable(
            "agent startup timeout must be between 0 and 300 seconds",
        ));
    }
    let mut command = vec![program.path.display().to_string()];
    command.extend_from_slice(arguments);
    let line = shell_join(&command)?;
    term_err(PaneTerminal::prompt(backend, &PaneId::from(pane_id), &line))?;
    let deadline = Instant::now() + timeout;
    let mut pinned: Option<CustomProcessIdentity> = None;
    loop {
        let (state, processes) = pane_state_and_processes(backend, pane_id)?;
        let candidate = harness_candidate(&state, &processes, kind, Some(program))?;
        if let Some(identity) = candidate {
            pinned = Some(identity.clone());
            let screen = AgentApi::read(backend, pane_id, "visible", Some(200))?;
            if crate::client::relay_trust_prompt(&screen) {
                return Err(AdapterError::unavailable(format!(
                    "{kind} workspace trust prompt requires human attention; no input was submitted"
                )));
            }
            let ready = match crate::harness_tui::followup_tui(Some(kind)) {
                Some(adapter) => {
                    adapter.classify(&screen).state != crate::harness_tui::State::Unknown
                }
                None => true,
            };
            if ready {
                let (state, _) = pane_state_and_processes(backend, pane_id)?;
                if verify_foreground_identity(&state, &identity)? {
                    return Ok(identity);
                }
                pinned = None;
            }
        }
        if Instant::now() >= deadline {
            return Err(AdapterError::unavailable(if pinned.is_some() {
                format!(
                    "harness {kind:?} did not reach a recognisable composer in pane {pane_id} after {}s; read the pane for its error",
                    timeout.as_secs()
                )
            } else {
                format!(
                    "{} was not the foreground process in pane {pane_id} after {}s; read the pane for its error",
                    program.path.display(),
                    timeout.as_secs()
                )
            }));
        }
        std::thread::sleep(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

impl crate::subagents::ManagedApi for TmuxBackend {
    fn record_backend(&self) -> &'static str {
        "tmux"
    }

    fn prepare_launch_shell(
        &self,
        pane: &str,
        command_line: &str,
        timeout: Duration,
    ) -> Result<()> {
        if timeout.is_zero() || timeout > Duration::from_secs(300) {
            return Err(AdapterError::unavailable(
                "launch environment timeout must be between 0 and 300 seconds",
            ));
        }
        term_err(PaneTerminal::prompt(
            self,
            &PaneId::from(pane),
            command_line,
        ))?;
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok((state, _)) = pane_state_and_processes(self, pane) {
                if state.foreground == [state.shell_pid] {
                    std::thread::sleep(Duration::from_millis(300)); // let the shell redraw its prompt
                    return Ok(());
                }
            }
            std::thread::sleep(
                Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
        Err(AdapterError::unavailable(format!(
            "tmux pane {pane} did not return to its shell prompt within {}s after the launch environment line",
            timeout.as_secs_f64()
        )))
    }

    fn start_adapter(&self, _harness: &str, _relay: bool) -> String {
        "tmux".to_owned()
    }

    fn adopt_adapter(&self) -> String {
        "tmux-foreign".to_owned()
    }

    fn inherited_workspace(&self) -> Option<String> {
        // Inside tmux, `$TMUX_PANE` names this process's own pane; its
        // session is the workspace a bare start belongs in. A Herdr
        // workspace id from the environment means nothing here.
        let pane = std::env::var("TMUX_PANE")
            .ok()
            .filter(|value| !value.is_empty())?;
        PaneTerminal::pane_info(self, &PaneId::from(pane.as_str()))
            .ok()?
            .workspace_id
            .map(|id| id.to_string())
    }

    fn process_liveness(&self, expected: &CustomProcessIdentity) -> ProcessLiveness {
        agent_queue::identity::liveness(expected)
    }

    fn workspace_id_for_label(&self, label: &str) -> Result<Option<String>> {
        let matches: Vec<String> = term_err(self.list_workspaces())?
            .into_iter()
            .filter(|workspace| workspace.label == label)
            .map(|workspace| workspace.id.to_string())
            .collect();
        match matches.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(only.clone())),
            _ => Err(AdapterError::unavailable(format!(
                "tmux session label {label:?} is ambiguous"
            ))),
        }
    }

    fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<(String, String, String)> {
        let created: CreatedPane = term_err(Multiplexer::create_workspace(
            self,
            label,
            Path::new(cwd),
            environment,
        ))?;
        Ok((
            created.workspace_id.to_string(),
            created.tab_id.to_string(),
            created.pane_id.to_string(),
        ))
    }

    fn create_tab(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<String> {
        Ok(self
            .create_tab_with_pane(workspace, label, cwd, environment)?
            .0)
    }

    fn create_tab_with_pane(
        &self,
        workspace: &str,
        label: &str,
        cwd: &str,
        environment: &[String],
    ) -> Result<(String, String)> {
        let created: CreatedPane = term_err(Multiplexer::create_tab(
            self,
            &WorkspaceId::from(workspace),
            label,
            Path::new(cwd),
            environment,
        ))?;
        Ok((created.tab_id.to_string(), created.pane_id.to_string()))
    }

    fn close_pane(&self, pane: &str) -> Result<()> {
        term_err(Multiplexer::close_pane(self, &PaneId::from(pane)))
    }

    fn focus_pane(&self, pane: &str) -> Result<()> {
        term_err(Multiplexer::focus_pane(self, &PaneId::from(pane)))
    }

    fn rename_tab(&self, tab: &str, label: &str) -> Result<()> {
        term_err(Multiplexer::rename_tab(self, &TabId::from(tab), label))
    }

    fn close_tab(&self, tab: &str) -> Result<()> {
        term_err(Multiplexer::close_tab(self, &TabId::from(tab)))
    }

    fn start_agent(
        &self,
        _name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
    ) -> Result<()> {
        // tmux has no agent registry to start a kind by name: agentctl
        // runs the harness executable itself in the fresh pane and pins
        // the process it proves, exactly as the caller then anchors.
        let program = pin_tmux_program(resolve_harness_executable(harness)?)?;
        tmux_run_program(self, pane, harness, &program, args, timeout)?;
        Ok(())
    }

    fn send_keys(&self, pane: &str, key: &str) -> Result<()> {
        term_err(PaneTerminal::send_keys(self, &PaneId::from(pane), key))
    }

    fn send_text(&self, pane: &str, text: &str) -> Result<()> {
        term_err(PaneTerminal::paste_text(self, &PaneId::from(pane), text))
    }

    fn agent_pane(&self, name: &str) -> Result<String> {
        self.agent_names()?.get(name).cloned().ok_or_else(|| {
            AdapterError::unavailable(format!(
                "no tmux tab is labelled {name:?}; tmux agent names are tab labels"
            ))
        })
    }

    fn report_agent_session(
        &self,
        _name: &str,
        _pane: &str,
        _kind: &str,
        _session: &str,
    ) -> Result<()> {
        Err(AdapterError::unavailable(
            "tmux has no native session registry to report to; agentctl records sessions in its own registry",
        ))
    }

    fn agent_identity(&self, name: &str) -> Result<AgentIdentity> {
        let pane_id = self.agent_pane(name)?;
        let info = AgentApi::pane_info(self, &pane_id)?;
        Ok(AgentIdentity {
            name: name.to_owned(),
            pane_id,
            tab_id: info.tab_id,
            terminal_id: info.terminal_id,
        })
    }

    fn rename_agent(&self, pane: &str, name: &str) -> Result<()> {
        // A tmux agent's name is its tab label.
        let info = term_err(PaneTerminal::pane_info(self, &PaneId::from(pane)))?;
        let tab = info.tab_id.ok_or_else(|| {
            AdapterError::unavailable(format!("tmux pane {pane} reports no window"))
        })?;
        term_err(Multiplexer::rename_tab(self, &tab, name))
    }

    fn agent_names(&self) -> Result<BTreeMap<String, String>> {
        // A tmux agent's name is its single-pane window's label. A label
        // carried by more than one such window (across any sessions) is
        // ambiguous and names nobody, rather than resolving to whichever
        // window was listed first.
        let mut seen: Vec<(String, String)> = Vec::new();
        for workspace in term_err(self.list_workspaces())? {
            let panes = term_err(Multiplexer::list_panes(self, Some(&workspace.id)))?;
            let mut by_tab: HashMap<String, Vec<String>> = HashMap::new();
            for pane in &panes {
                if let Some(tab) = &pane.tab_id {
                    by_tab
                        .entry(tab.to_string())
                        .or_default()
                        .push(pane.id.to_string());
                }
            }
            for tab in term_err(Multiplexer::list_tabs(self, &workspace.id))? {
                if let Some([pane]) = by_tab.get(&tab.id.to_string()).map(Vec::as_slice) {
                    seen.push((tab.label, pane.clone()));
                }
            }
        }
        let mut counts: HashMap<String, usize> = HashMap::new();
        for (label, _) in &seen {
            *counts.entry(label.clone()).or_default() += 1;
        }
        Ok(seen
            .into_iter()
            .filter(|(label, _)| counts[label.as_str()] == 1)
            .collect())
    }

    fn tab_label(&self, tab: &str) -> Result<String> {
        for workspace in term_err(self.list_workspaces())? {
            if let Some(found) = term_err(Multiplexer::list_tabs(self, &workspace.id))?
                .into_iter()
                .find(|entry| entry.id.as_str() == tab)
            {
                return Ok(found.label);
            }
        }
        Err(AdapterError::unavailable(format!(
            "tmux window {tab:?} not found"
        )))
    }

    fn tab_labels(&self, workspace: &str) -> Result<BTreeMap<String, String>> {
        Ok(
            term_err(Multiplexer::list_tabs(self, &WorkspaceId::from(workspace)))?
                .into_iter()
                .map(|tab| (tab.id.to_string(), tab.label))
                .collect(),
        )
    }

    fn explain_agent(&self, pane: &str) -> Result<(Option<String>, String)> {
        let info = AgentApi::pane_info(self, pane)?;
        Ok((info.agent, info.status))
    }

    fn harness_identity(&self, pane: &str, kind: &str) -> Result<Option<CustomProcessIdentity>> {
        let (state, processes) = pane_state_and_processes(self, pane)?;
        let program = program_for_kind(kind);
        harness_candidate(&state, &processes, kind, program.as_ref())
    }

    fn verify_harness_identity(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
    ) -> Result<bool> {
        verify_foreground_identity(&pane_processes(self, pane)?, expected)
    }

    fn process_command_line(&self, expected: &CustomProcessIdentity) -> Option<Vec<String>> {
        proved_argv(expected)
    }

    fn verify_custom_harness(
        &self,
        pane: &str,
        harness: &str,
        identity: Option<&CustomProcessIdentity>,
    ) -> Result<()> {
        let (state, processes) = pane_state_and_processes(self, pane)?;
        let observed = match identity {
            Some(identity) => verify_foreground_identity(&state, identity)?,
            None => {
                let program = program_for_kind(harness);
                harness_candidate(&state, &processes, harness, program.as_ref())?.is_some()
            }
        };
        if !observed {
            return Err(AdapterError::unavailable(format!(
                "custom harness {harness:?} is not the foreground process in pane {pane}"
            )));
        }
        Ok(())
    }

    fn pane_runs_command(&self, pane: &str, identity: &CustomProcessIdentity) -> Result<bool> {
        verify_foreground_identity(&pane_processes(self, pane)?, identity)
    }

    fn start_pane_command(
        &self,
        pane: &str,
        executable: &Path,
        arguments: &[String],
        timeout: Duration,
    ) -> Result<CustomProcessIdentity> {
        let canonical = fs::canonicalize(executable).map_err(|error| {
            AdapterError::unavailable(format!(
                "cannot resolve pane command {}: {error}",
                executable.display()
            ))
        })?;
        let kind = canonical
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        let program = pin_tmux_program(canonical)?;
        tmux_run_program(self, pane, &kind, &program, arguments, timeout)
    }

    fn pane_is_idle_shell(&self, pane: &str) -> Result<bool> {
        Ok(pane_idle_shell_proof(self, pane)?.is_some())
    }

    fn pane_shell_identity(&self, pane: &str) -> Result<CustomProcessIdentity> {
        let state = pane_processes(self, pane)?;
        supported_shell_process(state.shell_pid)?
            .filter(|observed| observed.process_group_id == state.shell_pid)
            .map(|observed| observed.identity)
            .ok_or_else(|| {
                AdapterError::unavailable(format!(
                    "cannot capture a supported identity-bound shell process for pane {pane}"
                ))
            })
    }

    fn pane_is_same_idle_shell(
        &self,
        pane: &str,
        expected: &CustomProcessIdentity,
    ) -> Result<bool> {
        if !expected.valid() {
            return Err(AdapterError::unavailable(
                "recorded pane shell identity is invalid",
            ));
        }
        Ok(pane_idle_shell_proof(self, pane)?.is_some_and(|proof| proof.identity == *expected))
    }

    fn pane_idle_shell_identity(&self, pane: &str) -> Result<Option<PaneShellProof>> {
        pane_idle_shell_proof(self, pane)
    }

    fn read_screen_with_runtime(&self, pane: &str, _runtime: &dyn AgentRuntime) -> Result<String> {
        term_err(PaneTerminal::read_screen(
            self,
            &PaneId::from(pane),
            ScreenRead {
                scrollback_lines: Some(0),
                ansi: true,
            },
        ))
    }

    fn prompt_guarded(
        &self,
        pane: &str,
        text: &str,
        terminal: &dyn GuardedInput,
        runtime: &dyn AgentRuntime,
    ) -> Result<Submission> {
        let harness = match PaneTerminal::pane_info(self, &PaneId::from(pane)) {
            Ok(info) => info.agent.unwrap_or_default(),
            Err(error) => {
                return Ok(Submission::NotStaged(format!(
                    "pane {pane}: harness lookup failed before typing: {error}"
                )))
            }
        };
        if crate::submission::verifies(&harness, text) {
            return crate::submission::submit_verified(
                &GuardedPrompt(terminal),
                pane,
                &harness,
                text,
                SubmitTimeouts::default(),
                runtime,
            );
        }
        terminal
            .native_prompt(pane, text)
            .map(|()| Submission::Unconfirmed)
    }

    fn submit_guarded(
        &self,
        pane: &str,
        harness: &str,
        text: &str,
        terminal: &dyn GuardedInput,
        runtime: &dyn AgentRuntime,
    ) -> Result<Submission> {
        crate::submission::submit_verified(
            &GuardedPrompt(terminal),
            pane,
            harness,
            text,
            SubmitTimeouts::default(),
            runtime,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use term_backend::{CommandOutput, CommandRunner};

    #[derive(Default)]
    struct Scripted {
        calls: Mutex<Vec<Vec<String>>>,
        responses: Mutex<Vec<String>>,
    }

    impl CommandRunner for Scripted {
        fn run(
            &self,
            _program: &Path,
            args: &[String],
            _stdin: Option<&str>,
        ) -> term_backend::Result<CommandOutput> {
            self.calls.lock().unwrap().push(args.to_vec());
            let stdout = self
                .responses
                .lock()
                .unwrap()
                .drain(..1)
                .next()
                .unwrap_or_default();
            Ok(CommandOutput {
                status: 0,
                stdout,
                stderr: String::new(),
            })
        }
    }

    fn tmux(responses: Vec<&str>) -> TmuxBackend {
        TmuxBackend::with_runner(
            PathBuf::from("tmux"),
            None,
            Arc::new(Scripted {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
            }),
        )
    }

    fn tproc(pid: u64, ppid: u64, pgrp: u64, tpgid: i64) -> TmuxProc {
        TmuxProc {
            pid,
            ppid,
            pgrp,
            tpgid,
            argv: Vec::new(),
        }
    }

    #[test]
    fn pane_processes_scopes_the_foreground_to_the_shells_descendants() {
        // shell(100, tpgid 200) -> wrapper(200) -> harness(201); a
        // background job (300) and another session's group member (400,
        // same numeric group but not a descendant) stay out.
        let processes = vec![
            tproc(100, 1, 100, 200),
            tproc(200, 100, 200, 200),
            tproc(201, 200, 200, 200),
            tproc(300, 100, 300, 200),
            tproc(400, 1, 200, 200),
        ];
        let state = tmux_pane_processes(100, &processes).expect("state");
        assert_eq!(state.foreground_pgid, 200);
        assert_eq!(state.foreground, vec![200, 201]);
        assert!(state.descendants.contains(&300));
        assert!(!state.descendants.contains(&400));
        assert!(tmux_pane_processes(999, &processes).is_none());
    }

    #[test]
    fn choose_candidate_pins_the_launched_process_not_its_same_image_child() {
        let processes = vec![
            tproc(100, 1, 100, 200),
            tproc(200, 100, 200, 200),
            tproc(201, 200, 200, 200),
            tproc(202, 200, 200, 200),
        ];
        assert_eq!(choose_candidate(&[], &processes), None);
        assert_eq!(choose_candidate(&[201], &processes), Some(201));
        // 200 launched 201 and 202: 200 is the pin.
        assert_eq!(choose_candidate(&[202, 200, 201], &processes), Some(200));
        // Two unrelated candidates cannot be pinned.
        assert_eq!(choose_candidate(&[201, 202], &processes), None);
    }

    #[test]
    fn shebang_interpreter_resolves_env_and_direct_paths() {
        assert_eq!(
            shebang_interpreter(b"#!/usr/bin/env python3\nx").as_deref(),
            Some("python3")
        );
        assert_eq!(
            shebang_interpreter(b"#!/opt/node/bin/node --flag\n").as_deref(),
            Some("node")
        );
        assert_eq!(shebang_interpreter(b"\x7fELF..."), None);
        assert_eq!(
            shebang_interpreter(b"#!/usr/bin/env -S node x\n").as_deref(),
            Some("node")
        );
    }

    #[test]
    fn a_pager_holding_the_script_never_matches_the_resolved_program() {
        let directory = std::env::temp_dir().join(format!("agentctl-term-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let script = directory.join("claude");
        std::fs::write(&script, "#!/usr/bin/env python3\n").unwrap();
        let program = pin_tmux_program(script.clone()).unwrap();
        let identity = |pid| agent_queue::identity::LiveProcess {
            identity: agent_queue::identity::ProcessIdentity {
                version: 1,
                pid,
                starttime_ticks: 1,
                boot_id: "boot".to_owned(),
                executable_device: 9,
                executable_inode: 9,
            },
            executable_path: PathBuf::from("/usr/bin/less"),
            process_group_id: pid,
        };
        let pager_argv = vec!["less".to_owned(), script.display().to_string()];
        assert_eq!(
            process_match_class(&identity(10), &pager_argv, Some(&program), "claude"),
            None
        );
        // The shebang interpreter running the same script does match.
        let mut interpreter = identity(11);
        interpreter.executable_path = PathBuf::from("/usr/bin/python3");
        assert_eq!(
            process_match_class(&interpreter, &pager_argv, Some(&program), "claude"),
            Some(MatchClass::Script)
        );
        // With a resolved program there is no argv[0] name fallthrough.
        let spoofed = vec!["claude".to_owned()];
        assert_eq!(
            process_match_class(&identity(12), &spoofed, Some(&program), "claude"),
            None
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn tmux_agent_api_maps_panes_and_infers_unknown_without_a_harness() {
        let backend = tmux(vec![
            "%1\t@1\t$1\t/work\ttitle\t99\tbash\n",
            "%1\t@1\t$1\t/work\ttitle\t99\tbash\n",
        ]);
        let panes = AgentApi::panes(&backend).unwrap();
        assert_eq!(panes[0].pane_id, "%1");
        assert_eq!(panes[0].workspace_id, "$1");
        let info = AgentApi::pane_info(&backend, "%1").unwrap();
        assert_eq!(info.cwd, "/work");
        assert_eq!(info.status, "unknown");
    }

    #[test]
    fn explicit_backend_beats_environment_and_detection() {
        let (kind, explicit) = select_backend(
            Some("tmux"),
            &|_| Some("herdr".to_owned()),
            Path::new("/nonexistent/.agentctl"),
            Path::new("herdr"),
            Path::new("tmux"),
        )
        .unwrap();
        assert_eq!(kind, BackendKind::Tmux);
        assert!(explicit);
    }

    #[test]
    fn environment_backend_beats_detection_and_is_validated() {
        let (kind, explicit) = select_backend(
            None,
            &|name| (name == BACKEND_ENV).then(|| "tmux".to_owned()),
            Path::new("/nonexistent/.agentctl"),
            Path::new("herdr"),
            Path::new("tmux"),
        )
        .unwrap();
        assert_eq!(kind, BackendKind::Tmux);
        assert!(explicit);
        assert!(select_backend(
            Some("screen"),
            &|_| None,
            Path::new("/nonexistent/.agentctl"),
            Path::new("herdr"),
            Path::new("tmux"),
        )
        .is_err());
    }
}
