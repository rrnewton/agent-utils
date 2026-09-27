//! Agentcloud sessions presented through an attached terminal in the agent's Herdr tab.
//!
//! `agentcloudctl create` owns the session and any node it provisions; `agentterm -s ID`
//! only renders that session in the tab. Input, status, waits, output, and retirement therefore
//! use the durable agentcloud verbs instead of terminal keystrokes or Herdr's harness detection.

use super::*;

/// Registry adapter and harness name for an agentcloud-backed agent.
pub(crate) const CLOUD_HARNESS: &str = "agentcloud";
/// Session drivers accepted by `agentcloudctl create --harness`.
pub(crate) const CLOUD_DRIVERS: [&str; 4] = ["native", "claude-code", "codex", "muse-code"];
/// First inputs travel as one process argument; stay well below Linux's per-argument limit.
pub(crate) const MAX_CLOUD_TEXT_BYTES: usize = 100 * 1024;
/// `agentcloudctl create` returns once the session exists; provisioning continues server-side.
const CREATE_TIMEOUT: Duration = Duration::from_secs(300);
/// Bound for one ordinary agentcloud control verb.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(120);
/// Extra process allowance beyond the wait bound passed to `agentcloudctl wait`.
const WAIT_MARGIN: Duration = Duration::from_secs(60);
const MAX_NOTE_CHARS: usize = 2000;
const VALID_EFFORTS: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
/// Options that agentctl owns, or that must stay identical for every later verb it runs.
const RESERVED_CREATE_OPTIONS: [&str; 14] = [
    "--prompt",
    "--title",
    "--harness",
    "--model",
    "--effort",
    "--provision",
    "--envspec",
    "--purpose",
    "--workspace",
    "--node-id",
    "--ws-url",
    "--client-cert",
    "--client-key",
    "--as-crewmate",
];

/// Agentcloud-specific settings for `agentctl start --harness agentcloud`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudLaunch {
    /// Session driver for `agentcloudctl create --harness`; `None` keeps the server default.
    #[serde(default)]
    pub harness: Option<String>,
    /// Provision a fresh node for the session (`agentcloudctl create --provision`).
    #[serde(default)]
    pub provision: bool,
    /// Envspec the provisioned node is reserved from, which decides its checkout.
    #[serde(default)]
    pub envspec: Option<String>,
    /// Roster label for the provisioned node; requires `provision`.
    #[serde(default)]
    pub purpose: Option<String>,
    /// Absolute session working directory on the node; `None` keeps the server default.
    #[serde(default)]
    pub workspace: Option<String>,
    /// Existing node bound at creation; conflicts with `provision`.
    #[serde(default)]
    pub node_id: Option<String>,
}

fn single_line(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains(['\0', '\n', '\r'])
}

impl CloudLaunch {
    /// Refuse contradictory or malformed settings before any registry or remote state exists.
    pub(crate) fn validate(&self, label: &str) -> Result<()> {
        if let Some(driver) = self.harness.as_deref() {
            if !CLOUD_DRIVERS.contains(&driver) {
                return Err(fail(format!(
                    "{label} has unsupported agentcloud driver {driver:?}; use one of {}",
                    CLOUD_DRIVERS.join(", ")
                )));
            }
        }
        for (field, value) in [
            ("envspec", self.envspec.as_deref()),
            ("purpose", self.purpose.as_deref()),
            ("workspace", self.workspace.as_deref()),
            ("node_id", self.node_id.as_deref()),
        ] {
            if value.is_some_and(|value| !single_line(value)) {
                return Err(fail(format!(
                    "{label} agentcloud {field} must be a nonempty single-line string"
                )));
            }
        }
        if !self.provision && (self.envspec.is_some() || self.purpose.is_some()) {
            return Err(fail(format!(
                "{label} sets an agentcloud envspec or purpose without provision; enable provision (--provision) or remove them"
            )));
        }
        if self.provision && self.node_id.is_some() {
            return Err(fail(format!(
                "{label} cannot both provision a fresh node and bind node_id; choose one"
            )));
        }
        if self
            .workspace
            .as_deref()
            .is_some_and(|workspace| !workspace.starts_with('/'))
        {
            return Err(fail(format!(
                "{label} agentcloud workspace must be an absolute path on the session's node"
            )));
        }
        Ok(())
    }
}

/// Validate literal extra `agentcloudctl create` arguments from a profile or `--harness-arg`.
pub(crate) fn validate_create_arguments(label: &str, argv: &[String]) -> Result<()> {
    for item in argv {
        if item.is_empty() || item.contains('\0') {
            return Err(fail(format!(
                "{label} agentcloud arguments must be nonempty and contain no NUL"
            )));
        }
        let key = item.split_once('=').map_or(item.as_str(), |(key, _)| key);
        if !item.starts_with("--") || key == "--" {
            return Err(fail(format!(
                "{label} agentcloud argument {item:?} must be a long option; join values as --option=value because positional values and short options are reserved"
            )));
        }
        if RESERVED_CREATE_OPTIONS.contains(&key) {
            return Err(fail(format!(
                "{label} cannot pass {key} through raw agentcloud arguments; agentctl sets it from the name, brief, model, reasoning effort, or structured agentcloud settings, and endpoint or identity options must match every later agentctl verb"
            )));
        }
    }
    Ok(())
}

/// Executables used by agentcloud-backed agents.
#[derive(Clone, Debug)]
pub struct CloudTools {
    /// `agentcloudctl` name on PATH or explicit path.
    pub agentcloudctl: PathBuf,
    /// `agentterm` name on PATH or explicit path.
    pub agentterm: PathBuf,
    /// Agentcloud session this caller itself runs in. Input for another session is then sent as
    /// an attributed `agentcloudctl send-message` from it, because `agentcloudctl send` refuses
    /// to journal text as the human from inside a different session.
    pub caller_session: Option<String>,
}

impl Default for CloudTools {
    fn default() -> Self {
        Self {
            agentcloudctl: PathBuf::from("agentcloudctl"),
            agentterm: PathBuf::from("agentterm"),
            caller_session: None,
        }
    }
}

/// Durable agentcloud facts stored in the agent record. The session ID is `session_value`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct CloudRecord {
    pub(super) launch: CloudLaunch,
    /// Resolved absolute `agentterm` executable run in the tab.
    pub(super) agentterm: String,
    #[serde(default)]
    pub(super) create_exit: Option<i32>,
    /// Whether `agentcloudctl create` verified its node clause and queued the brief.
    #[serde(default)]
    pub(super) verified: Option<bool>,
    #[serde(default)]
    pub(super) create_note: Option<String>,
    #[serde(default)]
    pub(super) terminal_identity: Option<CustomProcessIdentity>,
    #[serde(default)]
    pub(super) halted: bool,
}

impl CloudRecord {
    pub(super) fn valid(&self) -> bool {
        self.launch.validate("agent record").is_ok()
            && self.agentterm.starts_with('/')
            && !self.agentterm.contains('\0')
            && self
                .create_note
                .as_deref()
                .is_none_or(|note| note.chars().count() <= MAX_NOTE_CHARS && !note.contains('\0'))
            && self
                .terminal_identity
                .as_ref()
                .is_none_or(CustomProcessIdentity::valid)
    }
}

/// Agentcloud session identifiers are opaque single tokens; `create` prints full UUIDs.
pub(super) fn valid_session_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

/// `create` prints only the session ID on stdout; anything else is not a recognizable ID.
fn parse_session_id(stdout: &str) -> Option<String> {
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next()?;
    (lines.next().is_none() && valid_session_id(first)).then(|| first.to_owned())
}

fn excerpt(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_NOTE_CHARS {
        return text.to_owned();
    }
    let mut kept = text.chars().take(MAX_NOTE_CHARS).collect::<String>();
    kept.push_str(" [truncated]");
    kept
}

/// Resolve a configured executable before any registry or remote state changes.
pub(crate) fn resolve_tool(configured: &Path, flag: &str) -> Result<PathBuf> {
    if configured.as_os_str().is_empty() {
        return Err(fail(format!("{flag} must not be empty")));
    }
    let candidates = if configured.components().count() > 1 || configured.is_absolute() {
        vec![configured.to_owned()]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(configured))
            .collect()
    };
    let mut not_executable = None;
    for path in candidates {
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 => {
                return fs::canonicalize(&path)
                    .map_err(|error| fail(format!("cannot resolve {}: {error}", path.display())));
            }
            Ok(metadata) if metadata.is_file() => not_executable = Some(path),
            _ => {}
        }
    }
    Err(fail(match not_executable {
        Some(path) => format!(
            "{} is not executable; fix its permissions or pass {flag} PATH",
            path.display()
        ),
        None => format!(
            "{} executable not found; install it or pass {flag} PATH",
            configured.display()
        ),
    }))
}

