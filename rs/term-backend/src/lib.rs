//! Backend-neutral terminal-multiplexer traits with Herdr and tmux backends.
//!
//! Callers that only need a pane to read and type into should depend on
//! [`Multiplexer`] and [`PaneTerminal`], never on a concrete multiplexer's
//! CLI, JSON shapes, or identifier grammar. [`HerdrBackend`] and
//! [`TmuxBackend`] implement the traits; [`FakeBackend`] is an in-memory
//! implementation for tests; [`open_backend`] selects one from a
//! [`BackendConfig`].

use std::fmt;
use std::path::{Path, PathBuf};

mod fake;
mod herdr;
mod runner;
mod tmux;

pub use fake::FakeBackend;
pub use herdr::HerdrBackend;
pub use runner::{CommandOutput, CommandRunner, SystemRunner};
pub use tmux::TmuxBackend;

/// Result type used by every backend operation.
pub type Result<T> = std::result::Result<T, TermError>;

/// One backend failure, classified so callers can tell "not there" apart
/// from "this backend cannot do that".
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TermError {
    /// The backend executable or server is unavailable.
    Unavailable(String),
    /// The named workspace, tab, or pane does not exist.
    NotFound(String),
    /// The backend has no such operation.
    Unsupported(String),
    /// The backend ran and refused, or returned an unusable response.
    Failed(String),
}

impl TermError {
    /// Build an [`TermError::Unavailable`] from any displayable detail.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::Unavailable(detail.into())
    }
    /// Build an [`TermError::Failed`] from any displayable detail.
    pub fn failed(detail: impl Into<String>) -> Self {
        Self::Failed(detail.into())
    }
    /// Build an [`TermError::Unsupported`] from any displayable detail.
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::Unsupported(detail.into())
    }
}

impl fmt::Display for TermError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(detail)
            | Self::NotFound(detail)
            | Self::Unsupported(detail)
            | Self::Failed(detail) => formatter.write_str(detail),
        }
    }
}

impl std::error::Error for TermError {}

impl From<std::io::Error> for TermError {
    fn from(error: std::io::Error) -> Self {
        Self::Unavailable(error.to_string())
    }
}

macro_rules! opaque_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
        pub struct $name(pub String);

        impl $name {
            /// The raw backend spelling, uninterpreted.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

opaque_id!(WorkspaceId, "Opaque workspace (tmux session) identifier.");
opaque_id!(TabId, "Opaque tab (tmux window) identifier.");
opaque_id!(PaneId, "Opaque pane identifier.");

/// Native per-agent status, when the backend tracks one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentStatus {
    /// The agent is waiting for input.
    Idle,
    /// The agent is mid-turn.
    Working,
    /// The agent finished its turn.
    Done,
    /// The agent is waiting on a human decision.
    Blocked,
    /// The backend reports no status, or an unreadable one.
    Unknown,
    /// Any other backend-specific status string, preserved verbatim.
    Other(String),
}

impl AgentStatus {
    /// Parse the wire spelling used by Herdr's `agent_status` field.
    pub fn parse(raw: &str) -> Self {
        match raw {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "unknown" | "" => Self::Unknown,
            other => Self::Other(other.to_owned()),
        }
    }

    /// The wire spelling of this status.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
            Self::Other(raw) => raw,
        }
    }
}

/// One workspace (tmux session).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceInfo {
    /// Backend workspace identifier.
    pub id: WorkspaceId,
    /// Human-facing workspace label.
    pub label: String,
}

/// One tab (tmux window) inside a workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabInfo {
    /// Backend tab identifier.
    pub id: TabId,
    /// Owning workspace.
    pub workspace_id: WorkspaceId,
    /// Human-facing tab label.
    pub label: String,
}

/// Identity and live state of one pane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneInfo {
    /// Backend pane identifier.
    pub id: PaneId,
    /// Owning tab, when the backend reports it.
    pub tab_id: Option<TabId>,
    /// Owning workspace, when the backend reports it.
    pub workspace_id: Option<WorkspaceId>,
    /// Pane working directory, when reported.
    pub cwd: Option<PathBuf>,
    /// Pane title, when reported.
    pub title: Option<String>,
    /// Recognised interactive-agent kind, when the backend detects one.
    pub agent: Option<String>,
    /// Native agent status; `None` when the backend tracks none (tmux).
    pub agent_status: Option<AgentStatus>,
    /// Foreground/shell process id of the pane, when reported.
    pub pid: Option<u32>,
    /// Backend terminal identity distinct from the pane, when reported.
    pub terminal_id: Option<String>,
}

