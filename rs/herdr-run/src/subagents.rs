//! Named, long-lived interactive agents sharing a Herdr workspace.
//!
//! Herdr owns terminals and harness processes. This layer owns durable names,
//! launch intent, queue routing, snapshots, and conservative tab teardown.

use std::collections::BTreeMap;
use std::fs::{self, DirBuilder, File};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{self, AgentApi, AgentError, DrainOptions, QueueResult, Target};
use crate::client::{AgentPaneInfo, HerdrClient, Pane};

/// Result of a managed-agent operation, including durable delivery outcomes.
pub type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

fn message_id(value: &str) -> bool {
    value.len() <= 255
        && value
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn name(value: &str) -> Result<&str> {
    if value.is_empty()
        || value.len() > 32
        || value == "archive"
        || !value.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        || !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(fail(
            "agent name must start with a lowercase letter and contain 1-32 lowercase letters, digits or hyphens; 'archive' is reserved",
        ));
    }
    Ok(value)
}

/// Harness arguments for Codex/Claude presets, leaving permission policy untouched.
pub fn harness_arguments(
    harness: &str,
    model: Option<&str>,
    resume: Option<&str>,
    extra: &[String],
) -> Result<Vec<String>> {
    if harness.is_empty()
        || harness.len() > 64
        || !harness.as_bytes()[0].is_ascii_lowercase()
        || !harness
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(fail("harness must be a Herdr agent kind"));
    }
    if extra
        .iter()
        .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err(fail(
            "harness arguments must be nonempty and contain no NUL",
        ));
    }
    let mut arguments = Vec::new();
    match harness {
        "codex" => {
            if let Some(resume) = resume.filter(|value| !value.is_empty()) {
                arguments.extend(["resume".to_owned(), resume.to_owned()]);
            }
            arguments.push("--no-alt-screen".to_owned());
            if let Some(model) = model.filter(|value| !value.is_empty()) {
                arguments.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        "claude" => {
            if let Some(resume) = resume.filter(|value| !value.is_empty()) {
                arguments.extend(["--resume".to_owned(), resume.to_owned()]);
            }
            if let Some(model) = model.filter(|value| !value.is_empty()) {
                arguments.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        _ if model.is_some() || resume.is_some() => return Err(fail(
            "model and resume presets support codex/claude; use harness arguments for other kinds",
        )),
        _ => {}
    }
    if arguments.iter().any(|value| value.contains('\0')) {
        return Err(fail("harness arguments must contain no NUL"));
    }
    arguments.extend_from_slice(extra);
    Ok(arguments)
}

/// Lifecycle operations in addition to the existing interactive messaging API.
pub trait ManagedApi: AgentApi {
    /// Resolve a unique workspace label.
    fn workspace_id_for_label(&self, label: &str) -> crate::error::Result<Option<String>>;
    /// Create a workspace and return its workspace, tab, and pane IDs.
    fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
    ) -> crate::error::Result<(String, String, String)>;
    /// Create a fresh labelled tab without stealing focus.
    fn create_tab(&self, workspace: &str, label: &str, cwd: &str) -> crate::error::Result<String>;
    /// Rename one owned tab.
    fn rename_tab(&self, tab: &str, label: &str) -> crate::error::Result<()>;
    /// Start an interactive harness in a fresh shell pane.
    fn start_agent(
        &self,
        name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
    ) -> crate::error::Result<()>;
    /// Resolve an exact live Herdr agent name.
    fn agent_pane(&self, name: &str) -> crate::error::Result<String>;
    /// Report the explicitly supplied native session identity.
    fn report_agent_session(
        &self,
        name: &str,
        pane: &str,
        kind: &str,
        session: &str,
    ) -> crate::error::Result<()>;
    /// Send one explicit key to the named pane.
    fn send_keys(&self, pane: &str, key: &str) -> crate::error::Result<()>;
    /// Close one owned tab; never close its workspace.
    fn close_tab(&self, tab: &str) -> crate::error::Result<()>;
}

impl ManagedApi for HerdrClient {
    fn agent_pane(&self, name: &str) -> crate::error::Result<String> {
        HerdrClient::agent_pane(self, name)
    }
    fn report_agent_session(
        &self,
        name: &str,
        pane: &str,
        kind: &str,
        session: &str,
    ) -> crate::error::Result<()> {
        HerdrClient::report_agent_session(self, name, pane, kind, session)
    }
    fn send_keys(&self, pane: &str, key: &str) -> crate::error::Result<()> {
        HerdrClient::send_keys(self, pane, key)
    }

    fn workspace_id_for_label(&self, label: &str) -> crate::error::Result<Option<String>> {
        HerdrClient::workspace_id_for_label(self, label)
    }
    fn create_workspace(
        &self,
        label: &str,
        cwd: &str,
    ) -> crate::error::Result<(String, String, String)> {
        HerdrClient::create_workspace(self, label, cwd)
    }
    fn create_tab(&self, workspace: &str, label: &str, cwd: &str) -> crate::error::Result<String> {
        HerdrClient::create_tab(self, workspace, label, cwd)
    }
    fn rename_tab(&self, tab: &str, label: &str) -> crate::error::Result<()> {
        HerdrClient::rename_tab(self, tab, label)
    }
    fn start_agent(
        &self,
        name: &str,
        harness: &str,
        pane: &str,
        args: &[String],
        timeout: Duration,
    ) -> crate::error::Result<()> {
        HerdrClient::start_agent(self, name, harness, pane, args, timeout)
    }
    fn close_tab(&self, tab: &str) -> crate::error::Result<()> {
        HerdrClient::close_tab(self, tab)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AgentRecord {
    name: String,
    token: String,
    harness: String,
    cwd: String,
    created_at: f64,
    schema: u32,
    lifecycle: String,
    workspace_id: Option<String>,
    tab_id: Option<String>,
    pane_id: Option<String>,
    session_agent: Option<String>,
    session_value: Option<String>,
    model: Option<String>,
    resume: Option<String>,
    arguments: Vec<String>,
    error: Option<String>,
    goal: Option<String>,
    goal_delivery: Option<String>,
    goal_session_id: Option<String>,
    goal_command: Option<Vec<String>>,
    #[serde(default)]
    goal_messages: BTreeMap<String, String>,
    goal_message_id: Option<String>,
}

impl AgentRecord {
    fn target(&self) -> Result<Target> {
        if self.pane_id.as_deref().is_none_or(str::is_empty) {
            return Err(fail(format!(
                "agent {:?} has no confirmed pane; inspect its launch error",
                self.name
            )));
        }
        Ok(Target {
            pane_id: self.pane_id.clone(),
            session_agent: self.session_agent.clone(),
            session_value: self.session_value.clone(),
            expected_agent: Some(self.harness.clone()),
            expected_cwd: Some(PathBuf::from(&self.cwd)),
            expected_workspace: None,
        })
    }
}

fn goal_replacement_selected(screen: &str, objective: &str) -> bool {
    let screen = screen.split_whitespace().collect::<Vec<_>>().join(" ");
    let objective = objective.split_whitespace().collect::<Vec<_>>().join(" ");
    screen.contains("Replace goal?") && ["›", "❯"].iter().any(|marker| screen.contains(&format!("New objective: {objective} {marker} 1. Replace current goal Set the new objective and start it now")))
        && screen.contains("2. Cancel Keep the current goal") && screen.contains("Press enter to confirm or esc to go back")
}

struct WorkspaceClient<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    record: &'a AgentRecord,
    goal_objective: Mutex<Option<String>>,
    queue: Option<&'a Path>,
    check_prompt: bool,
}

impl<A: ManagedApi + ?Sized> AgentApi for WorkspaceClient<'_, A> {
    fn panes(&self) -> crate::error::Result<Vec<Pane>> {
        Ok(self
            .client
            .panes()?
            .into_iter()
            .filter(|pane| Some(&pane.workspace_id) == self.record.workspace_id.as_ref())
            .collect())
    }
    fn pane_info(&self, pane_id: &str) -> crate::error::Result<AgentPaneInfo> {
        if Some(self.client.agent_pane(&self.record.name)?) != self.record.pane_id {
            return Err(crate::error::HerdrRunError::unavailable(format!(
                "agent {:?} no longer owns its recorded pane",
                self.record.name
            )));
        }
        let info = self.client.pane_info(pane_id)?;
        if Some(&info.workspace_id) != self.record.workspace_id.as_ref() {
            return Err(crate::error::HerdrRunError::unavailable(format!(
                "agent {:?} workspace identity changed",
                self.record.name
            )));
        }
        if Some(pane_id) == self.record.pane_id.as_deref() {
            if self.record.goal_session_id.is_some()
                && info.session_value.is_some()
                && info.session_value != self.record.goal_session_id
            {
                return Err(crate::error::HerdrRunError::unavailable(format!(
                    "agent {:?} native session identity changed",
                    self.record.name
                )));
            }
            if self.check_prompt
                && matches!(info.status.as_str(), "idle" | "done")
                && info.agent.as_deref() == Some("claude")
            {
                let screen = self.client.read(pane_id, "visible", Some(200))?;
                if screen
                    .contains("Quick safety check: Is this a project you created or one you trust?")
                    && screen.contains("No, exit")
                    && screen.contains("Yes, I trust this folder")
                {
                    return Err(crate::error::HerdrRunError::unavailable("Claude workspace trust prompt requires human attention; no input was submitted"));
                }
            }
        }
        Ok(info)
    }
    fn workspace_label(&self, workspace_id: &str) -> crate::error::Result<String> {
        self.client.workspace_label(workspace_id)
    }
    fn run(&self, pane_id: &str, text: &str) -> crate::error::Result<()> {
        *self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned") = None;
        if let Some(queue) = self.queue {
            for (identifier, objective) in &self.record.goal_messages {
                let path = queue.join("inflight").join(format!("{identifier}.json"));
                if text == format!("/goal {objective}") && fs::symlink_metadata(&path).is_ok() {
                    let document = agent::read_private_json(&path).map_err(|error| {
                        crate::error::HerdrRunError::unavailable(error.to_string())
                    })?;
                    if document["text"].as_str() == Some(text) {
                        *self
                            .goal_objective
                            .lock()
                            .expect("goal operation lock poisoned") = Some(objective.clone());
                        break;
                    }
                }
            }
        }
        self.client.run(pane_id, text)
    }
    fn wait_agent_status(
        &self,
        pane_id: &str,
        status: &str,
        timeout_ms: u64,
    ) -> crate::error::Result<()> {
        let Some(objective) = self
            .goal_objective
            .lock()
            .expect("goal operation lock poisoned")
            .clone()
            .filter(|_| status == "working")
        else {
            return self.client.wait_agent_status(pane_id, status, timeout_ms);
        };
        let start = Instant::now();
        if self
            .client
            .wait_agent_status(pane_id, status, timeout_ms.min(1000))
            .is_ok()
        {
            return Ok(());
        }
        self.pane_info(pane_id)?;
        let screen = self.client.read(pane_id, "visible", Some(200))?;
        if goal_replacement_selected(&screen, &objective) {
            self.client.send_keys(pane_id, "Enter")?;
        }
        let elapsed = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.client
            .wait_agent_status(pane_id, status, timeout_ms.saturating_sub(elapsed).max(1))
    }
    fn read(
        &self,
        pane_id: &str,
        source: &str,
        lines: Option<usize>,
    ) -> crate::error::Result<String> {
        self.client.read(pane_id, source, lines)
    }
}

/// Options for a new visible subagent; the working directory is always explicit.
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Existing workspace ID, or the current workspace / shared `subagents` default.
    pub workspace_id: Option<String>,
    /// Herdr agent kind.
    pub harness: String,
    /// Optional model override for Codex or Claude.
    pub model: Option<String>,
    /// Optional saved conversation to resume.
    pub resume: Option<String>,
    /// Additional literal harness arguments.
    pub harness_args: Vec<String>,
    /// Initial task to submit after the harness is ready.
    pub brief: Option<String>,
    /// Herdr's bounded startup-readiness deadline.
    pub startup_timeout: Duration,
    /// Delivery options for the initial task.
    pub delivery: DrainOptions,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self {
            workspace_id: None,
            harness: "codex".to_owned(),
            model: None,
            resume: None,
            harness_args: Vec::new(),
            brief: None,
            startup_timeout: Duration::from_secs(30),
            delivery: DrainOptions::default(),
        }
    }
}