struct ToolRun {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_tool(executable: &Path, arguments: &[String], timeout: Duration) -> Result<ToolRun> {
    let mut command = std::process::Command::new(executable);
    command.args(arguments);
    let output = crate::client::bounded_output(command, timeout).map_err(|error| {
        AgentError::Client(crate::error::AdapterError::unavailable(format!(
            "cannot run {} {}: {error}",
            executable.display(),
            arguments.first().map_or("", String::as_str)
        )))
    })?;
    Ok(ToolRun {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Describe a failed invocation; 126 and 127 are named as invocation failures, never success.
fn failure(verb: &str, run: &ToolRun) -> String {
    let detail = if run.stderr.trim().is_empty() {
        excerpt(&run.stdout)
    } else {
        excerpt(&run.stderr)
    };
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    match run.code {
        Some(126) => format!("agentcloudctl {verb} could not execute a command (exit 126){detail}"),
        Some(127) => format!("agentcloudctl {verb} could not find a command (exit 127){detail}"),
        Some(code) => format!("agentcloudctl {verb} exited {code}{detail}"),
        None => format!("agentcloudctl {verb} was terminated by a signal{detail}"),
    }
}

fn client_error(message: impl Into<String>) -> AgentError {
    AgentError::Client(crate::error::AdapterError::unavailable(message))
}

impl AgentRecord {
    pub(super) fn is_cloud(&self) -> bool {
        self.adapter == CLOUD_HARNESS
    }

    fn cloud(&self) -> Result<&CloudRecord> {
        self.agentcloud
            .as_ref()
            .filter(|_| self.is_cloud())
            .ok_or_else(|| {
                fail(format!(
                    "agent {:?} is not an agentcloud session; use the ordinary agentctl commands",
                    self.name
                ))
            })
    }

    fn cloud_session(&self) -> Result<&str> {
        self.cloud()?;
        self.session_value.as_deref().ok_or_else(|| {
            fail(format!(
                "agent {:?} has no recorded agentcloud session; inspect its launch error with agentctl status, then agentctl stop to archive it",
                self.name
            ))
        })
    }
}

impl<A: ManagedApi + ?Sized> ManagedAgents<'_, A> {
    /// Report whether a registered agent is backed by an agentcloud session.
    pub fn is_cloud(&self, agent_name: &str) -> Result<bool> {
        Ok(self.load(agent_name)?.is_cloud())
    }

    fn cloud_launch_failed(
        &self,
        record: &mut AgentRecord,
        options: &StartOptions,
        error: String,
    ) -> Result<Value> {
        record.lifecycle = "launch_failed".to_owned();
        record.error = Some(if options.environment.is_empty() {
            error.clone()
        } else {
            "launch failed with caller-supplied environment; details omitted from status".to_owned()
        });
        self.save(record)?;
        let directory = self.directory(&record.name)?;
        let remedy = match record.session_value.as_deref() {
            Some(session) => format!(
                "; agentcloud session {session} exists and may be live: attach with `agentterm -s {session}`, or run `agentctl stop {}` to halt it and archive the record",
                record.name
            ),
            None => format!(
                "; no agentcloud session was recorded; run `agentctl stop {}` to archive the record",
                record.name
            ),
        };
        Err(client_error(format!(
            "launch of {:?} failed: {error}; record retained at {}{remedy}",
            record.name,
            directory.display()
        )))
    }

    /// Create the agentcloud session, then attach its terminal in a fresh Herdr tab.
    pub(super) fn start_cloud(
        &self,
        agent_name: &str,
        cwd: PathBuf,
        mut options: StartOptions,
        reasoning_effort: Option<&str>,
    ) -> Result<Value> {
        let launch = options.cloud.clone().unwrap_or_default();
        launch.validate("agentcloud launch")?;
        if options.resume.is_some() {
            return Err(fail(
                "--resume does not apply to agentcloud agents; attach an existing session directly with `agentterm -s SESSION_ID`",
            ));
        }
        validate_create_arguments("agentcloud launch", &options.harness_args)?;
        if options
            .model
            .as_deref()
            .is_some_and(|model| !single_line(model))
        {
            return Err(fail("model must be a nonempty single-line string"));
        }
        if reasoning_effort.is_some_and(|effort| !VALID_EFFORTS.contains(&effort)) {
            return Err(fail(format!(
                "unsupported reasoning effort {reasoning_effort:?}; use one of {}",
                VALID_EFFORTS.join(", ")
            )));
        }
        if options
            .brief
            .as_deref()
            .is_some_and(|brief| brief.len() > MAX_CLOUD_TEXT_BYTES)
        {
            return Err(fail(format!(
                "agentcloud brief exceeds {MAX_CLOUD_TEXT_BYTES} bytes; agentcloudctl takes the first input as one argument, so shorten it and send the remainder with agentctl send"
            )));
        }
        options.environment = environment_entries(&options.environment)?;
        let agentcloudctl = resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin")?;
        let agentterm = resolve_tool(&self.cloud_tools.agentterm, "--agentterm-bin")?;

        let mut create = vec![
            "create".to_owned(),
            "--title".to_owned(),
            agent_name.to_owned(),
        ];
        push_value(&mut create, "--harness", launch.harness.as_deref());
        push_value(&mut create, "--model", options.model.as_deref());
        push_value(&mut create, "--effort", reasoning_effort);
        push_value(&mut create, "--workspace", launch.workspace.as_deref());
        push_value(&mut create, "--node-id", launch.node_id.as_deref());
        if launch.provision {
            create.push("--provision".to_owned());
        }
        push_value(&mut create, "--envspec", launch.envspec.as_deref());
        push_value(&mut create, "--purpose", launch.purpose.as_deref());
        create.extend_from_slice(&options.harness_args);
        let recorded_arguments = create.clone();
        // agentcloudctl refuses a leading /goal in --prompt; it must travel through send.
        let goal_brief = options
            .brief
            .as_deref()
            .filter(|brief| brief.trim_start().starts_with("/goal"));
        if let Some(brief) = options.brief.as_deref().filter(|_| goal_brief.is_none()) {
            create.extend(["--prompt".to_owned(), brief.to_owned()]);
        }

        // The project workspace policy places the terminal tab exactly as for a local worker.
        let project_workspace = self.start_workspace_policy(&options)?;
        let _lock = self.lock(agent_name)?;
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
            adapter: CLOUD_HARNESS.to_owned(),
            mode: interactive_mode(),
            backend: herdr_adapter(),
            paused: false,
            runtime_home: None,
            pane_reported_by_agentctl: false,
            custom_process_identity: None,
            foreign_shell_identity: None,
            agentcloud: Some(CloudRecord {
                launch,
                agentterm: agentterm.display().to_string(),
                create_exit: None,
                verified: None,
                create_note: None,
                terminal_identity: None,
                halted: false,
            }),
            extra: BTreeMap::new(),
            name: agent_name.to_owned(),
            token: format!("{}-{}", now.as_nanos(), std::process::id()),
            harness: CLOUD_HARNESS.to_owned(),
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
            resume: None,
            arguments: recorded_arguments,
            startup_warning: None,
            effective_reasoning_effort: reasoning_effort.map(str::to_owned),
            error: None,
            goal: None,
            goal_delivery: None,
            goal_session_id: None,
            goal_command: None,
            goal_messages: BTreeMap::new(),
            goal_message_id: None,
        };
        self.save(&record)?;

        let run = match run_tool(&agentcloudctl, &create, CREATE_TIMEOUT) {
            Ok(run) => run,
            Err(error) => {
                return self.cloud_launch_failed(&mut record, &options, error.to_string())
            }
        };
        let session = parse_session_id(&run.stdout);
        let verified = match (run.code, session.as_deref()) {
            (Some(0), Some(_)) => true,
            (Some(9), Some(_)) => false,
            (_, Some(session)) => {
                record.session_agent = Some(CLOUD_HARNESS.to_owned());
                record.session_value = Some(session.to_owned());
                let message = format!(
                    "{} after printing session ID {session}",
                    failure("create", &run)
                );
                return self.cloud_launch_failed(&mut record, &options, message);
            }
            (Some(0), None) => {
                let message = format!(
                    "agentcloudctl create exited 0 but printed no recognizable session ID on stdout ({:?}); a session may exist, so check `agentcloudctl list` before retrying",
                    excerpt(&run.stdout)
                );
                return self.cloud_launch_failed(&mut record, &options, message);
            }
            (_, None) => {
                return self.cloud_launch_failed(&mut record, &options, failure("create", &run))
            }
        };
        let session = session.expect("matched session");
        record.session_agent = Some(CLOUD_HARNESS.to_owned());
        record.session_value = Some(session.clone());
        let cloud = record.agentcloud.as_mut().expect("cloud record");
        cloud.create_exit = run.code;
        cloud.verified = Some(verified);
        cloud.create_note = (!run.stderr.trim().is_empty()).then(|| excerpt(&run.stderr));
        self.save(&record)?;

        if let Err(error) =
            self.create_presentation(&mut record, &options, project_workspace.as_deref())
        {
            return self.cloud_launch_failed(
                &mut record,
                &options,
                format!("cannot create the agent's Herdr tab: {error}"),
            );
        }
        let pane_id = record.pane_id.clone().expect("new tab has pane");
        let arguments = ["-s".to_owned(), session.clone()];
        match self.client.start_pane_command(
            &pane_id,
            &agentterm,
            &arguments,
            options.startup_timeout,
        ) {
            Ok(identity) => {
                record
                    .agentcloud
                    .as_mut()
                    .expect("cloud record")
                    .terminal_identity = Some(identity);
            }
            Err(error) => {
                return self.cloud_launch_failed(
                    &mut record,
                    &options,
                    format!("agentterm did not attach in pane {pane_id}: {error}"),
                );
            }
        }
        record.lifecycle = "running".to_owned();
        self.save(&record)?;

        let mut deferred = None;
        if let Some(brief) = goal_brief.filter(|_| verified) {
            if let Err(error) = self.cloud_send_record(&record, brief, None) {
                deferred = Some(format!(
                    "session {session} is running and attached, but its /goal brief was not delivered: {error}"
                ));
            }
        }
        if !verified {
            let brief = if options.brief.is_some() {
                "the brief was NOT queued; "
            } else {
                ""
            };
            let note = record
                .agentcloud
                .as_ref()
                .and_then(|cloud| cloud.create_note.clone())
                .map_or_else(String::new, |note| format!(" ({note})"));
            deferred = Some(format!(
                "agentcloudctl create exited 9: session {session} exists but its node clause is UNVERIFIED{note}; {brief}the agentterm tab is attached. Do not start it again: inspect it with `agentctl status {agent_name}`, deliver work with `agentctl send {agent_name} --file PROMPT`, or retire it with `agentctl stop {agent_name}`"
            ));
        }
        if let Some(message) = deferred {
            return Err(client_error(message));
        }
        self.status(agent_name)
    }

    fn cloud_fleet(&self, fleet: &mut Option<std::result::Result<Vec<Value>, String>>) {
        if fleet.is_some() {
            return;
        }
        let fetched = resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin")
            .map_err(|error| error.to_string())
            .and_then(|executable| {
                run_tool(&executable, &["list".to_owned()], CONTROL_TIMEOUT)
                    .map_err(|error| error.to_string())
            })
            .and_then(|run| {
                if run.code != Some(0) {
                    return Err(failure("list", &run));
                }
                serde_json::from_str::<Vec<Value>>(&run.stdout)
                    .map_err(|error| format!("agentcloudctl list printed invalid JSON: {error}"))
            });
        *fleet = Some(fetched);
    }

    fn cloud_terminal(&self, record: &AgentRecord) -> Value {
        let Some(pane_id) = record.pane_id.as_deref() else {
            return json!({"pane_id": null, "pane_present": false, "agentterm_attached": false});
        };
        let panes = match self.client.panes() {
            Ok(panes) => panes,
            Err(error) => {
                return json!({"pane_id": pane_id, "probe_error": error.to_string()});
            }
        };
        let Some(pane) = panes.iter().find(|pane| pane.pane_id == pane_id) else {
            return json!({"pane_id": pane_id, "pane_present": false, "agentterm_attached": false});
        };
        let identity = record
            .agentcloud
            .as_ref()
            .and_then(|cloud| cloud.terminal_identity.as_ref());
        let mut terminal = json!({
            "pane_id": pane_id,
            "tab_id": pane.tab_id,
            "pane_present": true,
            "in_recorded_tab": Some(&pane.tab_id) == record.tab_id.as_ref(),
            "agentterm_attached": null,
        });
        if let Some(identity) = identity {
            match self.client.pane_runs_command(pane_id, identity) {
                Ok(attached) => terminal["agentterm_attached"] = json!(attached),
                Err(error) => terminal["probe_error"] = json!(error.to_string()),
            }
        }
        terminal
    }

    /// Status without attaching to the session: the fleet listing never wakes a dormant session.
    pub(super) fn cloud_status_record(
        &self,
        record: &AgentRecord,
        fleet: &mut Option<std::result::Result<Vec<Value>, String>>,
    ) -> Result<Value> {
        let mut result = json!(record);
        result["output"] = json!(self.directory(&record.name)?.join("output.json"));
        result["session_id"] = json!(record.session_value);
        result["terminal"] = self.cloud_terminal(record);
        result["cloud"] = Value::Null;
        let Some(session) = record.session_value.as_deref() else {
            result["agent_status"] = json!("unknown");
            result["probe_error"] = json!("no agentcloud session was recorded for this agent");
            return Ok(result);
        };
        self.cloud_fleet(fleet);
        match fleet.as_ref().expect("fleet fetched") {
            Err(error) => {
                result["agent_status"] = json!("unknown");
                result["probe_error"] = json!(error);
            }
            Ok(rows) => match rows.iter().find(|row| row["session_id"] == session) {
                None => {
                    result["agent_status"] = json!("unknown");
                    result["probe_error"] = json!(format!(
                        "session {session} is not in `agentcloudctl list`; the listing is a lagged projection, so retry shortly or run `agentcloudctl inspect -s {session}`"
                    ));
                }
                Some(row) => {
                    let mut cloud = serde_json::Map::new();
                    for key in [
                        "title",
                        "harness",
                        "workspace",
                        "running",
                        "activity",
                        "attached_nodes",
                        "status",
                        "last_outcome",
                        "resolved_status",
                        "last_activity_unix_ms",
                    ] {
                        if let Some(value) = row.get(key) {
                            cloud.insert(key.to_owned(), value.clone());
                        }
                    }
                    // A queued first prompt on a still-provisioning node is `waiting`, not idle.
                    let activity = row["activity"]
                        .as_str()
                        .filter(|activity| *activity != "idle" && valid_session_id(activity));
                    result["agent_status"] = json!(if row["running"] == true {
                        "working"
                    } else {
                        activity.unwrap_or("idle")
                    });
                    result["cloud"] = Value::Object(cloud);
                    result["probe_error"] = Value::Null;
                }
            },
        }
        Ok(result)
    }

    fn cloud_send_record(
        &self,
        record: &AgentRecord,
        text: &str,
        message_id: Option<&str>,
    ) -> Result<Value> {
        let session = record.cloud_session()?;
        if record.paused {
            return Err(fail(format!(
                "agent {:?} is paused for human input; run agentctl resume before sending automation input",
                record.name
            )));
        }
        if text.trim().is_empty() {
            return Err(fail("instruction must not be empty"));
        }
        if text.len() > MAX_CLOUD_TEXT_BYTES {
            return Err(fail(format!(
                "agentcloud input exceeds {MAX_CLOUD_TEXT_BYTES} bytes; agentcloudctl sends one argument, so split it into smaller messages"
            )));
        }
        let key = match message_id {
            Some(identifier) if super::message_id(identifier) => identifier.to_owned(),
            Some(_) => {
                return Err(fail(
                    "message id must be 1-255 ASCII letters, digits, dots, underscores, or hyphens and start with a letter or digit",
                ))
            }
            None => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| fail(error.to_string()))?;
                format!("agentctl-{}-{}-{}", record.name, now.as_nanos(), std::process::id())
            }
        };
        let caller = self
            .cloud_tools
            .caller_session
            .as_deref()
            .filter(|caller| *caller != session);
        if caller.is_some_and(|caller| !valid_session_id(caller)) {
            return Err(fail(
                "--from-session must be one agentcloud session ID token",
            ));
        }
        let goal = text.trim_start().starts_with("/goal");
        let (verb, arguments, end_of_turn) = match caller {
            // Inside another session, plain `send` is refused: it would journal the text as the
            // human. The attributed peer message arrives on the target as an ordinary input.
            Some(caller) => (
                "send-message",
                vec![
                    "send-message".to_owned(),
                    "--session".to_owned(),
                    caller.to_owned(),
                    "--to".to_owned(),
                    session.to_owned(),
                    "--body".to_owned(),
                    text.to_owned(),
                    "--idempotency-key".to_owned(),
                    key.clone(),
                ],
                None,
            ),
            // A message-initial /goal is consumed at ingress; everything else waits for the turn
            // to end, matching agentctl's deliver-when-ready contract instead of steering it.
            None => {
                let mut arguments = vec![
                    "send".to_owned(),
                    "--session".to_owned(),
                    session.to_owned(),
                    "--text".to_owned(),
                    text.to_owned(),
                    "--idempotency-key".to_owned(),
                    key.clone(),
                ];
                if !goal {
                    arguments.push("--end-of-turn".to_owned());
                }
                ("send", arguments, Some(!goal))
            }
        };
        let executable = resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin")?;
        let run = run_tool(&executable, &arguments, CONTROL_TIMEOUT)?;
        if run.code != Some(0) {
            return Err(client_error(format!(
                "{}; the input may or may not have been delivered. Retry with `agentctl send {} --message-id {key} ...`; agentcloudctl deduplicates that idempotency key",
                failure(verb, &run),
                record.name
            )));
        }
        Ok(json!({
            "name": record.name,
            "session_id": session,
            "message_id": key,
            "outcome": "journaled",
            "delivery": verb,
            "from_session": caller,
            "end_of_turn": end_of_turn,
            "agentcloud": excerpt(&run.stdout),
        }))
    }