/// The three identifiers a workspace or tab creation allocates together.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatedPane {
    /// Workspace holding the new tab.
    pub workspace_id: WorkspaceId,
    /// New tab.
    pub tab_id: TabId,
    /// Initial pane of the new tab.
    pub pane_id: PaneId,
}

/// What one screen read should return.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenRead {
    /// Scrollback lines to include before the visible screen; `None` reads
    /// the backend's recent output, `Some(0)` the visible screen only.
    pub scrollback_lines: Option<usize>,
    /// Keep SGR escape sequences in the returned text.
    pub ansi: bool,
}

impl ScreenRead {
    /// The visible screen only, plain text.
    pub fn visible() -> Self {
        Self {
            scrollback_lines: Some(0),
            ansi: false,
        }
    }
    /// Recent output including up to `lines` of scrollback, plain text.
    pub fn recent(lines: usize) -> Self {
        Self {
            scrollback_lines: Some(lines),
            ansi: false,
        }
    }
}

/// Optional backend abilities a caller may branch on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    /// The backend tracks a native per-agent status (Herdr yes, tmux no).
    pub native_agent_status: bool,
    /// The backend has a native prompt primitive beyond paste + Enter.
    pub native_prompt: bool,
}

/// The container half of a terminal multiplexer: workspaces, tabs, panes.
pub trait Multiplexer: Send + Sync {
    /// Stable backend name: `"herdr"`, `"tmux"`, or `"fake"`.
    fn backend_name(&self) -> &'static str;
    /// Optional abilities of this backend.
    fn capabilities(&self) -> Capabilities;
    /// Every live workspace.
    fn list_workspaces(&self) -> Result<Vec<WorkspaceInfo>>;
    /// Create a workspace with one tab and pane; `environment` entries are
    /// `NAME=value` strings for the initial pane.
    fn create_workspace(
        &self,
        label: &str,
        cwd: &Path,
        environment: &[String],
    ) -> Result<CreatedPane>;
    /// Bring one workspace to the front.
    fn focus_workspace(&self, workspace: &WorkspaceId) -> Result<()>;
    /// Rename one workspace.
    fn rename_workspace(&self, workspace: &WorkspaceId, label: &str) -> Result<()>;
    /// Close one workspace and everything in it.
    fn close_workspace(&self, workspace: &WorkspaceId) -> Result<()>;
    /// Every tab of one workspace.
    fn list_tabs(&self, workspace: &WorkspaceId) -> Result<Vec<TabInfo>>;
    /// Create a tab with one pane in an existing workspace.
    fn create_tab(
        &self,
        workspace: &WorkspaceId,
        label: &str,
        cwd: &Path,
        environment: &[String],
    ) -> Result<CreatedPane>;
    /// Bring one tab to the front.
    fn focus_tab(&self, tab: &TabId) -> Result<()>;
    /// Rename one tab.
    fn rename_tab(&self, tab: &TabId, label: &str) -> Result<()>;
    /// Close one tab and its panes.
    fn close_tab(&self, tab: &TabId) -> Result<()>;
    /// Every pane, optionally restricted to one workspace.
    fn list_panes(&self, workspace: Option<&WorkspaceId>) -> Result<Vec<PaneInfo>>;
    /// Bring one pane to the front.
    fn focus_pane(&self, pane: &PaneId) -> Result<()>;
    /// Close exactly one pane.
    fn close_pane(&self, pane: &PaneId) -> Result<()>;
}

/// The surface of one pane: reading its screen and typing into it.
///
/// This is the same seam a verified-submission layer already
/// drives through its `PromptTerminal`/`GuardedInput` traits; a
/// `PaneTerminal` supplies the primitives those traits guard.
pub trait PaneTerminal: Send + Sync {
    /// Live identity and state of one pane.
    fn pane_info(&self, pane: &PaneId) -> Result<PaneInfo>;
    /// Read the pane's screen per `read`.
    fn read_screen(&self, pane: &PaneId, read: ScreenRead) -> Result<String>;
    /// Insert literal text without any submission key (bracketed paste
    /// where the backend supports it).
    fn paste_text(&self, pane: &PaneId, text: &str) -> Result<()>;
    /// Send one named key, using the backend's key spelling
    /// (`Enter`, `Escape`, `C-c`, …).
    fn send_keys(&self, pane: &PaneId, keys: &str) -> Result<()>;
    /// Type `text` and submit it. The default pastes then sends Enter;
    /// backends with a native prompt primitive override it.
    fn prompt(&self, pane: &PaneId, text: &str) -> Result<()> {
        self.paste_text(pane, text)?;
        self.send_keys(pane, "Enter")
    }
    /// Wait until the pane's agent reaches `status`. Only backends with
    /// [`Capabilities::native_agent_status`] support this.
    fn wait_agent_status(
        &self,
        pane: &PaneId,
        status: &AgentStatus,
        timeout_ms: u64,
    ) -> Result<()> {
        let _ = (pane, status, timeout_ms);
        Err(TermError::unsupported(
            "this backend tracks no native agent status; infer readiness from the screen",
        ))
    }
}

