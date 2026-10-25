//! Terminal-backend selection and the bridges between the
//! backend-neutral multiplexer traits and this crate's own transport traits.
//!
//! Herdr remains the default backend. Its existing client keeps its exact
//! Herdr CLI behaviour; this module only re-expresses it through the
//! backend-neutral multiplexer and pane-terminal traits, and gives the tmux backend an
//! [`AgentApi`] so the durable direct-pane transport (send/drain/status/
//! read against an explicit pane) runs on either multiplexer. Agent
//! lifecycle management (`ManagedApi`: start/adopt/stop and the process
//! proofs behind them) is still Herdr-backed in this step.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use term_backend::{
    AgentStatus, BackendConfig, BackendKind, Capabilities, CreatedPane, Multiplexer, PaneId,
    PaneInfo as BackendPaneInfo, PaneTerminal, ScreenRead, TabId, TabInfo, TermError, TmuxBackend,
    WorkspaceId, WorkspaceInfo,
};

use crate::agent::{AgentApi, AgentRuntime};
use crate::client::{AgentPaneInfo, HerdrClient, Pane};
use crate::error::{AdapterError, Result};
use crate::submission::{PromptTerminal, Submission, SubmitTimeouts};

/// Environment variable selecting the terminal backend when no flag or
/// profile configuration names one.
pub const BACKEND_ENV: &str = "AGENTCTL_TERM_BACKEND";

fn term_err<T>(result: term_backend::Result<T>) -> Result<T> {
    result.map_err(|error| AdapterError::unavailable(error.to_string()))
}

fn client_err<T>(result: Result<T>) -> term_backend::Result<T> {
    result.map_err(|error| TermError::failed(error.to_string()))
}

/// Resolve which terminal backend this invocation uses.
///
/// Precedence: an explicit flag value, then `$AGENTCTL_TERM_BACKEND`, then
/// the project profile configuration beside the registry, then detection
/// (Herdr when its executable is available, else tmux). Detection failure
/// falls back to Herdr so the Herdr client's own "not found" error fires.
pub fn select_backend(
    explicit: Option<&str>,
    environment: &dyn Fn(&str) -> Option<String>,
    registry: &Path,
    herdr_bin: &Path,
    tmux_bin: &Path,
) -> std::result::Result<BackendKind, AdapterError> {
    if let Some(raw) = explicit {
        return BackendKind::parse(raw)
            .map_err(|error| AdapterError::unavailable(error.to_string()));
    }
    if let Some(raw) = environment(BACKEND_ENV).filter(|value| !value.is_empty()) {
        return BackendKind::parse(&raw)
            .map_err(|error| AdapterError::unavailable(format!("${BACKEND_ENV}: {error}")));
    }
    if let Some(kind) = crate::profiles::backend_for_registry(registry)
        .map_err(|error| AdapterError::unavailable(error.to_string()))?
    {
        return Ok(kind);
    }
    let config = BackendConfig {
        kind: None,
        herdr_bin: herdr_bin.to_path_buf(),
        tmux_bin: tmux_bin.to_path_buf(),
        tmux_socket: None,
    };
    Ok(term_backend::detect_kind(&config).unwrap_or(BackendKind::Herdr))
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
        if read.ansi && source == "visible" {
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
    let Some(adapter) = crate::harness_tui::harness_tui(info.agent.as_deref()) else {
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

fn to_agent_pane_info(backend: &TmuxBackend, info: BackendPaneInfo) -> AgentPaneInfo {
    let status = infer_status(backend, &info);
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
        terminal_id: info.terminal_id,
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
        self.run(pane_id, text).map(|()| Submission::Unconfirmed)
    }

    fn wait_agent_status(&self, pane_id: &str, status: &str, timeout_ms: u64) -> Result<()> {
        // No native status exists to wait on; poll the screen-inferred one.
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let info = AgentApi::pane_info(self, pane_id)?;
            if info.status == status {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(AdapterError::unavailable(format!(
                    "wait for pane {pane_id} status {status}: timed out; last inferred status={}",
                    info.status
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
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
        let kind = select_backend(
            Some("tmux"),
            &|_| Some("herdr".to_owned()),
            Path::new("/nonexistent/.agentctl"),
            Path::new("herdr"),
            Path::new("tmux"),
        )
        .unwrap();
        assert_eq!(kind, BackendKind::Tmux);
    }

    #[test]
    fn environment_backend_beats_detection_and_is_validated() {
        let kind = select_backend(
            None,
            &|name| (name == BACKEND_ENV).then(|| "tmux".to_owned()),
            Path::new("/nonexistent/.agentctl"),
            Path::new("herdr"),
            Path::new("tmux"),
        )
        .unwrap();
        assert_eq!(kind, BackendKind::Tmux);
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