    /// Durably journal one input through `agentcloudctl send`.
    pub fn send_cloud(
        &self,
        agent_name: &str,
        text: &str,
        message_id: Option<&str>,
    ) -> Result<Value> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        self.cloud_send_record(&record, text, message_id)
    }

    /// Wait for the session's current or last run to settle through `agentcloudctl wait`.
    pub(super) fn cloud_wait(&self, agent_name: &str, timeout: Duration) -> Result<Value> {
        let (token, session) = {
            let _lock = self.lock(agent_name)?;
            let record = self.load(agent_name)?;
            (record.token.clone(), record.cloud_session()?.to_owned())
        };
        let milliseconds = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let executable = resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin")?;
        let run = run_tool(
            &executable,
            &[
                "wait".to_owned(),
                "--session".to_owned(),
                session.clone(),
                "--until".to_owned(),
                "settle".to_owned(),
                "--timeout-ms".to_owned(),
                milliseconds.to_string(),
            ],
            timeout.saturating_add(WAIT_MARGIN),
        )?;
        let outcome = run
            .stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .to_owned();
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        if record.token != token {
            return Err(fail(format!(
                "agent {agent_name:?} was replaced while waiting"
            )));
        }
        match run.code {
            Some(0) => {
                let mut status = self.cloud_status_record(&record, &mut None)?;
                status["cloud_outcome"] = json!(outcome);
                Ok(status)
            }
            Some(1) => Err(fail(format!(
                "agent {agent_name:?} settled without completing its run (agentcloudctl wait printed {outcome:?}, exit 1); read its reply with `agentctl read {agent_name} --output last`"
            ))),
            Some(2) => Err(fail(format!(
                "agent {agent_name:?}'s run was interrupted (agentcloudctl wait printed {outcome:?}, exit 2); send new input or inspect it with `agentctl status {agent_name}`"
            ))),
            Some(3) => Err(fail(format!(
                "timed out or disconnected waiting for agent {agent_name:?} to settle; the outcome is unknown, so run `agentctl wait {agent_name}` again or inspect `agentctl status {agent_name}`"
            ))),
            _ => Err(client_error(failure("wait", &run))),
        }
    }

    /// Read the durable final reply (`last`) or the attached terminal (`tail`/`all`).
    pub fn read_cloud(&self, agent_name: &str, lines: usize, output: &str) -> Result<String> {
        let _lock = self.lock(agent_name)?;
        let record = self.load(agent_name)?;
        record.cloud()?;
        let text = match output {
            "last" => {
                let session = record.cloud_session()?;
                let executable =
                    resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin")?;
                let run = run_tool(
                    &executable,
                    &[
                        "output".to_owned(),
                        "--session".to_owned(),
                        session.to_owned(),
                    ],
                    CONTROL_TIMEOUT,
                )?;
                if run.code != Some(0) {
                    return Err(client_error(format!(
                        "{}; a session with no settled run has no final reply yet, so check `agentctl status {agent_name}`",
                        failure("output", &run)
                    )));
                }
                run.stdout
            }
            "tail" | "all" => {
                let pane_id = record
                    .pane_id
                    .as_deref()
                    .filter(|pane_id| {
                        self.client.panes().is_ok_and(|panes| {
                            panes.iter().any(|pane| {
                                pane.pane_id == *pane_id && Some(&pane.tab_id) == record.tab_id.as_ref()
                            })
                        })
                    })
                    .ok_or_else(|| {
                        fail(format!(
                            "agent {agent_name:?} has no live terminal tab; read its durable reply with `agentctl read {agent_name} --output last`"
                        ))
                    })?;
                self.client.read(pane_id, "recent", Some(lines))?
            }
            _ => {
                return Err(fail(
                    "agentcloud agents support --output last, tail, or all; since_turn requires a headless transcript",
                ))
            }
        };
        self.snapshot(&record, &text)?;
        Ok(text)
    }

    /// Focus the agent's terminal tab after confirming it is still the recorded pane.
    pub(super) fn cloud_attach(&self, record: &AgentRecord) -> Result<Value> {
        let session = record.cloud_session().ok();
        let pane_id = record.pane_id.as_deref().filter(|pane_id| {
            self.client.panes().is_ok_and(|panes| {
                panes.iter().any(|pane| {
                    pane.pane_id == *pane_id && Some(&pane.tab_id) == record.tab_id.as_ref()
                })
            })
        });
        let Some(pane_id) = pane_id else {
            return Err(fail(match session {
                Some(session) => format!(
                    "agent {:?} has no live terminal tab; attach from any terminal with `agentterm -s {session}`",
                    record.name
                ),
                None => format!("agent {:?} has no live terminal tab", record.name),
            }));
        };
        self.client.focus_pane(pane_id)?;
        Ok(
            json!({"name": record.name, "pane_id": pane_id, "paused": record.paused, "session_id": session}),
        )
    }

    /// Halt and archive the session, close the recorded terminal pane, and archive the record.
    pub(super) fn cloud_stop(
        &self,
        mut record: AgentRecord,
        options: &StopOptions,
    ) -> Result<Value> {
        let agent_name = record.name.clone();
        if options.recover_legacy_adoption {
            return Err(fail(
                "--recover-legacy-adoption applies only to adopted Herdr agents",
            ));
        }
        let panes = self.client.panes().map_err(|error| {
            fail(format!(
                "cannot inspect Herdr before stopping {agent_name:?}: {error}; nothing was halted, closed, or archived"
            ))
        })?;
        let owned = record
            .pane_id
            .as_ref()
            .and_then(|pane_id| panes.iter().find(|pane| &pane.pane_id == pane_id))
            .cloned();
        if let Some(pane) = &owned {
            if Some(&pane.tab_id) != record.tab_id.as_ref()
                || Some(&pane.workspace_id) != record.workspace_id.as_ref()
            {
                return Err(fail(format!(
                    "refusing to stop {agent_name:?}: its terminal pane moved to another tab or workspace; move it back or close it yourself, then retry"
                )));
            }
        }
        let session = record.session_value.clone();
        let executable = session
            .as_ref()
            .filter(|_| !options.skip_cloud_halt)
            .map(|_| resolve_tool(&self.cloud_tools.agentcloudctl, "--agentcloudctl-bin"))
            .transpose()?;
        if let (Some(session), Some(executable)) = (session.as_deref(), executable.as_deref()) {
            let run = run_tool(
                executable,
                &[
                    "halt".to_owned(),
                    "--session".to_owned(),
                    session.to_owned(),
                    "--reason".to_owned(),
                    format!("agentctl stop {agent_name}"),
                ],
                CONTROL_TIMEOUT,
            )?;
            if run.code != Some(0) {
                return Err(client_error(format!(
                    "{}; nothing was closed or archived. Retry `agentctl stop {agent_name}` (halt is idempotent), or pass --skip-cloud-halt if the session no longer exists or was retired elsewhere",
                    failure("halt", &run)
                )));
            }
            if let Some(cloud) = record.agentcloud.as_mut() {
                cloud.halted = true;
            }
            self.save(&record)?;
        }
        let (archive, destination) = self.archive_destination(&record)?;
        let mut snapshot_error = None;
        if let Some(pane) = &owned {
            match self.bounded_terminal_text(&pane.pane_id) {
                Ok(text) => self.snapshot(&record, &text)?,
                Err(error) => snapshot_error = Some(error.to_string()),
            }
            record.lifecycle = "stopping".to_owned();
            self.save(&record)?;
            self.client.close_pane(&pane.pane_id)?;
        }
        let mut session_archived = false;
        let mut archive_error = None;
        if let (Some(session), Some(executable)) = (session.as_deref(), executable.as_deref()) {
            match run_tool(
                executable,
                &[
                    "archive".to_owned(),
                    "--session".to_owned(),
                    session.to_owned(),
                ],
                CONTROL_TIMEOUT,
            ) {
                Ok(run) if run.code == Some(0) => session_archived = true,
                Ok(run) => archive_error = Some(failure("archive", &run)),
                Err(error) => archive_error = Some(error.to_string()),
            }
        }
        record.lifecycle = "stopped".to_owned();
        self.save(&record)?;
        fs::rename(self.directory(&agent_name)?, &destination)
            .map_err(|error| fail(error.to_string()))?;
        agent::sync_directory(&archive)?;
        agent::sync_directory(&self.registry)?;
        let tab_closed = if owned.is_none() {
            Some(false)
        } else {
            self.client.panes().ok().map(|panes| {
                panes
                    .iter()
                    .all(|pane| Some(&pane.tab_id) != record.tab_id.as_ref())
            })
        };
        let halted = executable.is_some();
        Ok(json!({
            "name": agent_name,
            "archive": destination,
            "session_id": session,
            "session_halted": halted,
            "session_archived": session_archived,
            "session_archive_error": archive_error,
            "pane_closed": owned.is_some(),
            "tab_closed": tab_closed,
            "snapshot_error": snapshot_error,
            "node_lease": if session.is_none() {
                "no agentcloud session was recorded"
            } else if halted {
                "stop does not release the node reservation; the orchestrator releases a provisioned node's reservation on its own schedule after the halted session goes idle, which can be long after stop returns. Check the reservation line of `agentcloudctl inspect -s SESSION_ID`"
            } else {
                "--skip-cloud-halt left the agentcloud session untouched, including any node lease"
            },
        }))
    }
}