/// A complete terminal backend: containers plus pane surfaces.
pub trait TermBackend: Multiplexer + PaneTerminal {}

impl<T: Multiplexer + PaneTerminal> TermBackend for T {}

/// Which concrete multiplexer a [`BackendConfig`] selects.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendKind {
    /// The Herdr terminal workspace manager.
    Herdr,
    /// tmux.
    Tmux,
}

impl BackendKind {
    /// Stable config spelling: `"herdr"` or `"tmux"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Herdr => "herdr",
            Self::Tmux => "tmux",
        }
    }
    /// Parse a config spelling, case-insensitively.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.to_ascii_lowercase().as_str() {
            "herdr" => Ok(Self::Herdr),
            "tmux" => Ok(Self::Tmux),
            other => Err(TermError::failed(format!(
                "unknown terminal backend {other:?}; expected \"herdr\" or \"tmux\""
            ))),
        }
    }
}

/// Backend selection and executable overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendConfig {
    /// Explicit backend; `None` detects (Herdr first, then tmux).
    pub kind: Option<BackendKind>,
    /// Herdr executable name or path.
    pub herdr_bin: PathBuf,
    /// tmux executable name or path.
    pub tmux_bin: PathBuf,
    /// tmux server socket name (`-L`), when not the default server.
    pub tmux_socket: Option<String>,
}

impl Default for BackendConfig {
    fn default() -> Self {
        Self {
            kind: None,
            herdr_bin: PathBuf::from("herdr"),
            tmux_bin: PathBuf::from("tmux"),
            tmux_socket: None,
        }
    }
}

/// Whether `program` resolves: directly when it names a path, else on `PATH`.
pub fn executable_available(program: &Path) -> bool {
    if program.components().count() > 1 || program.is_absolute() {
        return is_executable_file(program);
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| is_executable_file(&directory.join(program)))
}

fn is_executable_file(candidate: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        candidate.is_file()
    }
}

/// Pick a backend when the config names none: Herdr when its executable is
/// available, else tmux when only tmux exists.
pub fn detect_kind(config: &BackendConfig) -> Result<BackendKind> {
    if let Some(kind) = config.kind {
        return Ok(kind);
    }
    if executable_available(&config.herdr_bin) {
        return Ok(BackendKind::Herdr);
    }
    if executable_available(&config.tmux_bin) {
        return Ok(BackendKind::Tmux);
    }
    Err(TermError::unavailable(
        "no terminal backend found: neither herdr nor tmux is on PATH",
    ))
}

/// Open the backend a config selects.
pub fn open_backend(config: &BackendConfig) -> Result<Box<dyn TermBackend>> {
    match detect_kind(config)? {
        BackendKind::Herdr => Ok(Box::new(HerdrBackend::new(config.herdr_bin.clone()))),
        BackendKind::Tmux => Ok(Box::new(TmuxBackend::new(
            config.tmux_bin.clone(),
            config.tmux_socket.clone(),
        ))),
    }
}

/// Arguments as owned strings, the shape both CLI backends build.
pub(crate) fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_kind_round_trips_and_refuses_unknown() {
        assert_eq!(BackendKind::parse("herdr").unwrap(), BackendKind::Herdr);
        assert_eq!(BackendKind::parse("TMUX").unwrap(), BackendKind::Tmux);
        assert!(BackendKind::parse("screen").is_err());
    }

    #[test]
    fn agent_status_round_trips_the_wire_spelling() {
        for raw in ["idle", "working", "done", "blocked", "unknown", "waiting"] {
            assert_eq!(AgentStatus::parse(raw).as_str(), raw);
        }
        assert_eq!(AgentStatus::parse(""), AgentStatus::Unknown);
    }

    #[test]
    fn explicit_kind_wins_over_detection() {
        let config = BackendConfig {
            kind: Some(BackendKind::Tmux),
            ..BackendConfig::default()
        };
        assert_eq!(detect_kind(&config).unwrap(), BackendKind::Tmux);
    }

    #[test]
    fn detection_reports_when_neither_backend_exists() {
        let config = BackendConfig {
            kind: None,
            herdr_bin: PathBuf::from("/nonexistent/herdr"),
            tmux_bin: PathBuf::from("/nonexistent/tmux"),
            tmux_socket: None,
        };
        assert!(matches!(
            detect_kind(&config),
            Err(TermError::Unavailable(_))
        ));
    }
}