/// Registry-backed coordinator interface to visible foreign-harness workers.
pub struct ManagedAgents<'a, A: ManagedApi + ?Sized> {
    client: &'a A,
    registry: PathBuf,
}

impl<'a, A: ManagedApi + ?Sized> ManagedAgents<'a, A> {
    /// Use an explicit registry, independent of the agents' working directories.
    pub fn new(client: &'a A, registry: &Path) -> Result<Self> {
        let registry = if registry.is_absolute() {
            registry.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| fail(error.to_string()))?
                .join(registry)
        };
        Ok(Self { client, registry })
    }

    fn directory(&self, agent_name: &str) -> Result<PathBuf> {
        Ok(self.registry.join(name(agent_name)?))
    }

    fn lock(&self, agent_name: &str) -> Result<File> {
        name(agent_name)?;
        agent::create_private_directory(&self.registry, "agent registry", true, true)?;
        let file = agent::open_private_lock(
            &self.registry.join(format!(".{agent_name}.lock")),
            "agent lifecycle lock",
        )?;
        file.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        Ok(file)
    }

    fn load(&self, agent_name: &str) -> Result<AgentRecord> {
        let directory = self.directory(agent_name)?;
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        if !directory.exists() {
            return Err(fail(format!(
                "unknown agent {agent_name:?}; use list to inspect the registry"
            )));
        }
        agent::validate_private_directory(&directory, "agent directory", false)?;
        let path = directory.join("agent.json");
        let record: AgentRecord = serde_json::from_value(agent::read_private_json(&path)?)
            .map_err(|error| fail(format!("invalid agent record {}: {error}", path.display())))?;
        if record.name != agent_name
            || record.schema != 1
            || record.token.is_empty()
            || record.token.len() > 80
            || !record
                .token
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || record.harness.is_empty()
            || record.cwd.is_empty()
            || record.lifecycle.is_empty()
            || !record.created_at.is_finite()
            || record
                .goal_message_id
                .as_deref()
                .is_some_and(|value| !message_id(value))
            || record
                .goal_messages
                .iter()
                .any(|(key, value)| !message_id(key) || value.is_empty())
            || record.goal_command.as_ref().is_some_and(|command| {
                command.is_empty()
                    || command
                        .iter()
                        .any(|value| value.is_empty() || value.contains('\0'))
            })
        {
            return Err(fail(format!("invalid agent record: {}", path.display())));
        }
        Ok(record)
    }

    fn save(&self, record: &AgentRecord) -> Result<()> {
        agent::atomic_json(
            &self.directory(&record.name)?.join("agent.json"),
            &json!(record),
        )
    }

    fn queue(&self, agent_name: &str) -> Result<PathBuf> {
        Ok(self.directory(agent_name)?.join("queue"))
    }

    /// Read durable metadata even if Herdr is unreachable.
    pub fn get(&self, agent_name: &str) -> Result<Value> {
        Ok(json!(self.load(agent_name)?))
    }

    /// Start one fresh tab and retain failed launch artifacts for diagnosis.
    pub fn start(&self, agent_name: &str, cwd: &Path, options: StartOptions) -> Result<Value> {
        name(agent_name)?;
        let cwd = fs::canonicalize(cwd)
            .map_err(|_| fail(format!("cwd is not a directory: {}", cwd.display())))?;
        if !cwd.is_dir() {
            return Err(fail(format!("cwd is not a directory: {}", cwd.display())));
        }
        if options.startup_timeout.is_zero() || options.startup_timeout > Duration::from_secs(300) {
            return Err(fail("startup timeout must be between 0 and 300 seconds"));
        }
        if options.brief.as_deref().is_some_and(str::is_empty) {
            return Err(fail("brief must not be empty"));
        }
        let arguments = harness_arguments(
            &options.harness,
            options.model.as_deref(),
            options.resume.as_deref(),
            &options.harness_args,
        )?;
        let lock = self.lock(agent_name)?;
        let directory = self.directory(agent_name)?;
        if fs::symlink_metadata(&directory).is_ok() {
            return Err(fail(format!(
                "agent {agent_name:?} already registered; stop it before reusing the name"
            )));
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&self.registry)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?;
        let mut record = AgentRecord {
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            harness: options.harness.clone(),
            cwd: cwd.display().to_string(),
            created_at: now.as_secs_f64(),
            schema: 1,
            lifecycle: "starting".to_owned(),
            workspace_id: None,
            tab_id: None,
            pane_id: None,
            session_agent: None,
            session_value: None,
            model: options.model.clone(),
            resume: options.resume.clone(),
            arguments,
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
            goal_message_id: None,
        };
        self.save(&record)?;
        let launched = self.launch(&mut record, &options);
        if let Err(error) = launched {
            record.lifecycle = "launch_failed".to_owned();
            record.error = Some(error.to_string());
            self.save(&record)?;
            return Err(fail(format!("launch of {agent_name:?} failed: {error}; record and any created tab retained at {}", directory.display())));
        }
        drop(lock);
        if let Some(brief) = options.brief {
            self.send(agent_name, &brief, options.delivery)?;
        }
        self.status(agent_name)
    }

    fn launch(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        self.create_presentation(record, options)?;
        let pane_id = record.pane_id.as_deref().expect("new tab has pane");
        self.client.start_agent(
            &record.name,
            &record.harness,
            pane_id,
            &record.arguments,
            options.startup_timeout,
        )?;
        let info = agent::resolve_target(
            &WorkspaceClient {
                client: self.client,
                record,
                goal_objective: Mutex::new(None),
                queue: None,
                check_prompt: true,
            },
            &record.target()?,
        )?;
        record.session_agent = info.session_agent;
        record.session_value = info.session_value;
        record.lifecycle = "running".to_owned();
        self.save(record)
    }

    fn create_presentation(&self, record: &mut AgentRecord, options: &StartOptions) -> Result<()> {
        let lock = agent::open_private_lock(
            &agent::target_lock_path("managed-workspace:subagents")?,
            "workspace allocation lock",
        )?;
        lock.lock_exclusive()
            .map_err(|error| fail(error.to_string()))?;
        let selected = options
            .workspace_id
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("HERDR_WORKSPACE_ID")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
        let selected = if let Some(workspace) = selected {
            self.client.workspace_label(&workspace)?;
            Some(workspace)
        } else {
            self.client.workspace_id_for_label("subagents")?
        };
        match selected {
            Some(workspace) => {
                record.workspace_id = Some(workspace.clone());
                record.tab_id = Some(self.client.create_tab(
                    &workspace,
                    &record.name,
                    &record.cwd,
                )?);
                self.save(record)?;
                let panes: Vec<Pane> = self
                    .client
                    .panes()?
                    .into_iter()
                    .filter(|pane| {
                        Some(&pane.tab_id) == record.tab_id.as_ref()
                            && pane.workspace_id == workspace
                    })
                    .collect();
                if panes.len() != 1 {
                    return Err(fail("new agent tab did not contain exactly one shell pane"));
                }
                record.pane_id = Some(panes[0].pane_id.clone());
                self.save(record)?;
            }
            None => {
                let (workspace, tab, pane) =
                    self.client.create_workspace("subagents", &record.cwd)?;
                record.workspace_id = Some(workspace);
                record.tab_id = Some(tab.clone());
                record.pane_id = Some(pane);
                self.save(record)?;
                self.client.rename_tab(&tab, &record.name)?;
            }
        }
        Ok(())
    }

    fn checked(&self, record: &AgentRecord) -> Result<AgentPaneInfo> {
        agent::resolve_target(
            &WorkspaceClient {
                client: self.client,
                record,
                goal_objective: Mutex::new(None),
                queue: None,
                check_prompt: false,
            },
            &record.target()?,
        )
    }

    /// Report live state or an explicit probe error without reaping durable records.
    pub fn status(&self, agent_name: &str) -> Result<Value> {
        let record = self.load(agent_name)?;
        let mut result = json!(record);
        result["queue"] = json!(self.queue(agent_name)?);
        result["output"] = json!(self.directory(agent_name)?.join("output.json"));
        result["goal_source"] = if record.goal.is_some() {
            json!("requested")
        } else {
            Value::Null
        };
        result["goal_delivery"] = json!(self.goal_delivery(&record));
        let probe = self
            .checked(&record)
            .and_then(|_| agent::status(self.client, &record.target()?, &self.queue(agent_name)?));
        match probe {
            Ok(status) => {
                result
                    .as_object_mut()
                    .expect("record object")
                    .extend(json!(status).as_object().expect("status object").clone());
                result["probe_error"] = Value::Null;
            }
            Err(error) => {
                result["agent_status"] = json!("unknown");
                result["probe_error"] = json!(error.to_string());
            }
        }
        Ok(result)
    }

    /// List every registered agent, preserving records when Herdr is unavailable.
    pub fn list(&self) -> Result<Vec<Value>> {
        if !self.registry.exists() {
            return Ok(Vec::new());
        }
        agent::validate_private_directory(&self.registry, "agent registry", false)?;
        let mut names = fs::read_dir(&self.registry)
            .map_err(|error| fail(error.to_string()))?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| fail(error.to_string()))?;
        names.retain(|value| name(value).is_ok());
        names.sort();
        names.iter().map(|name| self.status(name)).collect()
    }

    /// Serialize against stop and durably deliver one prompt.
    pub fn send(&self, agent_name: &str, text: &str, options: DrainOptions) -> Result<QueueResult> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        agent::send(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                queue: Some(&self.queue(agent_name)?),
                check_prompt: true,
            },
            &record.target()?,
            &self.queue(agent_name)?,
            text,
            options,
        )
    }

    /// Drain only prompts known not to have been injected.
    pub fn drain(&self, agent_name: &str, options: DrainOptions) -> Result<QueueResult> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        agent::drain(
            &WorkspaceClient {
                client: self.client,
                record: &record,
                goal_objective: Mutex::new(None),
                queue: Some(&self.queue(agent_name)?),
                check_prompt: true,
            },
            &record.target()?,
            &self.queue(agent_name)?,
            options,
        )
    }

    fn snapshot(&self, record: &AgentRecord, text: &str) -> Result<()> {
        let captured = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_secs_f64();
        agent::atomic_json(
            &self.directory(&record.name)?.join("output.json"),
            &json!({"text":text,"captured_at":captured,"pane_id":record.pane_id}),
        )
    }

    /// Read the shared terminal and persist the latest bounded snapshot.
    pub fn read(&self, agent_name: &str, lines: usize) -> Result<String> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.checked(&record)?;
        let text = agent::read(self.client, &record.target()?, lines)?;
        self.snapshot(&record, &text)?;
        Ok(text)
    }

    /// Wait for readiness, which is separate from completion of the agent's goal.
    pub fn wait(&self, agent_name: &str, timeout: Duration) -> Result<Value> {
        if timeout > Duration::from_secs(31_536_000) {
            return Err(fail(
                "wait timeout must be finite and between 0 and 31536000 seconds",
            ));
        }
        let token = self.load(agent_name)?.token;
        let start = Instant::now();
        loop {
            let record = self.load(agent_name)?;
            if record.token != token {
                return Err(fail(format!(
                    "agent {agent_name:?} was replaced while waiting"
                )));
            }
            let info = self.checked(&record)?;
            if matches!(info.status.as_str(), "idle" | "done") {
                return self.status(agent_name);
            }
            if !matches!(info.status.as_str(), "working" | "starting" | "unknown") {
                return Err(fail(format!(
                    "agent {agent_name:?} requires attention (state {:?}); read its pane",
                    info.status
                )));
            }
            if start.elapsed() >= timeout {
                return Err(fail(format!(
                    "timed out waiting for agent {agent_name:?} (state {:?})",
                    info.status
                )));
            }
            std::thread::sleep(
                Duration::from_millis(250).min(timeout.saturating_sub(start.elapsed())),
            );
        }
    }

    /// Bind an explicitly known native session without changing the existing queue identity.
    pub fn bind_session(
        &self,
        agent_name: &str,
        session_id: &str,
        goal_command: Option<&[String]>,
    ) -> Result<Value> {
        if session_id.is_empty() || session_id.contains('\0') {
            return Err(fail(
                "native session id must be nonempty and contain no NUL",
            ));
        }
        if goal_command.is_some_and(|command| {
            command.is_empty()
                || command
                    .iter()
                    .any(|item| item.is_empty() || item.contains('\0'))
        }) {
            return Err(fail("goal command must be a nonempty argument vector"));
        }
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        let info = self.checked(&record)?;
        for existing in [
            &record.goal_session_id,
            &record.session_value,
            &info.session_value,
        ]
        .into_iter()
        .flatten()
        {
            if existing != session_id {
                return Err(fail("refusing to replace an already bound native session"));
            }
        }
        // Session metadata is optional in Herdr. This is an explicit caller
        // assertion anchored to the independently verified live name and pane.
        record.goal_session_id = Some(session_id.to_owned());
        if let Some(command) = goal_command {
            record.goal_command = Some(command.to_vec());
        }
        self.save(&record)?;
        Ok(json!({"name":agent_name,"session_id":session_id,"source":"explicit"}))
    }

    fn goal_delivery(&self, record: &AgentRecord) -> Option<String> {
        if let (Some(identifier), Ok(queue)) = (&record.goal_message_id, self.queue(&record.name)) {
            for (folder, outcome) in [
                ("processed", "delivered"),
                ("failed", "possibly_submitted"),
                ("inflight", "possibly_submitted"),
                ("inbox", "pending"),
            ] {
                if fs::symlink_metadata(queue.join(folder).join(format!("{identifier}.json")))
                    .is_ok()
                {
                    return Some(outcome.to_owned());
                }
            }
        }
        record.goal_delivery.clone()
    }

    fn goal_result(&self, record: &AgentRecord, command: Option<&[String]>) -> Value {
        let mut result = json!({"name":record.name,"goal":record.goal,"delivery":self.goal_delivery(record),"source":"requested","native_status":"unverified"});
        if record.harness != "codex" {
            return result;
        }
        let Some(session) = record
            .goal_session_id
            .as_deref()
            .or(record.session_value.as_deref())
            .filter(|s| !s.is_empty())
        else {
            result["native_error"] = json!(
                "native session unknown; bind-session with the session id reported by this agent"
            );
            return result;
        };
        let default = [
            "codex".to_owned(),
            "app-server".to_owned(),
            "proxy".to_owned(),
        ];
        let command = command
            .or(record.goal_command.as_deref())
            .unwrap_or(&default);
        match crate::codex_goal::get_goal(session, command, Duration::from_secs(30)) {
            Ok(native) => {
                result["source"] = json!("native");
                result["native_status"] = native
                    .as_ref()
                    .map_or(json!("absent"), |goal| goal["status"].clone());
                result["goal"] = native
                    .as_ref()
                    .map_or(Value::Null, |goal| goal["objective"].clone());
                result["native"] = json!(native);
            }
            Err(error) => result["native_error"] = json!(error),
        }
        result
    }

    /// Inspect a native Codex goal when bound, or submit a visible goal and wake its harness.
    pub fn goal(
        &self,
        agent_name: &str,
        text: Option<&str>,
        options: DrainOptions,
        goal_command: Option<&[String]>,
    ) -> Result<Value> {
        let Some(text) = text else {
            let record = self.load(agent_name)?;
            self.checked(&record)?;
            return Ok(self.goal_result(&record, goal_command));
        };
        if text.trim().is_empty() || text.contains(['\n', '\r']) {
            return Err(fail("goal must be a nonempty single line"));
        }
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        record.goal = Some(text.to_owned());
        record.goal_delivery = Some("pending".to_owned());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| fail(error.to_string()))?
            .as_nanos();
        let identifier = format!("{timestamp:020}-{}", std::process::id());
        record.goal_message_id = Some(identifier.clone());
        if record.harness == "codex" {
            record
                .goal_messages
                .insert(identifier.clone(), text.to_owned());
        }
        self.save(&record)?;
        let prompt = if record.harness == "codex" {
            format!("/goal {text}")
        } else {
            format!("Your ongoing goal: {text}\nWork toward this goal and report completion or blockers.")
        };
        let queue = self.queue(agent_name)?;
        let client = WorkspaceClient {
            client: self.client,
            record: &record,
            goal_objective: Mutex::new(None),
            queue: Some(&queue),
            check_prompt: true,
        };
        let outcome = agent::send_identified(
            &client,
            &record.target()?,
            &queue,
            &prompt,
            &identifier,
            options,
        );
        match outcome {
            Ok(delivered) => {
                record.goal_delivery = Some(delivered.outcome.as_str().to_owned());
                self.save(&record)?;
                Ok(self.goal_result(&record, goal_command))
            }
            Err(error) => {
                record.goal_delivery = Some(
                    error
                        .outcome()
                        .map_or("failed", |outcome| outcome.as_str())
                        .to_owned(),
                );
                self.save(&record)?;
                Err(error)
            }
        }
    }

    /// Close only the owned single-pane tab and archive its metadata, queue, and output.
    pub fn stop(&self, agent_name: &str) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let mut record = self.load(agent_name)?;
        let panes = self.client.panes()?;
        if panes.iter().any(|pane| {
            Some(&pane.pane_id) == record.pane_id.as_ref()
                && Some(&pane.tab_id) != record.tab_id.as_ref()
        }) {
            return Err(fail(
                "refusing to archive an agent whose pane moved to another tab",
            ));
        }
        let owned: Vec<Pane> = panes
            .into_iter()
            .filter(|pane| Some(&pane.tab_id) == record.tab_id.as_ref())
            .collect();
        if !owned.is_empty() {
            if owned.len() != 1
                || Some(&owned[0].pane_id) != record.pane_id.as_ref()
                || Some(&owned[0].workspace_id) != record.workspace_id.as_ref()
            {
                return Err(fail("refusing to close a tab whose pane ownership changed"));
            }
            if record.lifecycle == "running"
                || self.client.pane_info(&owned[0].pane_id)?.agent.is_some()
            {
                self.checked(&record)?;
            }
            let pane_id = &owned[0].pane_id;
            let mut text = self.client.read(pane_id, "recent-unwrapped", Some(5000))?;
            if text.is_empty() {
                text = self.client.read(pane_id, "recent", Some(5000))?;
            }
            self.snapshot(&record, &text)?;
            record.lifecycle = "stopping".to_owned();
            self.save(&record)?;
            self.client
                .close_tab(record.tab_id.as_deref().expect("owned tab present"))?;
        }
        record.lifecycle = "stopped".to_owned();
        self.save(&record)?;
        let archive = self.registry.join("archive");
        agent::create_private_directory(&archive, "agent archive", false, false)?;
        let destination = archive.join(format!("{agent_name}-{}", record.token));
        fs::rename(self.directory(agent_name)?, &destination)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&archive)?;
        agent::sync_directory(&self.registry)?;
        Ok(json!({"name":agent_name,"archive":destination,"tab_closed":!owned.is_empty()}))
    }
}