fn push_value(arguments: &mut Vec<String>, flag: &str, value: Option<&str>) {
    if let Some(value) = value {
        arguments.extend([flag.to_owned(), value.to_owned()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AdapterError, Result as AdapterResult};
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    const SESSION: &str = "0b7c9a2e-1f3d-4c5b-8a69-2d4e6f8a0b1c";

    /// Herdr stand-in: one tab per start, a pinned terminal identity, and recorded closes.
    #[derive(Default)]
    struct Herdr {
        panes: Mutex<Vec<Pane>>,
        environments: Mutex<Vec<Vec<String>>>,
        commands: Mutex<Vec<(String, PathBuf, Vec<String>)>>,
        closed: Mutex<Vec<String>>,
        focused: Mutex<Vec<String>>,
        fail_command: AtomicBool,
        detached: AtomicBool,
        labels: Mutex<Vec<String>>,
    }

    fn identity() -> CustomProcessIdentity {
        CustomProcessIdentity {
            version: 1,
            boot_id: "11111111-2222-3333-4444-555555555555".to_owned(),
            pid: 4343,
            starttime_ticks: 77,
            executable_device: 5,
            executable_inode: 9,
        }
    }

    fn unused<T>() -> AdapterResult<T> {
        Err(AdapterError::unavailable(
            "agentcloud agents never use Herdr harness control",
        ))
    }

    impl AgentApi for Herdr {
        fn panes(&self) -> AdapterResult<Vec<Pane>> {
            Ok(self.panes.lock().unwrap().clone())
        }
        fn pane_info(&self, _: &str) -> AdapterResult<AgentPaneInfo> {
            unused()
        }
        fn workspace_label(&self, _: &str) -> AdapterResult<String> {
            Ok("subagents".to_owned())
        }
        fn run(&self, _: &str, _: &str) -> AdapterResult<()> {
            unused()
        }
        fn wait_agent_status(&self, _: &str, _: &str, _: u64) -> AdapterResult<()> {
            unused()
        }
        fn read(&self, _: &str, _: &str, _: Option<usize>) -> AdapterResult<String> {
            Ok("agentterm screen".to_owned())
        }
    }

    impl ManagedApi for Herdr {
        fn workspace_id_for_label(&self, label: &str) -> AdapterResult<Option<String>> {
            self.labels.lock().unwrap().push(label.to_owned());
            Ok(Some("workspace".to_owned()))
        }
        fn create_workspace(
            &self,
            _: &str,
            _: &str,
            _: &[String],
        ) -> AdapterResult<(String, String, String)> {
            unused()
        }
        fn create_tab(&self, _: &str, _: &str, _: &str, _: &[String]) -> AdapterResult<String> {
            unused()
        }
        fn create_tab_with_pane(
            &self,
            workspace: &str,
            _: &str,
            _: &str,
            environment: &[String],
        ) -> AdapterResult<(String, String)> {
            self.environments.lock().unwrap().push(environment.to_vec());
            self.panes.lock().unwrap().push(Pane {
                pane_id: "cloud-pane".to_owned(),
                tab_id: "cloud-tab".to_owned(),
                workspace_id: workspace.to_owned(),
            });
            Ok(("cloud-tab".to_owned(), "cloud-pane".to_owned()))
        }
        fn close_pane(&self, pane: &str) -> AdapterResult<()> {
            self.closed.lock().unwrap().push(pane.to_owned());
            self.panes
                .lock()
                .unwrap()
                .retain(|entry| entry.pane_id != pane);
            Ok(())
        }
        fn focus_pane(&self, pane: &str) -> AdapterResult<()> {
            self.focused.lock().unwrap().push(pane.to_owned());
            Ok(())
        }
        fn rename_tab(&self, _: &str, _: &str) -> AdapterResult<()> {
            Ok(())
        }
        fn start_agent(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
            _: Duration,
        ) -> AdapterResult<()> {
            unused()
        }
        fn start_pane_command(
            &self,
            pane: &str,
            executable: &Path,
            arguments: &[String],
            _: Duration,
        ) -> AdapterResult<CustomProcessIdentity> {
            self.commands.lock().unwrap().push((
                pane.to_owned(),
                executable.to_owned(),
                arguments.to_vec(),
            ));
            if self.fail_command.load(Ordering::Relaxed) {
                return Err(AdapterError::unavailable(
                    "agentterm was not the foreground process",
                ));
            }
            Ok(identity())
        }
        fn pane_runs_command(
            &self,
            _: &str,
            recorded: &CustomProcessIdentity,
        ) -> AdapterResult<bool> {
            Ok(*recorded == identity() && !self.detached.load(Ordering::Relaxed))
        }
        fn agent_pane(&self, _: &str) -> AdapterResult<String> {
            unused()
        }
        fn report_agent_session(&self, _: &str, _: &str, _: &str, _: &str) -> AdapterResult<()> {
            unused()
        }
        fn send_keys(&self, _: &str, _: &str) -> AdapterResult<()> {
            unused()
        }
        fn close_tab(&self, _: &str) -> AdapterResult<()> {
            panic!("agentcloud stop must close exactly the recorded pane")
        }
    }

    /// A fake `agentcloudctl` that records each argv and replays per-verb canned responses.
    const FAKE_AGENTCLOUDCTL: &str = r#"#!/bin/sh
dir=$(cd "$(dirname "$0")" && pwd)
verb=$1
mkdir -p "$dir/calls"
n=$(ls "$dir/calls" | wc -l | tr -d ' ')
for arg in "$@"; do printf '%s\0' "$arg"; done > "$dir/calls/$(printf %04d "$n")-$verb"
[ -f "$dir/responses/$verb.stdout" ] && cat "$dir/responses/$verb.stdout"
[ -f "$dir/responses/$verb.stderr" ] && cat "$dir/responses/$verb.stderr" >&2
[ -f "$dir/responses/$verb.exit" ] && exit "$(cat "$dir/responses/$verb.exit")"
exit 0
"#;

    struct Fixture {
        root: PathBuf,
        herdr: Herdr,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "agentctl-cloud-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(root.join("tools/responses")).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            for (name, body) in [
                ("agentcloudctl", FAKE_AGENTCLOUDCTL),
                ("agentterm", "#!/bin/sh\nexit 0\n"),
            ] {
                let path = root.join("tools").join(name);
                fs::write(&path, body).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let fixture = Self {
                root,
                herdr: Herdr::default(),
            };
            fixture.respond("create", Some(&format!("{SESSION}\n")), None, None);
            fixture.respond("send", Some("42\n"), None, None);
            fixture
        }

        fn respond(
            &self,
            verb: &str,
            stdout: Option<&str>,
            stderr: Option<&str>,
            exit: Option<i32>,
        ) {
            let responses = self.root.join("tools/responses");
            for (suffix, value) in [
                ("stdout", stdout.map(str::to_owned)),
                ("stderr", stderr.map(str::to_owned)),
                ("exit", exit.map(|code| code.to_string())),
            ] {
                let path = responses.join(format!("{verb}.{suffix}"));
                match value {
                    Some(value) => fs::write(path, value).unwrap(),
                    None => {
                        let _ = fs::remove_file(path);
                    }
                }
            }
        }

        fn tools(&self) -> CloudTools {
            CloudTools {
                agentcloudctl: self.root.join("tools/agentcloudctl"),
                agentterm: self.root.join("tools/agentterm"),
                caller_session: None,
            }
        }

        fn manager(&self) -> ManagedAgents<'_, Herdr> {
            ManagedAgents::new(&self.herdr, &self.root.join("registry"))
                .unwrap()
                .with_inherited_workspace(None)
                .with_cloud_tools(self.tools())
        }

        fn options(&self, brief: Option<&str>) -> StartOptions {
            StartOptions {
                workspace_id: Some("workspace".to_owned()),
                harness: CLOUD_HARNESS.to_owned(),
                model: Some("claude-opus".to_owned()),
                brief: brief.map(str::to_owned),
                environment: vec!["TAB_ONLY=literal $(value)".to_owned()],
                cloud: Some(CloudLaunch {
                    harness: Some("claude-code".to_owned()),
                    provision: true,
                    envspec: Some("monorepo".to_owned()),
                    purpose: Some("sub-worker checkout".to_owned()),
                    ..CloudLaunch::default()
                }),
                ..StartOptions::default()
            }
        }

        fn start(&self, brief: Option<&str>) -> Result<Value> {
            self.manager().start_with_reasoning_effort(
                "sub-worker",
                &self.root,
                "high",
                self.options(brief),
            )
        }

        /// Every recorded agentcloudctl argv, in invocation order.
        fn calls(&self) -> Vec<Vec<String>> {
            let directory = self.root.join("tools/calls");
            let Ok(entries) = fs::read_dir(&directory) else {
                return Vec::new();
            };
            let mut names = entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            names.sort();
            names
                .iter()
                .map(|name| {
                    fs::read(directory.join(name))
                        .unwrap()
                        .split(|byte| *byte == 0)
                        .filter(|value| !value.is_empty())
                        .map(|value| String::from_utf8(value.to_vec()).unwrap())
                        .collect()
                })
                .collect()
        }

        fn verbs(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .map(|call| call[0].clone())
                .collect()
        }

        fn record(&self) -> Value {
            agent::read_private_json(&self.root.join("registry/sub-worker/agent.json")).unwrap()
        }

        fn list_row(&self, running: bool) {
            self.respond(
                "list",
                Some(
                    &json!([
                        {"session_id": "someone-else", "running": true},
                        {"session_id": SESSION, "running": running, "attached_nodes": ["node-a"], "title": "sub-worker", "tokens": {"input_durable": 1}}
                    ])
                    .to_string(),
                ),
                None,
                None,
            );
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn start_creates_the_session_then_attaches_agentterm_in_a_new_tab() {
        let fixture = Fixture::new();
        fixture.list_row(true);
        let status = fixture
            .start(Some("Run hostname, then reply DONE."))
            .unwrap();

        let calls = fixture.calls();
        assert_eq!(
            calls[0],
            strings(&[
                "create",
                "--title",
                "sub-worker",
                "--harness",
                "claude-code",
                "--model",
                "claude-opus",
                "--effort",
                "high",
                "--provision",
                "--envspec",
                "monorepo",
                "--purpose",
                "sub-worker checkout",
                "--prompt",
                "Run hostname, then reply DONE.",
            ])
        );
        assert_eq!(calls[1], ["list"]);
        let commands = fixture.herdr.commands.lock().unwrap().clone();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].0, "cloud-pane");
        assert_eq!(
            commands[0].1,
            fs::canonicalize(fixture.root.join("tools/agentterm")).unwrap()
        );
        assert_eq!(commands[0].2, strings(&["-s", SESSION]));
        assert_eq!(
            *fixture.herdr.environments.lock().unwrap(),
            [strings(&["TAB_ONLY=literal $(value)"])]
        );

        assert_eq!(status["lifecycle"], "running");
        assert_eq!(status["adapter"], "agentcloud");
        assert_eq!(status["session_id"], SESSION);
        assert_eq!(status["agent_status"], "working");
        assert_eq!(status["cloud"]["attached_nodes"], json!(["node-a"]));
        assert!(status["cloud"].get("tokens").is_none());
        assert_eq!(status["terminal"]["agentterm_attached"], true);
        assert_eq!(status["probe_error"], Value::Null);
        assert!(!status.to_string().contains("literal $(value)"));

        let record = fixture.record();
        assert_eq!(record["session_value"], SESSION);
        assert_eq!(record["agentcloud"]["verified"], true);
        assert_eq!(record["agentcloud"]["terminal_identity"]["pid"], 4343);
        assert!(
            !record["arguments"].to_string().contains("reply DONE"),
            "the brief is input, not launch policy"
        );
    }

    #[test]
    fn create_failure_reports_stderr_and_never_opens_a_tab() {
        let fixture = Fixture::new();
        fixture.respond(
            "create",
            None,
            Some("envspec refused: no capacity\n"),
            Some(1),
        );
        let error = fixture.start(Some("task")).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("agentcloudctl create exited 1: envspec refused: no capacity"));
        assert!(message.contains("no agentcloud session was recorded"));
        assert_eq!(error.exit_code(), 69);
        assert!(fixture.herdr.panes.lock().unwrap().is_empty());
        let record = fixture.record();
        assert_eq!(record["lifecycle"], "launch_failed");
        assert_eq!(record["session_value"], Value::Null);

        let stopped = fixture.manager().stop("sub-worker").unwrap();
        assert_eq!(stopped["session_halted"], false);
        assert_eq!(
            fixture.verbs(),
            ["create"],
            "nothing to halt without a session"
        );
    }

    #[test]
    fn invocation_failures_126_and_127_are_failures_not_success() {
        for code in [126, 127] {
            let fixture = Fixture::new();
            fixture.respond("create", Some(&format!("{SESSION}\n")), None, Some(code));
            let message = fixture.start(None).unwrap_err().to_string();
            assert!(
                message.contains(&format!("(exit {code})")),
                "exit {code} was not named: {message}"
            );
            assert_eq!(fixture.record()["lifecycle"], "launch_failed");
            // A printed ID after a failed exit is kept so stop can still halt a possible session.
            assert_eq!(fixture.record()["session_value"], SESSION);
        }
    }

    #[test]
    fn unrecognizable_create_output_is_refused_without_a_tab() {
        let fixture = Fixture::new();
        fixture.respond("create", Some("created!\nsee the inbox\n"), None, None);
        let message = fixture.start(None).unwrap_err().to_string();
        assert!(message.contains("no recognizable session ID"));
        assert!(message.contains("agentcloudctl list"));
        assert!(fixture.herdr.panes.lock().unwrap().is_empty());
    }

    #[test]
    fn unverified_create_keeps_the_session_attached_but_fails_loudly() {
        let fixture = Fixture::new();
        fixture.respond(
            "create",
            Some(&format!("{SESSION}\n")),
            Some("node clause did not land on the attach read\n"),
            Some(9),
        );
        let message = fixture.start(Some("task")).unwrap_err().to_string();
        assert!(message.contains("UNVERIFIED"));
        assert!(message.contains("brief was NOT queued"));
        assert!(message.contains("Do not start it again"));
        let record = fixture.record();
        assert_eq!(record["lifecycle"], "running");
        assert_eq!(record["agentcloud"]["verified"], false);
        assert_eq!(fixture.herdr.commands.lock().unwrap().len(), 1);
    }

    #[test]
    fn terminal_attach_failure_keeps_the_live_session_stoppable() {
        let fixture = Fixture::new();
        fixture.herdr.fail_command.store(true, Ordering::Relaxed);
        let message = fixture.start(None).unwrap_err().to_string();
        assert!(message.contains("agentterm did not attach"));
        assert!(message.contains(&format!("agentterm -s {SESSION}")));
        assert_eq!(fixture.record()["lifecycle"], "launch_failed");

        let stopped = fixture.manager().stop("sub-worker").unwrap();
        assert_eq!(stopped["session_halted"], true);
        assert_eq!(stopped["pane_closed"], true);
        assert_eq!(fixture.verbs(), ["create", "halt", "archive"]);
    }

    #[test]
    fn goal_briefs_are_sent_after_create_instead_of_as_a_prompt() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(Some("/goal finish the migration")).unwrap();
        let calls = fixture.calls();
        assert!(!calls[0].contains(&"--prompt".to_owned()));
        assert_eq!(calls[1][0], "send");
        assert_eq!(calls[1][4], "/goal finish the migration");
        assert!(!calls[1].contains(&"--end-of-turn".to_owned()));
    }

    #[test]
    fn missing_tools_are_refused_before_registry_state() {
        let fixture = Fixture::new();
        let manager = ManagedAgents::new(&fixture.herdr, &fixture.root.join("registry"))
            .unwrap()
            .with_cloud_tools(CloudTools {
                agentcloudctl: fixture.root.join("tools/absent-agentcloudctl"),
                agentterm: fixture.root.join("tools/agentterm"),
                caller_session: None,
            });
        let message = manager
            .start("sub-worker", &fixture.root, fixture.options(None))
            .unwrap_err()
            .to_string();
        assert!(message.contains("--agentcloudctl-bin"));
        assert!(!fixture.root.join("registry/sub-worker").exists());
    }

    #[test]
    fn launch_settings_refuse_contradictions_and_reserved_arguments() {
        let fixture = Fixture::new();
        for (launch, expected) in [
            (
                CloudLaunch {
                    envspec: Some("monorepo".to_owned()),
                    ..CloudLaunch::default()
                },
                "without provision",
            ),
            (
                CloudLaunch {
                    provision: true,
                    node_id: Some("node-a".to_owned()),
                    ..CloudLaunch::default()
                },
                "choose one",
            ),
            (
                CloudLaunch {
                    workspace: Some("relative/dir".to_owned()),
                    ..CloudLaunch::default()
                },
                "absolute path",
            ),
            (
                CloudLaunch {
                    harness: Some("claude".to_owned()),
                    ..CloudLaunch::default()
                },
                "claude-code",
            ),
        ] {
            let mut options = fixture.options(None);
            options.cloud = Some(launch);
            let message = fixture
                .manager()
                .start("sub-worker", &fixture.root, options)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains(expected),
                "{expected:?} not in {message:?}"
            );
        }
        for (argv, expected) in [
            (strings(&["--prompt=hi"]), "--prompt"),
            (strings(&["--ws-url=ws://elsewhere"]), "--ws-url"),
            (strings(&["--skill", "builder"]), "--option=value"),
            (strings(&["-p"]), "long option"),
        ] {
            let mut options = fixture.options(None);
            options.harness_args = argv;
            let message = fixture
                .manager()
                .start("sub-worker", &fixture.root, options)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains(expected),
                "{expected:?} not in {message:?}"
            );
        }
        let mut options = fixture.options(None);
        options.resume = Some(SESSION.to_owned());
        assert!(fixture
            .manager()
            .start("sub-worker", &fixture.root, options)
            .unwrap_err()
            .to_string()
            .contains("agentterm -s SESSION_ID"));
        assert!(fixture.calls().is_empty());
        assert!(!fixture.root.join("registry/sub-worker").exists());

        let mut options = fixture.options(None);
        options.harness_args = strings(&["--skill=builder", "--narration"]);
        fixture.list_row(false);
        fixture
            .manager()
            .start("sub-worker", &fixture.root, options)
            .unwrap();
        let create = &fixture.calls()[0];
        assert_eq!(
            &create[create.len() - 2..],
            ["--skill=builder", "--narration"]
        );
    }

    #[test]
    fn send_journals_after_the_turn_with_an_idempotency_key() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        let sent = fixture
            .manager()
            .send_cloud("sub-worker", "follow-up", Some("review-1"))
            .unwrap();
        assert_eq!(sent["outcome"], "journaled");
        assert_eq!(sent["message_id"], "review-1");
        assert_eq!(sent["end_of_turn"], true);
        assert_eq!(sent["delivery"], "send");
        let send = fixture.calls().pop().unwrap();
        assert_eq!(
            send,
            strings(&[
                "send",
                "--session",
                SESSION,
                "--text",
                "follow-up",
                "--idempotency-key",
                "review-1",
                "--end-of-turn",
            ])
        );

        let generated = fixture
            .manager()
            .send_cloud("sub-worker", "another", None)
            .unwrap();
        assert!(generated["message_id"]
            .as_str()
            .unwrap()
            .starts_with("agentctl-sub-worker-"));

        fixture.respond("send", None, Some("transport lost\n"), Some(3));
        let message = fixture
            .manager()
            .send_cloud("sub-worker", "retry me", Some("retry-1"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("exited 3: transport lost"));
        assert!(message.contains("--message-id retry-1"));

        fixture.manager().pause("sub-worker", true).unwrap();
        let before = fixture.calls().len();
        assert!(fixture
            .manager()
            .send_cloud("sub-worker", "blocked", None)
            .unwrap_err()
            .to_string()
            .contains("paused"));
        assert_eq!(
            fixture.calls().len(),
            before,
            "a paused agent is not contacted"
        );
    }

    #[test]
    fn send_from_inside_another_session_uses_the_attributed_message() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond("send-message", Some("msg-7\n"), None, None);
        let coordinator = "c0ffee00-0000-4000-8000-000000000001";
        let manager = ManagedAgents::new(&fixture.herdr, &fixture.root.join("registry"))
            .unwrap()
            .with_cloud_tools(CloudTools {
                caller_session: Some(coordinator.to_owned()),
                ..fixture.tools()
            });
        let sent = manager
            .send_cloud("sub-worker", "follow-up", Some("review-2"))
            .unwrap();
        assert_eq!(sent["delivery"], "send-message");
        assert_eq!(sent["from_session"], coordinator);
        assert_eq!(sent["end_of_turn"], Value::Null);
        assert_eq!(
            fixture.calls().pop().unwrap(),
            strings(&[
                "send-message",
                "--session",
                coordinator,
                "--to",
                SESSION,
                "--body",
                "follow-up",
                "--idempotency-key",
                "review-2",
            ])
        );

        // A worker addressing its own session keeps the plain send.
        let own = ManagedAgents::new(&fixture.herdr, &fixture.root.join("registry"))
            .unwrap()
            .with_cloud_tools(CloudTools {
                caller_session: Some(SESSION.to_owned()),
                ..fixture.tools()
            });
        assert_eq!(
            own.send_cloud("sub-worker", "self", None).unwrap()["delivery"],
            "send"
        );

        fixture.respond("send-message", None, Some("not same owner\n"), Some(1));
        let message = manager
            .send_cloud("sub-worker", "again", Some("review-3"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("agentcloudctl send-message exited 1: not same owner"));
        assert!(message.contains("--message-id review-3"));
    }

    #[test]
    fn status_reports_missing_listing_rows_and_detached_terminals_truthfully() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond("list", Some("[]"), None, None);
        fixture.herdr.detached.store(true, Ordering::Relaxed);
        let status = fixture.manager().status("sub-worker").unwrap();
        assert_eq!(status["agent_status"], "unknown");
        assert!(status["probe_error"]
            .as_str()
            .unwrap()
            .contains("lagged projection"));
        assert_eq!(status["terminal"]["agentterm_attached"], false);

        fixture.respond("list", None, Some("unauthenticated\n"), Some(1));
        let status = fixture.manager().status("sub-worker").unwrap();
        assert!(status["probe_error"]
            .as_str()
            .unwrap()
            .contains("agentcloudctl list exited 1: unauthenticated"));

        fixture.list_row(false);
        let listed = fixture.manager().list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["agent_status"], "idle");

        fixture.respond(
            "list",
            Some(
                &json!([{"session_id": SESSION, "running": false, "activity": "waiting"}])
                    .to_string(),
            ),
            None,
            None,
        );
        let status = fixture.manager().status("sub-worker").unwrap();
        assert_eq!(status["agent_status"], "waiting");
    }

    #[test]
    fn wait_maps_settle_exit_codes_to_readiness_and_failures() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond("wait", Some("completed\n"), None, Some(0));
        let status = fixture
            .manager()
            .wait("sub-worker", Duration::from_secs(90))
            .unwrap();
        assert_eq!(status["cloud_outcome"], "completed");
        let wait = fixture
            .calls()
            .into_iter()
            .find(|call| call[0] == "wait")
            .unwrap();
        assert_eq!(
            wait,
            strings(&[
                "wait",
                "--session",
                SESSION,
                "--until",
                "settle",
                "--timeout-ms",
                "90000"
            ])
        );
        for (code, expected) in [
            (1, "without completing"),
            (2, "interrupted"),
            (3, "outcome is unknown"),
            (127, "(exit 127)"),
        ] {
            fixture.respond("wait", Some("failed\n"), None, Some(code));
            let message = fixture
                .manager()
                .wait("sub-worker", Duration::from_secs(1))
                .unwrap_err()
                .to_string();
            assert!(
                message.contains(expected),
                "{expected:?} not in {message:?}"
            );
        }
    }

    #[test]
    fn read_last_returns_the_durable_reply_and_tail_reads_the_terminal() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond("output", Some("DONE\n"), None, None);
        let manager = fixture.manager();
        assert_eq!(
            manager.read_cloud("sub-worker", 50, "last").unwrap(),
            "DONE\n"
        );
        let snapshot =
            agent::read_private_json(&fixture.root.join("registry/sub-worker/output.json"))
                .unwrap();
        assert_eq!(snapshot["text"], "DONE\n");
        assert_eq!(
            manager.read_cloud("sub-worker", 50, "tail").unwrap(),
            "agentterm screen"
        );
        assert!(manager
            .read_cloud("sub-worker", 50, "since_turn")
            .unwrap_err()
            .to_string()
            .contains("since_turn"));
        fixture.herdr.panes.lock().unwrap().clear();
        assert!(manager
            .read_cloud("sub-worker", 50, "tail")
            .unwrap_err()
            .to_string()
            .contains("--output last"));
    }

    #[test]
    fn stop_halts_then_closes_the_recorded_pane_and_archives_both() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        let stopped = fixture.manager().stop("sub-worker").unwrap();
        assert_eq!(stopped["session_halted"], true);
        assert_eq!(stopped["session_archived"], true);
        assert_eq!(stopped["pane_closed"], true);
        assert_eq!(stopped["tab_closed"], true);
        assert!(stopped["node_lease"]
            .as_str()
            .unwrap()
            .contains("stop does not release the node reservation"));
        assert_eq!(*fixture.herdr.closed.lock().unwrap(), ["cloud-pane"]);
        let calls = fixture.calls();
        let halt = calls.iter().find(|call| call[0] == "halt").unwrap();
        assert_eq!(
            *halt,
            strings(&[
                "halt",
                "--session",
                SESSION,
                "--reason",
                "agentctl stop sub-worker"
            ])
        );
        assert!(calls
            .iter()
            .any(|call| *call == strings(&["archive", "--session", SESSION])));
        assert!(!fixture.root.join("registry/sub-worker").exists());
        let archived = stopped["archive"].as_str().unwrap();
        let record = agent::read_private_json(&Path::new(archived).join("agent.json")).unwrap();
        assert_eq!(record["lifecycle"], "stopped");
        assert_eq!(record["agentcloud"]["halted"], true);
    }

    #[test]
    fn stop_refuses_to_archive_when_halt_fails_and_offers_the_explicit_skip() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond("halt", None, Some("unknown session\n"), Some(1));
        let message = fixture
            .manager()
            .stop("sub-worker")
            .unwrap_err()
            .to_string();
        assert!(message.contains("exited 1: unknown session"));
        assert!(message.contains("--skip-cloud-halt"));
        assert!(fixture.herdr.closed.lock().unwrap().is_empty());
        assert!(fixture.root.join("registry/sub-worker/agent.json").exists());

        let stopped = fixture
            .manager()
            .stop_with_options(
                "sub-worker",
                StopOptions {
                    skip_cloud_halt: true,
                    ..StopOptions::default()
                },
            )
            .unwrap();
        assert_eq!(stopped["session_halted"], false);
        assert_eq!(stopped["session_archived"], false);
        assert_eq!(stopped["pane_closed"], true);
        assert_eq!(
            fixture
                .verbs()
                .iter()
                .filter(|verb| *verb == "archive")
                .count(),
            0
        );
    }

    #[test]
    fn stop_reports_a_failed_session_archive_without_hiding_the_halt() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        fixture.respond(
            "archive",
            None,
            Some("archive_v1 not advertised\n"),
            Some(1),
        );
        let stopped = fixture.manager().stop("sub-worker").unwrap();
        assert_eq!(stopped["session_halted"], true);
        assert_eq!(stopped["session_archived"], false);
        assert!(stopped["session_archive_error"]
            .as_str()
            .unwrap()
            .contains("archive_v1 not advertised"));
    }

    #[test]
    fn herdr_only_operations_redirect_to_the_cloud_interface() {
        let fixture = Fixture::new();
        fixture.list_row(false);
        fixture.start(None).unwrap();
        let manager = fixture.manager();
        for message in [
            manager
                .drain("sub-worker", DrainOptions::default())
                .unwrap_err()
                .to_string(),
            manager
                .goal(
                    "sub-worker",
                    Some("objective"),
                    DrainOptions::default(),
                    None,
                )
                .unwrap_err()
                .to_string(),
            manager
                .send("sub-worker", "via herdr", DrainOptions::default())
                .unwrap_err()
                .to_string(),
        ] {
            assert!(message.contains("agentcloud session"), "{message}");
            assert!(message.contains("agentctl send"), "{message}");
        }
        let attached = manager.attach("sub-worker").unwrap();
        assert_eq!(attached["pane_id"], "cloud-pane");
        assert_eq!(*fixture.herdr.focused.lock().unwrap(), ["cloud-pane"]);
    }

    #[test]
    fn project_workspace_policy_places_and_guards_the_terminal_tab() {
        let fixture = Fixture::new();
        let mut options = fixture.options(None);
        options.workspace_id = Some("workspace".to_owned());
        let message = fixture
            .manager()
            .with_project_workspace(Some("project-agents"))
            .start("sub-worker", &fixture.root, options)
            .unwrap_err()
            .to_string();
        assert!(message.contains("project configuration requires \"project-agents\""));
        assert!(
            fixture.calls().is_empty(),
            "no session is created on a policy refusal"
        );
        assert!(!fixture.root.join("registry/sub-worker").exists());

        fixture.list_row(false);
        let mut options = fixture.options(None);
        options.workspace_id = None;
        fixture
            .manager()
            .with_project_workspace(Some("project-agents"))
            .start("sub-worker", &fixture.root, options)
            .unwrap();
        assert_eq!(*fixture.herdr.labels.lock().unwrap(), ["project-agents"]);
    }

    #[test]
    fn session_ids_are_single_opaque_tokens() {
        assert_eq!(
            parse_session_id(&format!("  {SESSION}\n")).as_deref(),
            Some(SESSION)
        );
        for invalid in ["", "\n", "two\nlines", "has space", "-leading", "a\0b"] {
            assert!(parse_session_id(invalid).is_none(), "{invalid:?}");
        }
    }
}
