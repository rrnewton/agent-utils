//! The always-on one-shot recovery shepherd (`#1559`, Phase 2): after a
//! boot, it judges recovery from `team manifest` facts with stateless
//! one-shot agent calls (`run-agent-prompt`: muse, then claude, then
//! codex) and effects only through `team act`.
//!
//! One incident per restore epoch (kernel boot plus user-manager
//! identity, the epoch the restore replay guard uses). Inside the
//! incident it loops bounded rounds until every judged team is a
//! deterministic YES (lead running, bridge healthy, handoff acked —
//! each an explicit pass; held/skipped never count) or bounded give-up
//! escalates and stamps. A mid-session death is not its job: watch,
//! `team kick` and the driver own that; the shepherd re-enters only on
//! a new epoch or an explicit manual run, which resumes or no-ops
//! against the existing stamp.
//!
//! Two restorers, mechanised from the team-restore skill's step 0: a
//! team whose lead is live is judged read-only and never acted on, and
//! only the invocation's own registry (the `--registry` team) is ever
//! acted on — every team in `--config` is judged read-only. The
//! shepherd state flock is held only for the constant-time load/save
//! of its small state and stamp files, never across a manifest build,
//! a restore, a one-shot call, or a `team act` wait.
//!
//! Persisted files live only under the shepherd state directory
//! (`<state root>/shepherd/<team-key>/<boot-id>-<epoch-tag>/`): `state.json` (resumable round
//! bookkeeping) and `stamp.json` (the terminal verdict). No older
//! reader opens either path, and no driver record field changes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent::AgentError;
use crate::restore::{Host, RestoreConfig, RestoreEpoch};
use crate::subagents::ManagedApi;
use crate::team_manifest::manifest_team_with_verify;
use crate::team_verify::{explicit_yes, VerifyOptions};

/// Schema of the terminal stamp document.
pub const SHEPHERD_STAMP_SCHEMA: &str = "agentctl-team-shepherd-stamp/v1";
/// Schema of the resumable per-incident state document.
pub const SHEPHERD_STATE_SCHEMA: &str = "agentctl-team-shepherd-state/v1";
/// Schema of the `team shepherd` run report printed on stdout.
pub const SHEPHERD_REPORT_SCHEMA: &str = "agentctl-team-shepherd/v1";

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// Bounds and wiring for one shepherd run, from the CLI flags of the
/// same names (defaults are the approved phase-plan bounds).
#[derive(Clone, Debug)]
pub struct ShepherdOptions {
    /// Agentctl state root (default: `$XDG_STATE_HOME/agentctl`, else
    /// `~/.local/state/agentctl`).
    pub state_dir: Option<PathBuf>,
    /// Maximum judgement rounds in one incident.
    pub max_rounds: u32,
    /// Maximum wall minutes in one incident.
    pub max_minutes: u64,
    /// Hard seconds budget for one `run-agent-prompt` chain.
    pub call_timeout_seconds: u64,
    /// Seconds between rounds.
    pub round_delay_seconds: u64,
    /// Consecutive identical-manifest rounds that escalate early.
    pub no_progress_rounds: u32,
    /// The `run-agent-prompt` executable.
    pub run_agent_prompt: PathBuf,
    /// Ordered recovery profiles file, when the team configured one.
    pub profile_file: Option<PathBuf>,
    /// Incident claim file to heartbeat while running, when launched
    /// through `start-recovery`.
    pub claim_file: Option<PathBuf>,
    /// Seconds before a dead claim with no heartbeat counts as stale.
    pub stale_seconds: f64,
    /// Minutes a restore may run before the wedged escalation is
    /// enqueued (the loop keeps watching past it).
    pub restore_bound_minutes: u64,
    /// Escalation route file for pages (default: the synced
    /// escalation.toml; never the default under a test harness).
    pub escalation_config: Option<PathBuf>,
    /// Seconds a `team act` execution may wait for its receipt.
    pub act_wait_seconds: u64,
}

impl Default for ShepherdOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            max_rounds: 20,
            max_minutes: 120,
            call_timeout_seconds: 300,
            round_delay_seconds: 30,
            no_progress_rounds: 2,
            run_agent_prompt: PathBuf::from("run-agent-prompt"),
            profile_file: None,
            claim_file: None,
            stale_seconds: 180.0,
            restore_bound_minutes: 60,
            escalation_config: None,
            act_wait_seconds: 60,
        }
    }
}

/// Resumable bookkeeping for one incident, persisted between rounds so
/// a restarted shepherd resumes instead of repeating effects.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ShepherdState {
    /// Always [`SHEPHERD_STATE_SCHEMA`].
    pub schema: String,
    /// Kernel boot this incident belongs to.
    pub boot_id: String,
    /// Owning team's incident id (12 hex), once known.
    pub incident: String,
    /// Rounds completed so far.
    pub round: u32,
    /// SHA-256 of the owning team's last manifest, for no-progress.
    pub last_manifest_hash: String,
    /// Consecutive rounds with an identical owning-team manifest.
    pub no_progress: u32,
    /// The deterministic restore arm already ran for this incident.
    pub restore_done: bool,
    /// The owned restore child's pid (0 = none), recorded so a
    /// restarted supervisor re-adopts the running restore instead of
    /// starting a second one.
    #[serde(default)]
    pub restore_pid: u32,
    /// `/proc` start ticks of `restore_pid`, verifying the record.
    #[serde(default)]
    pub restore_pid_start: String,
    /// Unix seconds the restore child started (wedged bound).
    #[serde(default)]
    pub restore_started_at: f64,
    /// The wedged-restore escalation was already enqueued.
    #[serde(default)]
    pub wedged_escalated: bool,
    /// Verdict kinds already paged this incident (at most one
    /// delivered page per kind).
    #[serde(default)]
    pub page_delivered: Vec<String>,
    /// Page delivery attempts this incident (retries only follow
    /// failures).
    #[serde(default)]
    pub page_attempts: u32,
    /// One grace round after restore finishes before no-progress
    /// counting resumes.
    #[serde(default)]
    pub restore_settle: bool,
    /// Harness that answered the latest one-shot (`none` until one does).
    pub judge: String,
}

/// Facts the loop gathered for one team in one round.
struct TeamRound {
    registry: PathBuf,
    manifest: Value,
    yes: bool,
    lead_live: bool,
    facts_unknown: bool,
}

/// The shepherd state directory for one team's one incident:
/// `<state root>/shepherd/<team-key>/<boot-id>-<epoch-tag>/` (B1: a
/// host's teams never share a stamp or state file, and a user-manager
/// restart begins a new epoch in the same boot).
pub fn shepherd_dir(state_dir: Option<&Path>, registry: &Path, epoch_key: &str) -> Option<PathBuf> {
    Some(
        state_root(state_dir)?
            .join("shepherd")
            .join(crate::restore::team_key(registry))
            .join(epoch_key.replace('/', "-")),
    )
}

fn state_root(state_dir: Option<&Path>) -> Option<PathBuf> {
    match state_dir {
        Some(dir) => Some(dir.to_path_buf()),
        None => {
            crate::restore::default_report_dir().and_then(|dir| dir.parent().map(Path::to_path_buf))
        }
    }
}

/// Whether start-recovery's restore child is still running, from
/// the claim's recorded restore pid plus start ticks.
pub fn restore_running(claim_path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(claim_path) else {
        return false;
    };
    let Ok(claim) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(pid) = claim["restore_pid"].as_u64() else {
        return false;
    };
    match (claim["restore_pid_start"].as_str(), pid_start_ticks(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => Path::new(&format!("/proc/{pid}")).exists(),
    }
}

/// The canonical epoch identity for claims and state directories:
/// the boot id plus the user manager's start time, one derivation
/// whatever format the manager identity was read in.
pub fn epoch_identity(host: &dyn Host, epoch: &RestoreEpoch) -> String {
    match (epoch.user_manager.as_deref(), host.user_manager_started()) {
        (Some(_), Some(started)) => format!("{}@{started:.0}", epoch.boot_id),
        (Some(manager), None) => {
            let digest = format!("{:x}", Sha256::digest(manager.as_bytes()));
            format!("{}@{}", epoch.boot_id, &digest[..8])
        }
        _ => format!("{}@nomgr", epoch.boot_id),
    }
}

/// SHA-256 of a manifest's canonical JSON text.
pub fn manifest_hash(manifest: &Value) -> String {
    let text = serde_json::to_string(manifest).unwrap_or_default();
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// The progress hash: the manifest with volatile fields (`checked_at`
/// timestamps and other per-build clock readings) removed, so two
/// rounds over unchanged facts hash identically (S2).
pub fn progress_hash(manifest: &Value) -> String {
    fn strip(value: &mut Value) {
        match value {
            Value::Object(map) => {
                // checked_at clocks and evidence prose (which
                // embeds ages) are volatile; check identities,
                // statuses and agent facts are the progress signal.
                map.retain(|key, _| key != "checked_at" && key != "evidence");
                for inner in map.values_mut() {
                    strip(inner);
                }
            }
            Value::Array(items) => {
                for inner in items {
                    strip(inner);
                }
            }
            _ => {}
        }
    }
    let mut projected = manifest.clone();
    strip(&mut projected);
    manifest_hash(&projected)
}

/// The proposal a one-shot answer carries: its last JSON line with an
/// `action` field. Anything else is judgement prose, not a proposal.
pub fn parse_proposal(answer: &str) -> Option<Value> {
    answer.lines().rev().find_map(|line| {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        value.get("action")?.is_string().then_some(value)
    })
}

/// The `team act` argv (after `team act`) for a validated proposal, or
/// the refusal reason. Only allowlisted actions with their required
/// fields survive; a session gap's candidates stay report-only, so no
/// proposal can bind a session or rewrite a record.
pub fn proposal_argv(proposal: &Value) -> std::result::Result<Vec<String>, String> {
    let action = proposal["action"].as_str().unwrap_or_default();
    let field = |name: &str| -> std::result::Result<String, String> {
        proposal[name]
            .as_str()
            .filter(|value| !value.is_empty() && !value.starts_with('-'))
            .map(str::to_owned)
            .ok_or_else(|| format!("proposal {action} missing or invalid {name}"))
    };
    let argv = match action {
        "facts" => vec!["facts".to_owned()],
        "revive" => vec!["revive".to_owned(), field("agent")?],
        "start-unit" => vec!["start-unit".to_owned(), field("unit")?],
        "reset-unit" => vec!["reset-unit".to_owned(), field("unit")?],
        "drain" => vec!["drain".to_owned(), field("agent")?],
        "declare-done" => vec!["declare-done".to_owned()],
        "escalate" => {
            let text = proposal["text"]
                .as_str()
                .filter(|text| !text.is_empty() && !text.starts_with('-'))
                .unwrap_or("recovery needs a human decision");
            vec!["escalate".to_owned(), text.to_owned()]
        }
        "kill-pid" => vec![
            "kill-pid".to_owned(),
            field("pid")?,
            "--expect-cmdline".to_owned(),
            field("expect_cmdline")?,
            "--expect-start".to_owned(),
            field("expect_start")?,
            "--boot-id".to_owned(),
            field("boot_id")?,
        ],
        other => return Err(format!("proposal action {other:?} not allowlisted")),
    };
    Ok(argv)
}

/// Run one bounded subprocess, killing it past `timeout`. Stdout and
/// stderr are returned on success or failure alike; the exit code
/// rides along.
fn run_bounded(
    program: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<(i32, String, String)> {
    use std::io::Read;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| fail(format!("cannot run {}: {error}", program.display())))?;
    // Drain both pipes while the child runs, so a large report can
    // never block it on a full pipe.
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let out_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = out_pipe.read_to_string(&mut text);
        text
    });
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let err_thread = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = err_pipe.read_to_string(&mut text);
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Ok(None) => {
                // SIGTERM first (the wrapper cleans up its own
                // process group on TERM), grace, then SIGKILL.
                let _ = Command::new("kill").arg(child.id().to_string()).status();
                let grace = Instant::now();
                let mut waited: Option<std::process::ExitStatus> = None;
                while waited.is_none() {
                    match child.try_wait() {
                        Ok(Some(status)) => waited = Some(status),
                        Ok(None) if grace.elapsed() < Duration::from_secs(2) => {
                            std::thread::sleep(Duration::from_millis(50));
                        }
                        Ok(None) => {
                            let _ = child.kill();
                            waited = child.wait().ok();
                        }
                        Err(error) => {
                            return Err(fail(format!("cannot wait on a subprocess: {error}")))
                        }
                    }
                }
                break waited.expect("timeout arm produced a status");
            }
            Err(error) => return Err(fail(format!("cannot wait on a subprocess: {error}"))),
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok((status.code().unwrap_or(1), stdout, stderr))
}

/// Which profile answered, from the wrapper's stderr attempt lines
/// (`run-agent-prompt: profile=NAME harness=muse status=answered ...`).
/// Without profiles, the harness name is the judge identity.
pub fn judge_of(stderr: &str) -> Option<String> {
    stderr.lines().find_map(|line| {
        if !line.contains("status=answered") {
            return None;
        }
        let word = |prefix: &str| {
            line.split_whitespace()
                .find_map(|word| word.strip_prefix(prefix))
                .map(str::to_owned)
        };
        word("profile=").or_else(|| word("harness="))
    })
}

/// Hold the incident flock only for one load or save of the small
/// state/stamp files: constant-time under the lock, never across a
/// manifest build, restore, one-shot call, or `team act` wait.
fn lock_state(dir: &Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("shepherd.lock"))
        .map_err(|error| fail(format!("cannot open the shepherd lock: {error}")))?;
    file.lock_exclusive()
        .map_err(|error| fail(format!("cannot take the shepherd lock: {error}")))?;
    Ok(file)
}

fn load_state(dir: &Path) -> ShepherdState {
    let Ok(text) = std::fs::read_to_string(dir.join("state.json")) else {
        return ShepherdState::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn save_state(dir: &Path, state: &ShepherdState) -> Result<()> {
    let _lock = lock_state(dir)?;
    let value = serde_json::to_value(state).map_err(|error| fail(error.to_string()))?;
    crate::agent::atomic_json(&dir.join("state.json"), &value)
        .map_err(|error| fail(error.to_string()))
}

fn load_stamp(dir: &Path) -> Option<Value> {
    std::fs::read_to_string(dir.join("stamp.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

fn save_stamp(dir: &Path, stamp: &Value) -> Result<()> {
    let _lock = lock_state(dir)?;
    crate::agent::atomic_json(&dir.join("stamp.json"), stamp)
        .map_err(|error| fail(error.to_string()))
}

fn append_log(dir: &Path, line: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("shepherd.log"))
    {
        let _ = writeln!(file, "{line}");
    }
}

/// Whether bounded give-up may enqueue an escalation: never when
/// the team is verified, its lead is live (B4), or the claim was
/// lost. Unknown facts DO page (NS4 remainder) — through the
/// enqueue-only path, so paging never kicks a driver.
pub fn should_escalate(yes: bool, lead_live: bool, _facts_unknown: bool, claim_lost: bool) -> bool {
    !yes && !lead_live && !claim_lost
}

/// Whether the team's verified lead is live this round: the
/// desired-live row for the coordinator explicitly passes. A live lead
/// owns recovery (skill step 0), so the shepherd only judges it.
fn lead_live(verify: &Value, coordinator: &str) -> bool {
    verify["checks"].as_array().is_some_and(|checks| {
        checks.iter().any(|row| {
            row["name"] == "desired-live"
                && row["subject"] == coordinator
                && row["status"] == "pass"
        })
    })
}

/// The brief for one one-shot call: instructions, the owning team's
/// manifest (allowlist facts only, never raw records), and one-line
/// read-only verdicts for every judged team.
fn brief(own: &TeamRound, judged: &[TeamRound], round: u32) -> String {
    let mut lines = vec![
        "You are a stateless recovery shepherd in emergency recovery mode. You have no tools."
            .to_owned(),
        "Judge the facts below. Effects happen only through the shepherd, never by you.".to_owned(),
        format!("Round {round}. Owning team manifest:"),
        serde_json::to_string(&own.manifest).unwrap_or_default(),
        "Other teams (read-only, never act on them):".to_owned(),
    ];
    for team in judged {
        lines.push(format!(
            "- {}: YES={} lead_live={}",
            team.registry.display(),
            team.yes,
            team.lead_live
        ));
    }
    lines.push(
        "Reply with a short judgement, then a FINAL line that is one JSON object: \
         {\"action\": \"facts\"} or {\"action\": \"revive\", \"agent\": NAME} or \
         {\"action\": \"start-unit\", \"unit\": UNIT} or {\"action\": \"reset-unit\", \"unit\": UNIT} or \
         {\"action\": \"drain\", \"agent\": NAME} or {\"action\": \"declare-done\"} or \
         {\"action\": \"escalate\", \"text\": WHY}. Propose at most one action, for the owning team only."
            .to_owned(),
    );
    lines.join("\n")
}

/// Inputs the CLI resolved once per run.
pub struct ShepherdInputs {
    /// This run's own executable, for `restore` and `team act`.
    pub agentctl: PathBuf,
    /// The Herdr binary every subprocess and client uses.
    pub herdr_bin: PathBuf,
}

/// Whether a recorded restore (pid plus start ticks) is still the
/// live process. A zombie of our own child is reaped by `try_wait`
/// before this is consulted for adopted records; for an adopted pid
/// the state field reads Z from /proc.
pub fn recorded_restore_alive(pid: u32, start: &str) -> bool {
    if pid == 0 {
        return false;
    }
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    if stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().next() == Some("Z"))
        .unwrap_or(false)
    {
        return false;
    }
    pid_start_ticks(u64::from(pid)).is_some_and(|current| current == start)
}

/// Spawn the restore child in its own process group (its own
/// session scope), so a supervisor stop cannot take it down, with
/// output appended to the incident's restore log.
pub fn spawn_restore(agentctl: &Path, args: &[String], dir: &Path) -> Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("restore.log"))
        .map_err(|error| fail(format!("cannot open the restore log: {error}")))?;
    let err = log
        .try_clone()
        .map_err(|error| fail(format!("cannot clone the restore log: {error}")))?;
    Command::new(agentctl)
        .args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(err))
        .spawn()
        .map_err(|error| fail(format!("cannot launch restore: {error}")))
}

#[allow(clippy::too_many_arguments)]
/// Enqueue one shepherd act (never kick, never run a driver): the
/// shepherd's only effect path. Returns the child exit code.
pub fn enqueue_shepherd_act(
    inputs: &ShepherdInputs,
    own: &Path,
    options: &ShepherdOptions,
    incident: &str,
    request_id: &str,
    action_argv: &[String],
    dir: &Path,
    round: u32,
) -> i32 {
    let mut args = vec![
        "--registry".to_owned(),
        own.display().to_string(),
        "--herdr-bin".to_owned(),
        inputs.herdr_bin.display().to_string(),
        "team".to_owned(),
        "act".to_owned(),
        "--enqueue-only".to_owned(),
        "--wait-seconds".to_owned(),
        "0".to_owned(),
    ];
    if let Some(state) = &options.state_dir {
        args.extend(["--state-dir".to_owned(), state.display().to_string()]);
    }
    args.extend([
        "--incident".to_owned(),
        incident.to_owned(),
        "--request-id".to_owned(),
        request_id.to_owned(),
    ]);
    args.extend(action_argv.iter().cloned());
    match run_bounded(
        &inputs.agentctl,
        &args,
        Duration::from_secs(options.max_minutes.max(1) * 60),
    ) {
        Ok((code, _, _)) => code,
        Err(error) => {
            append_log(
                dir,
                &format!("round {round}: team act could not run: {error}"),
            );
            1
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Page the humans through the escalation routes, delivered by the
/// shepherd itself (never through the team driver or lead). At most
/// one delivered page per verdict kind per incident; retries only
/// follow failures. Returns the stamp status string.
pub fn page(
    host: &dyn Host,
    options: &ShepherdOptions,
    state: &mut ShepherdState,
    kind: &str,
    text: &str,
    registry: &Path,
    incident: &str,
    dir: &Path,
) -> String {
    if state.page_delivered.iter().any(|known| known == kind) {
        return "already-delivered".to_owned();
    }
    // Tests must never reach a real route: under a test harness the
    // default config path is refused loudly, not loaded.
    if options.escalation_config.is_none() && std::env::var_os("AGENTCTL_TEST_HARNESS").is_some() {
        append_log(
            dir,
            "page refused: default escalation config under a test harness",
        );
        return "unconfigured-refused-default".to_owned();
    }
    let config = match &options.escalation_config {
        Some(path) => crate::team_escalate::load_escalation_config(path),
        None => match crate::team_escalate::default_escalation_config_path() {
            Some(path) if path.exists() => crate::team_escalate::load_escalation_config(&path),
            _ => Ok(crate::team_escalate::EscalationConfig::default()),
        },
    };
    let config = match config {
        Ok(config) => config,
        Err(error) => return format!("failed:config {error}"),
    };
    if config.routes.is_empty() {
        append_log(dir, "page unconfigured: no escalation routes");
        return "unconfigured".to_owned();
    }
    state.page_attempts += 1;
    let incident_record = crate::team_escalate::Incident {
        host: std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_owned()),
        team: registry.display().to_string(),
        incident: incident.to_owned(),
        text: text.to_owned(),
    };
    match crate::team_escalate::deliver(host, &config, &incident_record) {
        crate::team_escalate::Delivery::Delivered { route_index } => {
            state.page_delivered.push(kind.to_owned());
            format!("delivered:{route_index}")
        }
        crate::team_escalate::Delivery::Unconfigured => "unconfigured".to_owned(),
        crate::team_escalate::Delivery::Failed { detail } => {
            format!("failed:{}", detail.chars().take(120).collect::<String>())
        }
    }
}

/// Run one shepherd incident to its stamp. `own` is the only registry
/// ever acted on; `judged` registries are read-only. Exit semantics
/// ride in the report's `exit_code`: 0 every team YES, 75 gave up with
/// the stamp written, 1 the run itself was refused.
#[allow(clippy::too_many_arguments)]
pub fn run_shepherd<A: ManagedApi + ?Sized>(
    client: &A,
    own: &Path,
    judged: &[PathBuf],
    host: &dyn Host,
    inputs: &ShepherdInputs,
    options: &ShepherdOptions,
) -> Result<Value> {
    crate::team_driver::require_host_namespace(host)?;
    let boot_id = host.boot_id()?;
    let epoch = RestoreEpoch {
        boot_id: boot_id.clone(),
        user_manager: host.user_manager_id(),
    };
    let epoch_key = epoch_identity(host, &epoch);
    let dir = shepherd_dir(options.state_dir.as_deref(), own, &epoch_key)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    agent_queue::fsio::create_private_directory(&dir, "shepherd state directory", true, true)
        .map_err(|error| fail(error.to_string()))?;
    if let Some(stamp) = load_stamp(&dir) {
        // A terminal stamp for this boot: resume means report, never
        // repeat effects.
        return Ok(json!({
            "schema": SHEPHERD_REPORT_SCHEMA,
            "boot_id": boot_id,
            "verdict": stamp["verdict"],
            "judge": stamp["judge"],
            "rounds": stamp["rounds"],
            "stamp": dir.join("stamp.json"),
            "resumed": true,
            "exit_code": if stamp["verdict"] == "YES" { 0 } else { 75 },
        }));
    }
    let incident = crate::team_driver::incident_id(own, &epoch);
    // Own the incident before touching state (B2 nit): a manual
    // `team shepherd` takes the claim itself; a start-recovery
    // supervisor adopts its starter's pending claim. A live foreign
    // claim means report, never act. An env-named claim is trusted
    // only when it is this team's own claim path.
    let default_claim =
        recovery_dir(options.state_dir.as_deref(), own).map(|dir| dir.join("claim.json"));
    let claim_path = match (&options.claim_file, &default_claim) {
        (Some(named), Some(expected))
            if std::fs::canonicalize(named).unwrap_or_else(|_| named.clone())
                == std::fs::canonicalize(expected).unwrap_or_else(|_| expected.clone()) =>
        {
            named.clone()
        }
        _ => match default_claim {
            Some(path) => path,
            None => return Err(fail("cannot determine the recovery claim path")),
        },
    };
    // Children (restore, team act, the wrapper) never inherit the
    // claim pointers.
    std::env::remove_var("AGENTCTL_RECOVERY_CLAIM_FILE");
    std::env::remove_var("AGENTCTL_RECOVERY_RESTORE_LAUNCHED");
    match acquire_claim(
        &claim_path,
        own,
        &incident,
        &epoch,
        &epoch_key,
        options.stale_seconds,
        false,
        ClaimRole::Supervisor,
    )? {
        ClaimOutcome::Owned(_) => {}
        ClaimOutcome::AlreadyRunning => {
            return Ok(json!({
                "schema": SHEPHERD_REPORT_SCHEMA,
                "boot_id": boot_id,
                "verdict": "already-running",
                "incident": incident,
                "claim": claim_path,
                "exit_code": 75,
            }));
        }
    }
    let mut state = load_state(&dir);
    if state.schema.is_empty() {
        state.schema = SHEPHERD_STATE_SCHEMA.to_owned();
        state.boot_id = boot_id.clone();
        state.judge = "none".to_owned();
    }
    state.incident = incident.clone();
    save_state(&dir, &state)?;
    // Heartbeat independently of rounds: rounds can legitimately
    // outrun the stale interval, so a thread refreshes the claim and
    // flags loss of ownership for the loop to stop on.
    let claim_lost = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heartbeat_stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let heartbeat_thread = {
        let claim_path = claim_path.clone();
        let incident = incident.clone();
        let lost = claim_lost.clone();
        let stop = heartbeat_stop.clone();
        let interval = Duration::from_secs_f64((options.stale_seconds / 4.0).clamp(5.0, 60.0));
        std::thread::spawn(move || {
            let mut failures = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(interval);
                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                match heartbeat_claim_result(&claim_path, &incident) {
                    Heartbeat::Owned => failures = 0,
                    // Another owner took the claim: stop at once,
                    // not after retries.
                    Heartbeat::Lost => {
                        lost.store(true, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                    Heartbeat::Transient => {
                        failures += 1;
                        if failures >= 3 {
                            lost.store(true, std::sync::atomic::Ordering::Relaxed);
                            break;
                        }
                    }
                }
            }
        })
    };
    let started = Instant::now();
    let mut verdict = "NO";
    let mut summaries: BTreeMap<String, Value> = BTreeMap::new();
    let mut round = state.round;
    let mut restore_child: Option<std::process::Child> = None;
    while round < options.max_rounds {
        round += 1;
        let mut rounds_teams: Vec<TeamRound> = Vec::new();
        for registry in std::iter::once(own).chain(judged.iter().map(PathBuf::as_path)) {
            // A team's facts error is a fact, never a supervisor
            // abort (S3): a broken peer must not disable own-team
            // recovery, and an own-team error counts toward give-up.
            let gathered = RestoreConfig::load(registry).and_then(|config| {
                let coordinator = config.coordinator.clone().unwrap_or_default();
                manifest_team_with_verify(
                    client,
                    registry,
                    host,
                    &config,
                    &VerifyOptions {
                        state_dir: options.state_dir.clone(),
                        ack_timeout_seconds: 1800,
                        dismiss_allowlisted: false,
                    },
                )
                .map(|(manifest, verify)| (manifest, verify, coordinator))
            });
            match gathered {
                Ok((manifest, verify, coordinator)) => {
                    rounds_teams.push(TeamRound {
                        registry: registry.to_path_buf(),
                        yes: explicit_yes(&verify, &coordinator),
                        lead_live: lead_live(&verify, &coordinator),
                        facts_unknown: false,
                        manifest,
                    });
                }
                Err(error) => {
                    append_log(
                        &dir,
                        &format!(
                            "round {round}: facts error for {}: {error}",
                            registry.display()
                        ),
                    );
                    rounds_teams.push(TeamRound {
                        registry: registry.to_path_buf(),
                        yes: false,
                        lead_live: false,
                        // Unknown is not "lead down" (NS4): no
                        // restore arm, no act, no escalation act.
                        facts_unknown: true,
                        manifest: json!({"schema": crate::team_manifest::MANIFEST_SCHEMA, "error": error.to_string()}),
                    });
                }
            }
        }
        let (own_round, judged_rounds) = rounds_teams.split_first().unwrap();
        for team in &rounds_teams {
            summaries.insert(
                team.registry.display().to_string(),
                json!({"yes": team.yes, "lead_live": team.lead_live, "facts_unknown": team.facts_unknown,
                       "record_gaps": team.manifest["record_gaps"].as_array().map_or(0, Vec::len)}),
            );
        }
        append_log(
            &dir,
            &format!(
                "round {round}: own YES={} lead_live={}",
                own_round.yes, own_round.lead_live
            ),
        );
        // The YES exit is computed here but only taken once the
        // restore child has exited (below, after the poll): a
        // wedged restore after YES still needs its page.
        // Step 0 + the deterministic arm, own team only, and only
        // while its lead is down. The supervisor owns the restore:
        // it spawns the child in its own session (so a supervisor
        // stop cannot take the restore down), records it, and a
        // restarted supervisor re-adopts the running child by that
        // record instead of starting a second one. While the child
        // runs, the loop only watches: no acts, no no-progress
        // counting, and no exit (NB1).
        let own_actionable = !own_round.lead_live && !own_round.facts_unknown;
        // Poll the owned or adopted restore child.
        let mut restore_alive = false;
        if let Some(child) = &mut restore_child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    append_log(&dir, &format!("round {round}: restore exit={status}"));
                    state.restore_done = true;
                    state.restore_settle = true;
                    state.restore_pid = 0;
                    restore_child = None;
                }
                Ok(None) => restore_alive = true,
                Err(_) => restore_child = None,
            }
        } else if state.restore_pid != 0 && !state.restore_done {
            if recorded_restore_alive(state.restore_pid, &state.restore_pid_start) {
                restore_alive = true;
            } else {
                state.restore_done = true;
                state.restore_settle = true;
                state.restore_pid = 0;
            }
        }
        if own_actionable
            && !state.restore_done
            && restore_child.is_none()
            && state.restore_pid == 0
        {
            let base = vec![
                "--registry".to_owned(),
                own.display().to_string(),
                "--herdr-bin".to_owned(),
                inputs.herdr_bin.display().to_string(),
            ];
            let mut dry_args = base.clone();
            dry_args.extend(["restore".to_owned(), "--dry-run".to_owned()]);
            let _ = run_bounded(
                &inputs.agentctl,
                &dry_args,
                Duration::from_secs(options.call_timeout_seconds.max(60)),
            );
            let mut real_args = base;
            real_args.push("restore".to_owned());
            match spawn_restore(&inputs.agentctl, &real_args, &dir) {
                Ok(child) => {
                    state.restore_pid = child.id();
                    state.restore_pid_start =
                        pid_start_ticks(u64::from(child.id())).unwrap_or_default();
                    state.restore_started_at = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d: Duration| d.as_secs_f64())
                        .unwrap_or(0.0);
                    save_state(&dir, &state)?;
                    restore_child = Some(child);
                    restore_alive = true;
                }
                Err(error) => {
                    append_log(
                        &dir,
                        &format!("round {round}: restore could not run: {error}"),
                    );
                    state.restore_done = true;
                }
            }
        }
        // A restore wedged past the bound pages the humans once,
        // through an enqueued escalation only, and keeps watching.
        if restore_alive
            && !state.wedged_escalated
            && state.restore_started_at > 0.0
            && std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d: Duration| d.as_secs_f64())
                .unwrap_or(0.0)
                - state.restore_started_at
                > (options.restore_bound_minutes.max(1) * 60) as f64
        {
            let wedged_status = page(
                host,
                options,
                &mut state,
                "wedged",
                "restore is still running past its bound",
                own,
                &incident,
                &dir,
            );
            append_log(&dir, &format!("round {round}: wedged page {wedged_status}"));
            state.wedged_escalated = true;
            save_state(&dir, &state)?;
        }
        if rounds_teams.iter().all(|team| team.yes) && !restore_alive {
            verdict = "YES";
            state.round = round;
            save_state(&dir, &state)?;
            break;
        }
        // One judgement call per round; at most one own-team act.
        // Fail closed (S1): no profile file, or no current all-pass
        // smoke record for it, means no model is consulted at all;
        // deterministic recovery never waits on judgement.
        let judgement = if own_round.facts_unknown {
            append_log(
                &dir,
                &format!("round {round}: own facts unknown; not judging"),
            );
            None
        } else {
            match &options.profile_file {
                Some(file)
                    if judgement_allowed(
                        options.state_dir.as_deref(),
                        own,
                        file,
                        &options.run_agent_prompt,
                    ) =>
                {
                    Some(file.clone())
                }
                _ => {
                    append_log(
                        &dir,
                        &format!("round {round}: judgement unavailable (smoke)"),
                    );
                    None
                }
            }
        };
        let prompt = brief(own_round, judged_rounds, round);
        let prompt_path = dir.join(format!("prompt-{round}.txt"));
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&prompt_path)
                .map_err(|error| fail(format!("cannot write the round brief: {error}")))?;
            use std::io::Write;
            file.write_all(prompt.as_bytes())
                .map_err(|error| fail(format!("cannot write the round brief: {error}")))?;
        }
        let mut wrapper_args = vec![
            "--prompt-file".to_owned(),
            prompt_path.display().to_string(),
            "--timeout".to_owned(),
            options.call_timeout_seconds.to_string(),
        ];
        if let Some(file) = &judgement {
            wrapper_args.extend(["--profile-file".to_owned(), file.display().to_string()]);
        }
        wrapper_args.extend([
            "--total-timeout".to_owned(),
            (options.call_timeout_seconds * 3).to_string(),
        ]);
        let answer = if judgement.is_none() {
            Err(fail("judgement unavailable"))
        } else {
            run_bounded(
                &options.run_agent_prompt,
                &wrapper_args,
                Duration::from_secs(options.call_timeout_seconds * 3 + 60),
            )
        };
        let mut proposal: Option<Value> = None;
        if let Ok((0, text, log)) = &answer {
            if !text.trim().is_empty() {
                if let Some(judge) = judge_of(log) {
                    state.judge = judge;
                }
                proposal = parse_proposal(text);
            }
        }
        if claim_lost.load(std::sync::atomic::Ordering::Relaxed) {
            append_log(
                &dir,
                &format!("round {round}: claim lost before acting; stopping"),
            );
            break;
        }
        if own_actionable && !restore_alive {
            if let Some(proposal) = &proposal {
                match proposal_argv(proposal) {
                    Ok(action_argv) => {
                        // Enqueue only: the shepherd never kicks or
                        // runs a driver (structural B2/NS3 fix).
                        let code = enqueue_shepherd_act(
                            inputs,
                            own,
                            options,
                            &incident,
                            &format!("shepherd-{incident}-r{round}"),
                            &action_argv,
                            &dir,
                            round,
                        );
                        append_log(
                            &dir,
                            &format!("round {round}: team act {} exit={code}", proposal["action"]),
                        );
                    }
                    Err(reason) => {
                        append_log(&dir, &format!("round {round}: proposal refused: {reason}"));
                    }
                }
            }
        }
        // No-progress bookkeeping on the owning team's manifest.
        // Never count while the own restore is still running, in the
        // first round after it finishes, or while a live lead is
        // mid-handoff (pending ack is progress, NB1): a normal slow
        // boot must not read as no progress.
        let restore_live = restore_alive;
        let hash = progress_hash(&own_round.manifest);
        if restore_live || own_round.lead_live {
            state.no_progress = 0;
            state.last_manifest_hash = hash;
            state.restore_settle = true;
        } else if state.restore_settle {
            state.restore_settle = false;
            state.no_progress = 0;
            state.last_manifest_hash = hash;
        } else if hash == state.last_manifest_hash {
            state.no_progress += 1;
        } else {
            state.no_progress = 0;
            state.last_manifest_hash = hash;
        }
        state.round = round;
        save_state(&dir, &state)?;
        if claim_lost.load(std::sync::atomic::Ordering::Relaxed) {
            append_log(&dir, &format!("round {round}: claim lost; stopping"));
            break;
        }
        let elapsed = started.elapsed() >= Duration::from_secs(options.max_minutes * 60);
        // Every exit is gated on the restore child (NB1): while it
        // runs, bounds do not fire and no stamp is written.
        if !restore_alive && (elapsed || state.no_progress >= options.no_progress_rounds) {
            break;
        }
        if restore_alive && round >= options.max_rounds {
            // Keep watching past the round bound until the child
            // exits; rounds stop counting against the bound.
            round = options.max_rounds - 1;
            state.round = round;
        }
        if round < options.max_rounds && options.round_delay_seconds > 0 {
            std::thread::sleep(Duration::from_secs(options.round_delay_seconds));
        }
    }
    let claim_lost_now = claim_lost.load(std::sync::atomic::Ordering::Relaxed);
    heartbeat_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = heartbeat_thread.join();
    if claim_lost_now {
        // The new owner owns this incident: exit without stamping
        // anything over its state.
        append_log(&dir, "claim lost; exiting without a stamp");
        return Ok(json!({
            "schema": SHEPHERD_REPORT_SCHEMA,
            "boot_id": boot_id,
            "verdict": "claim-lost",
            "judge": state.judge,
            "rounds": state.round,
            "resumed": false,
            "exit_code": 75,
        }));
    }
    let mut escalation = "none".to_owned();
    if verdict != "YES" {
        escalation = page(
            host,
            options,
            &mut state,
            "give-up",
            "shepherd gave up without a deterministic YES",
            own,
            &incident,
            &dir,
        );
        save_state(&dir, &state)?;
    }
    if verdict != "YES" {
        // Bounded give-up: escalate through the landed path only
        // while the own lead is down (B4). A live lead owns recovery
        // under step 0: the stamp records the outcome and no `team
        // act` — which could kick the driver — is ever sent to it.
        let own_row = summaries.get(&own.display().to_string());
        let own_yes = own_row.is_some_and(|row| row["yes"] == json!(true));
        let own_lead_live = own_row.is_some_and(|row| row["lead_live"] == json!(true));
        let own_unknown = own_row.is_some_and(|row| row["facts_unknown"] == json!(true));
        if should_escalate(own_yes, own_lead_live, own_unknown, claim_lost_now) {
            let code = enqueue_shepherd_act(
                inputs,
                own,
                options,
                &incident,
                &format!("shepherd-{incident}-giveup"),
                &[
                    "escalate".to_owned(),
                    "shepherd gave up without a deterministic YES".to_owned(),
                ],
                &dir,
                state.round,
            );
            // The enqueued act is for the team driver (drained from
            // its own paths); the human page above is independent
            // of it, so its result never overwrites the page status.
            append_log(
                &dir,
                &format!("give-up act enqueue exit={code} (page: {escalation})"),
            );
        }
        verdict = "GAVE-UP";
    }
    let stamp = json!({
        "schema": SHEPHERD_STAMP_SCHEMA,
        "reason": if verdict == "YES" { "verified" } else { "bounded-give-up" },
        "escalation": escalation,
        "boot_id": boot_id,
        "epoch": epoch.as_json(),
        "incident": incident,
        "registry": own,
        "verdict": verdict,
        "judge": state.judge,
        "rounds": state.round,
        "teams": summaries,
    });
    save_stamp(&dir, &stamp)?;
    append_log(&dir, &format!("stamp: {verdict} judge={}", state.judge));
    Ok(json!({
        "schema": SHEPHERD_REPORT_SCHEMA,
        "boot_id": stamp["boot_id"],
        "verdict": verdict,
        "judge": state.judge,
        "rounds": state.round,
        "stamp": dir.join("stamp.json"),
        "resumed": false,
        "exit_code": if verdict == "YES" { 0 } else { 75 },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_parsing_takes_the_last_json_line() {
        let answer = "judging\n{\"action\": \"facts\"}\n{\"action\": \"revive\", \"agent\": \"lane\"}\ntrail";
        let proposal = parse_proposal(answer).expect("proposal");
        assert_eq!(proposal["action"], "revive");
        assert_eq!(proposal_argv(&proposal).unwrap(), ["revive", "lane"]);
        assert!(parse_proposal("no json here").is_none());
        assert!(proposal_argv(&json!({"action": "bind-session"})).is_err());
        assert!(proposal_argv(&json!({"action": "revive"})).is_err());
    }

    #[test]
    fn judge_comes_from_the_answered_attempt_line() {
        let log = "run-agent-prompt: harness=muse status=timeout exit=- duration_s=1.0 model=unknown effort=low\n\
                   run-agent-prompt: harness=claude status=answered exit=0 duration_s=0.5 model=unknown effort=low\n";
        assert_eq!(judge_of(log).as_deref(), Some("claude"));
        assert_eq!(judge_of("run-agent-prompt: every harness failed"), None);
    }

    #[test]
    fn manifest_hash_is_stable_and_sensitive() {
        let one = json!({"a": 1});
        let two = json!({"a": 2});
        assert_eq!(manifest_hash(&one), manifest_hash(&one));
        assert_ne!(manifest_hash(&one), manifest_hash(&two));
    }
}

/// Schema of a recovery profile file.
pub const PROFILES_SCHEMA: &str = "agentctl-recovery-profiles/v1";
/// Schema of the recorded install-smoke results.
pub const SMOKE_SCHEMA: &str = "agentctl-recovery-smoke/v1";
/// Schema of a recovery incident claim.
pub const CLAIM_SCHEMA: &str = "agentctl-recovery-claim/v1";

/// One ordered harness/model profile (`[[profile]]` in the file).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct RecoveryProfile {
    /// Unique profile name; stamps and smoke records use it.
    pub name: String,
    /// `muse`, `claude`, or `codex`.
    pub harness: String,
    /// Requested model, when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Requested effort, when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Per-attempt seconds budget for this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<f64>,
    /// Absolute harness executable, when not resolved by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Explicitly out of the chain (install records the reason in prose).
    #[serde(default)]
    pub disabled: bool,
}

/// Parse the strict v1 profile-file subset: a top-level `schema`
/// string plus ordered `[[profile]]` tables whose only keys are the
/// [`RecoveryProfile`] fields with string, number, or boolean values.
/// This is the whole v1 schema, so the subset loses nothing; anything
/// outside it is refused rather than guessed at.
pub fn parse_profiles(text: &str) -> Result<Vec<RecoveryProfile>> {
    let mut schema: Option<String> = None;
    let mut profiles: Vec<RecoveryProfile> = Vec::new();
    let mut current: Option<RecoveryProfile> = None;
    let mut in_profile = false;
    let unquote = |value: &str| -> Result<String> {
        let value = value.trim();
        value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(str::to_owned)
            .ok_or_else(|| fail(format!("profile value must be a quoted string: {value}")))
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[profile]]" {
            if let Some(done) = current.take() {
                profiles.push(done);
            }
            current = Some(RecoveryProfile::default());
            in_profile = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(fail(format!("invalid profile line: {line}")));
        };
        let (key, value) = (key.trim(), value.trim());
        if !in_profile {
            if key != "schema" {
                return Err(fail(format!("unknown top-level profile key {key}")));
            }
            schema = Some(unquote(value)?);
            continue;
        }
        let profile = current.as_mut().expect("profile table open");
        match key {
            "name" => profile.name = unquote(value)?,
            "harness" => profile.harness = unquote(value)?,
            "model" => profile.model = Some(unquote(value)?),
            "effort" => profile.effort = Some(unquote(value)?),
            "command" => profile.command = Some(unquote(value)?),
            "timeout_seconds" => {
                let parsed: f64 = value
                    .parse()
                    .map_err(|_| fail("profile timeout_seconds must be a number"))?;
                if !parsed.is_finite() {
                    return Err(fail("profile timeout_seconds must be finite"));
                }
                profile.timeout_seconds = Some(parsed);
            }
            "disabled" => {
                profile.disabled = match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(fail("profile disabled must be true or false")),
                };
            }
            other => return Err(fail(format!("unknown profile key {other}"))),
        }
    }
    if let Some(done) = current.take() {
        profiles.push(done);
    }
    if schema.as_deref() != Some(PROFILES_SCHEMA) {
        return Err(fail(format!("profile schema must be {PROFILES_SCHEMA}")));
    }
    if profiles.is_empty() {
        return Err(fail("at least one [[profile]] is required"));
    }
    let mut names = std::collections::BTreeSet::new();
    for profile in &profiles {
        if profile.name.is_empty() || profile.harness.is_empty() {
            return Err(fail("each profile needs name and harness"));
        }
        if !names.insert(profile.name.clone()) {
            return Err(fail(format!("duplicate profile name {}", profile.name)));
        }
        if !matches!(profile.harness.as_str(), "muse" | "claude" | "codex") {
            return Err(fail(format!("unknown harness {}", profile.harness)));
        }
        if let Some(command) = &profile.command {
            if !Path::new(command).is_absolute() {
                return Err(fail(format!(
                    "profile {} command must be absolute",
                    profile.name
                )));
            }
        }
        if profile
            .timeout_seconds
            .is_some_and(|timeout| timeout <= 0.0)
        {
            return Err(fail(format!(
                "profile {} timeout_seconds must be positive",
                profile.name
            )));
        }
    }
    Ok(profiles)
}

/// Resolve a wrapper name to the absolute canonical executable the
/// smoke record binds: an absolute path canonicalizes, a bare name
/// resolves through PATH exactly once. Unresolvable means judgement
/// is unavailable, never a cwd-relative guess (NS2).
pub fn resolve_wrapper(wrapper: &Path) -> Option<PathBuf> {
    let candidate = if wrapper.is_absolute() {
        wrapper.to_path_buf()
    } else if wrapper.components().count() == 1 {
        std::env::var_os("PATH").and_then(|path| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(wrapper))
                .find(|candidate| candidate.is_file())
        })?
    } else {
        std::path::absolute(wrapper).ok()?
    };
    std::fs::canonicalize(&candidate).ok()
}

/// Load and validate the profile file at `path`.
pub fn load_profiles(path: &Path) -> Result<Vec<RecoveryProfile>> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| fail(format!("cannot read {}: {error}", path.display())))?;
    parse_profiles(&text)
}

/// SHA-256 of a profile file's bytes; smoke records bind to it.
pub fn profiles_hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The team recovery state directory: `<state root>/recovery/<key>/`.
pub fn recovery_dir(state_dir: Option<&Path>, registry: &Path) -> Option<PathBuf> {
    Some(
        state_root(state_dir)?
            .join("recovery")
            .join(crate::restore::team_key(registry)),
    )
}

/// Whether judgement is currently trusted for this profile file: a
/// smoke record exists, binds to the file's current hash, and every
/// enabled profile passed. Anything else leaves judgement unavailable.
pub fn judgement_allowed(
    state_dir: Option<&Path>,
    registry: &Path,
    profile_file: &Path,
    wrapper: &Path,
) -> bool {
    let Ok(bytes) = std::fs::read(profile_file) else {
        return false;
    };
    let Ok(profiles) = parse_profiles(&String::from_utf8_lossy(&bytes)) else {
        return false;
    };
    let Some(dir) = recovery_dir(state_dir, registry) else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(dir.join("install-smoke.json")) else {
        return false;
    };
    let Ok(record) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    if record["schema"] != SMOKE_SCHEMA || record["config_hash"] != profiles_hash(&bytes) {
        return false;
    }
    if record["chain"]["status"] != "pass" {
        return false;
    }
    // The record binds the wrapper it smoked; a different wrapper at
    // runtime is a different, unsmoked judgement path.
    let smoked_wrapper = record["wrapper"].as_str().unwrap_or_default();
    let Some(runtime) = resolve_wrapper(wrapper) else {
        return false;
    };
    if smoked_wrapper != runtime.display().to_string() {
        return false;
    }
    let enabled: Vec<&RecoveryProfile> = profiles
        .iter()
        .filter(|profile| !profile.disabled)
        .collect();
    if enabled.is_empty() {
        return false;
    }
    enabled.iter().all(|profile| {
        record["profiles"].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["name"] == profile.name && row["status"] == "pass")
        })
    })
}

/// The `/proc` start ticks (field 22) of a live pid, for verifying
/// that a recorded pid is still the same process, not a recycled one.
pub fn pid_start_ticks(pid: u64) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19).map(str::to_owned)
}

/// Whether a claim's recorded owner is verifiably still that process:
/// same pid, same start ticks.
pub fn claim_owner_alive(claim: &Value) -> bool {
    let Some(pid) = claim["pid"].as_u64() else {
        return false;
    };
    // A zombie is dead: its parent simply has not reaped it yet.
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        if stat
            .rsplit_once(')')
            .map(|(_, rest)| rest.split_whitespace().next() == Some("Z"))
            .unwrap_or(false)
        {
            return false;
        }
    }
    match (claim["pid_start"].as_str(), pid_start_ticks(pid)) {
        (Some(recorded), Some(current)) => recorded == current,
        _ => Path::new(&format!("/proc/{pid}")).exists(),
    }
}

/// What claiming an incident should do, decided purely.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimDecision {
    /// No live claimant: take the incident.
    Take,
    /// A previous-boot (or previous-epoch) claim: it never blocks.
    TakeOverPreviousBoot,
    /// A dead or stale claimant in this boot: take over, recorded.
    TakeOverStale,
    /// A live claimant in this boot owns the incident.
    AlreadyRunning,
}

/// Decide from a claim record. A claim from a different boot is stale
/// by definition, whatever its heartbeat says (the reboot rule). A
/// verifiably live owner is never taken over, however old its
/// heartbeat is; a dead owner is superseded only once its heartbeat
/// is also past the stale interval.
pub fn claim_decision(
    claim: Option<&Value>,
    epoch_key: &str,
    now: f64,
    stale_seconds: f64,
    owner_alive: bool,
) -> ClaimDecision {
    let Some(claim) = claim else {
        return ClaimDecision::Take;
    };
    // A different canonical epoch (boot plus manager start time)
    // never blocks, live heartbeat or not.
    if claim["epoch_key"].as_str() != Some(epoch_key) {
        return ClaimDecision::TakeOverPreviousBoot;
    }
    if owner_alive {
        return ClaimDecision::AlreadyRunning;
    }
    let heartbeat = claim["heartbeat_at"].as_f64().unwrap_or(0.0);
    if now - heartbeat >= stale_seconds {
        ClaimDecision::TakeOverStale
    } else {
        ClaimDecision::AlreadyRunning
    }
}

/// The outcome of trying to own an incident.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaimOutcome {
    /// This process owns the incident (fresh, adopted, or taken over).
    Owned(ClaimDecision),
    /// A live supervisor already owns it; start nothing.
    AlreadyRunning,
}

/// Claim (or adopt a starter's pending claim) under the short flock.
/// The starter marks its claim `supervisor_pending`; the supervisor it
/// launched adopts it by incident. Any other live claim refuses.
/// Who is claiming: the starter (which only creates a pending
/// claim for its supervisor) or the supervisor itself (which may
/// adopt a starter's pending claim). A second starter must never
/// adopt the first starter's pending claim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ClaimRole {
    /// Creating a pending claim for a supervisor about to launch.
    Starter,
    /// The supervisor: may adopt a starter's pending claim.
    Supervisor,
}

#[allow(clippy::too_many_arguments)]
/// Claim (or adopt) the incident under its short flock.
pub fn acquire_claim(
    claim_path: &Path,
    registry: &Path,
    incident: &str,
    epoch: &RestoreEpoch,
    epoch_key: &str,
    stale_seconds: f64,
    supervisor_pending: bool,
    role: ClaimRole,
) -> Result<ClaimOutcome> {
    let lock_path = claim_path.with_extension("lock");
    if let Some(parent) = claim_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| fail(format!("cannot create {}: {error}", parent.display())))?;
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|error| fail(format!("cannot open the claim lock: {error}")))?;
    lock.lock_exclusive()
        .map_err(|error| fail(format!("cannot take the claim lock: {error}")))?;
    let existing: Option<Value> = std::fs::read_to_string(claim_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d: Duration| d.as_secs_f64())
        .unwrap_or(0.0);
    let me = std::process::id();
    if let Some(claim) = &existing {
        let same_incident = claim["incident"].as_str() == Some(incident);
        if same_incident && claim["pid"].as_u64() == Some(u64::from(me)) {
            return Ok(ClaimOutcome::Owned(ClaimDecision::Take));
        }
        // Adoption (supervisors only): the starter that claimed for
        // us is our parent, or has exited leaving its pending claim.
        if role == ClaimRole::Supervisor
            && same_incident
            && claim["supervisor_pending"] == json!(true)
        {
            let owner = claim["pid"].as_u64().unwrap_or(0);
            let parent = std::fs::read_to_string("/proc/self/stat")
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(')')?
                        .1
                        .split_whitespace()
                        .nth(1)?
                        .parse::<u64>()
                        .ok()
                })
                .unwrap_or(0);
            if owner == parent || !claim_owner_alive(claim) {
                write_claim(
                    claim_path,
                    registry,
                    incident,
                    epoch,
                    epoch_key,
                    now,
                    supervisor_pending,
                    "adopted",
                )?;
                return Ok(ClaimOutcome::Owned(ClaimDecision::Take));
            }
        }
    }
    let owner_alive = existing.as_ref().is_some_and(claim_owner_alive);
    let decision = claim_decision(
        existing.as_ref(),
        epoch_key,
        now,
        stale_seconds,
        owner_alive,
    );
    if decision == ClaimDecision::AlreadyRunning {
        return Ok(ClaimOutcome::AlreadyRunning);
    }
    write_claim(
        claim_path,
        registry,
        incident,
        epoch,
        epoch_key,
        now,
        supervisor_pending,
        &format!("{decision:?}"),
    )?;
    Ok(ClaimOutcome::Owned(decision))
}

#[allow(clippy::too_many_arguments)]
fn write_claim(
    claim_path: &Path,
    registry: &Path,
    incident: &str,
    epoch: &RestoreEpoch,
    epoch_key: &str,
    now: f64,
    supervisor_pending: bool,
    takeover: &str,
) -> Result<()> {
    let me = std::process::id();
    let claim = json!({
        "schema": CLAIM_SCHEMA,
        "registry": registry,
        "incident": incident,
        "boot_id": epoch.boot_id,
        "epoch": epoch.as_json(),
        "epoch_key": epoch_key,
        "pid": me,
        "pid_start": pid_start_ticks(u64::from(me)),
        "heartbeat_at": now,
        "started_at": now,
        "supervisor_pending": supervisor_pending,
        "takeover": takeover,
    });
    crate::agent::atomic_json(claim_path, &claim).map_err(|error| fail(error.to_string()))
}

/// Refresh a claim's heartbeat under its short flock: constant-time
/// file work only, never across a round's long operations. Returns
/// false when this process no longer owns the claim (another owner
/// took it, or it vanished): the caller must stop acting.
/// Lost = another owner holds the claim (stop at once); Transient =
/// an io/parse error (retry); Owned = heartbeat refreshed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Heartbeat {
    /// This process still owns the claim; heartbeat refreshed.
    Owned,
    /// Another owner holds the claim; stop at once.
    Lost,
    /// An io or parse error; retry before declaring loss.
    Transient,
}

/// Refresh the claim heartbeat and report ownership precisely.
pub fn heartbeat_claim_result(path: &Path, incident: &str) -> Heartbeat {
    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))
    else {
        return Heartbeat::Transient;
    };
    if file.lock_exclusive().is_err() {
        return Heartbeat::Transient;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return Heartbeat::Transient;
    };
    let Ok(mut record) = serde_json::from_str::<Value>(&text) else {
        return Heartbeat::Transient;
    };
    if record["incident"].as_str() != Some(incident)
        || record["pid"].as_u64() != Some(u64::from(std::process::id()))
    {
        return Heartbeat::Lost;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d: Duration| d.as_secs_f64())
        .unwrap_or(0.0);
    record["heartbeat_at"] = json!(now);
    record["supervisor_pending"] = json!(false);
    if crate::agent::atomic_json(path, &record).is_ok() {
        Heartbeat::Owned
    } else {
        Heartbeat::Transient
    }
}

/// Whether this process still owns the claim (heartbeat refreshed).
pub fn heartbeat_claim(path: &Path, incident: &str) -> bool {
    heartbeat_claim_result(path, incident) == Heartbeat::Owned
}

/// The fixed harmless smoke prompt: an exact final JSON marker, so a
/// pass proves executable resolution, credentials, model/effort
/// selection, timeout handling, and the structured reply shape.
pub const SMOKE_PROMPT: &str = "You are smoke-testing a recovery profile. \
Reply with exactly one final line and nothing after it:\n\
{\"marker\": \"agentctl-recovery-smoke\"}";

/// Whether an answer passes smoke: its last JSON line is the marker.
pub fn smoke_passed(answer: &str) -> bool {
    answer.lines().rev().find_map(|line| {
        let value: Value = serde_json::from_str(line.trim()).ok()?;
        Some(value["marker"] == "agentctl-recovery-smoke")
    }) == Some(true)
}

/// Run `recovery smoke`: every enabled profile (or one named profile)
/// plus one whole-chain run, recording `install-smoke.json`.
pub fn smoke_profiles(
    registry: &Path,
    profile_file: &Path,
    wrapper: &Path,
    only: Option<&str>,
    state_dir: Option<&Path>,
) -> Result<(Value, i32)> {
    // Resolve the wrapper once, to the absolute canonical path the
    // record binds and every attempt uses (NS2).
    let resolved = resolve_wrapper(wrapper)
        .ok_or_else(|| fail("run-agent-prompt is not resolvable to an absolute executable"))?;
    let wrapper = &resolved;
    let profiles = load_profiles(profile_file)?;
    let bytes = std::fs::read(profile_file)
        .map_err(|error| fail(format!("cannot read {}: {error}", profile_file.display())))?;
    let dir = recovery_dir(state_dir, registry)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    agent_queue::fsio::create_private_directory(&dir, "recovery state directory", true, true)
        .map_err(|error| fail(error.to_string()))?;
    let prompt_path = dir.join("smoke-prompt.txt");
    std::fs::write(&prompt_path, SMOKE_PROMPT)
        .map_err(|error| fail(format!("cannot write the smoke prompt: {error}")))?;
    let mut rows: Vec<Value> = Vec::new();
    let mut all_pass = true;
    for profile in profiles.iter().filter(|profile| !profile.disabled) {
        if let Some(name) = only {
            if profile.name != name {
                continue;
            }
        }
        let started = Instant::now();
        let timeout = profile.timeout_seconds.unwrap_or(120.0);
        let result = run_bounded(
            wrapper,
            &[
                "--profile-file".to_owned(),
                profile_file.display().to_string(),
                "--profile".to_owned(),
                profile.name.clone(),
                "--prompt-file".to_owned(),
                prompt_path.display().to_string(),
                "--timeout".to_owned(),
                timeout.to_string(),
            ],
            Duration::from_secs_f64(timeout * 3.0 + 30.0),
        );
        let (status, detail) = match result {
            Ok((0, answer, _)) if smoke_passed(&answer) => {
                ("pass", "answered with the smoke marker")
            }
            Ok((0, _, _)) => ("fail", "answered without the smoke marker"),
            Ok((69, _, _)) => ("fail", "every harness failed"),
            Ok((code, _, _)) => {
                all_pass = false;
                rows.push(json!({"name": profile.name, "harness": profile.harness,
                    "model": profile.model, "effort": profile.effort,
                    "status": "fail", "detail": format!("wrapper exit {code}"),
                    "duration_s": started.elapsed().as_secs_f64()}));
                continue;
            }
            Err(_error) => ("fail", "wrapper could not run"),
        };
        if status != "pass" {
            all_pass = false;
        }
        rows.push(json!({"name": profile.name, "harness": profile.harness,
            "model": profile.model, "effort": profile.effort,
            "status": status, "detail": detail,
            "duration_s": started.elapsed().as_secs_f64()}));
    }
    // One whole-chain run proves fall-through order end to end.
    let chain_started = Instant::now();
    let chain = run_bounded(
        wrapper,
        &[
            "--profile-file".to_owned(),
            profile_file.display().to_string(),
            "--prompt-file".to_owned(),
            prompt_path.display().to_string(),
            "--timeout".to_owned(),
            "120".to_owned(),
        ],
        Duration::from_secs(390),
    );
    let (chain_status, judge) = match &chain {
        Ok((0, answer, log)) if smoke_passed(answer) => {
            ("pass", judge_of(log).unwrap_or_else(|| "none".to_owned()))
        }
        _ => ("fail", "none".to_owned()),
    };
    if chain_status != "pass" {
        all_pass = false;
    }
    let mut record = json!({
        "schema": SMOKE_SCHEMA,
        "wrapper": std::fs::canonicalize(wrapper)
            .unwrap_or_else(|_| wrapper.to_path_buf())
            .display()
            .to_string(),
        "at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d: Duration| d.as_secs_f64())
            .unwrap_or(0.0),
        "config_hash": profiles_hash(&bytes),
        "profiles": rows,
        "chain": {"status": chain_status, "judge": judge,
                  "duration_s": chain_started.elapsed().as_secs_f64()},
    });
    // A single-profile smoke refreshes that profile's row in the
    // existing record instead of discarding the others (N8).
    if only.is_some() {
        if let Ok(text) = std::fs::read_to_string(dir.join("install-smoke.json")) {
            if let Ok(mut existing) = serde_json::from_str::<Value>(&text) {
                if existing["config_hash"] == record["config_hash"] {
                    let mut merged: Vec<Value> = existing["profiles"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|row| !rows.iter().any(|new| new["name"] == row["name"]))
                        .collect();
                    merged.extend(rows.iter().cloned());
                    existing["profiles"] = json!(merged);
                    existing["at"] = record["at"].clone();
                    record = existing;
                }
            }
        }
    }
    crate::agent::atomic_json(&dir.join("install-smoke.json"), &record)
        .map_err(|error| fail(error.to_string()))?;
    Ok((record, if all_pass { 0 } else { 75 }))
}

/// Options for the one `start-recovery` entry point.
#[derive(Clone, Debug)]
pub struct StartOptions {
    /// Keep the supervisor in the foreground (container/manual use).
    pub foreground: bool,
    /// Seconds before a same-boot claim with no heartbeat is stale.
    pub stale_seconds: f64,
    /// The `run-agent-prompt` executable for the supervisor child.
    pub run_agent_prompt: PathBuf,
}

/// Claim or join the incident, then launch restore and the recovery
/// supervisor concurrently. This is the single trigger-neutral entry
/// point: a service unit, a script, a remote shell, and a human all
/// call it, and a second live call joins instead of starting a
/// second restorer. The claim flock is held only for the claim
/// read/write itself.
pub fn start_recovery(
    registry: &Path,
    host: &dyn Host,
    inputs: &ShepherdInputs,
    state_dir: Option<&Path>,
    options: &StartOptions,
) -> Result<Value> {
    crate::team_driver::require_host_namespace(host)?;
    if !registry.is_dir() {
        return Ok(json!({"schema": "agentctl-start-recovery/v1",
            "status": "BLOCKED_PREREQUISITE",
            "detail": format!("registry {} is not a directory", registry.display()),
            "exit_code": 1}));
    }
    let boot_id = host.boot_id()?;
    let epoch = RestoreEpoch {
        boot_id: boot_id.clone(),
        user_manager: host.user_manager_id(),
    };
    let incident = crate::team_driver::incident_id(registry, &epoch);
    let dir = recovery_dir(state_dir, registry)
        .ok_or_else(|| fail("cannot determine the agentctl state directory"))?;
    agent_queue::fsio::create_private_directory(&dir, "recovery state directory", true, true)
        .map_err(|error| fail(error.to_string()))?;
    let claim_path = dir.join("claim.json");
    // Claim for the supervisor we are about to launch (pending until
    // it adopts by incident): short flock, constant-time work only.
    match acquire_claim(
        &claim_path,
        registry,
        &incident,
        &epoch,
        &epoch_identity(host, &epoch),
        options.stale_seconds,
        true,
        ClaimRole::Starter,
    )? {
        ClaimOutcome::Owned(_) => {}
        ClaimOutcome::AlreadyRunning => {
            return Ok(json!({"schema": "agentctl-start-recovery/v1",
                "status": "already-running", "incident": incident,
                "claim": claim_path, "exit_code": 75}));
        }
    }
    // Launch the supervisor; it spawns and owns the restore child
    // itself (in the child's own session), so the starter never
    // holds a restore pid and a supervisor restart re-adopts the
    // running restore by the supervisor's own state record.
    let base = vec![
        "--registry".to_owned(),
        registry.display().to_string(),
        "--herdr-bin".to_owned(),
        inputs.herdr_bin.display().to_string(),
    ];
    let spawn_logged =
        |args: &[String], log_name: &str, claim_env: bool| -> Result<std::process::Child> {
            let log = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(log_name))
                .map_err(|error| fail(format!("cannot open the recovery log: {error}")))?;
            let err = log
                .try_clone()
                .map_err(|error| fail(format!("cannot clone the recovery log: {error}")))?;
            let mut command = Command::new(&inputs.agentctl);
            command
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log))
                .stderr(Stdio::from(err));
            if claim_env {
                command
                    .env("AGENTCTL_RECOVERY_CLAIM_FILE", &claim_path)
                    .env("AGENTCTL_RECOVERY_RESTORE_LAUNCHED", "1");
            }
            command
                .spawn()
                .map_err(|error| fail(format!("cannot launch a recovery child: {error}")))
        };
    let mut shepherd_args = base;
    shepherd_args.extend([
        "team".to_owned(),
        "shepherd".to_owned(),
        "--run-agent-prompt".to_owned(),
        options.run_agent_prompt.display().to_string(),
    ]);
    if let Some(state) = state_dir {
        shepherd_args.extend(["--state-dir".to_owned(), state.display().to_string()]);
    }
    let mut supervisor = spawn_logged(&shepherd_args, "supervisor.log", true)?;
    if options.foreground {
        let status = supervisor
            .wait()
            .map_err(|error| fail(format!("cannot wait on the supervisor: {error}")))?;
        return Ok(json!({"schema": "agentctl-start-recovery/v1",
            "status": "finished", "incident": incident, "claim": claim_path,
            "exit_code": status.code().unwrap_or(1)}));
    }
    Ok(json!({"schema": "agentctl-start-recovery/v1",
        "status": "started", "incident": incident, "claim": claim_path,
        "supervisor_pid": supervisor.id(),
        "exit_code": 0}))
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn profiles_parse_in_file_order_and_refuse_guesses() {
        let text = "schema = \"agentctl-recovery-profiles/v1\"\n\
                    [[profile]]\nname = \"local-claude\"\nharness = \"claude\"\n\
                    [[profile]]\nname = \"local-codex\"\nharness = \"codex\"\nmodel = \"m\"\n";
        let profiles = parse_profiles(text).expect("profiles");
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "local-claude");
        assert_eq!(profiles[1].model.as_deref(), Some("m"));
        assert!(parse_profiles(
            "schema = \"nope\"\n[[profile]]\nname = \"a\"\nharness = \"claude\"\n"
        )
        .is_err());
        assert!(parse_profiles(
            "schema = \"agentctl-recovery-profiles/v1\"\n[[profile]]\nname = \"a\"\nharness = \"claude\"\ncommand = \"relative\"\n"
        )
        .is_err());
    }

    #[test]
    fn previous_boot_claim_never_blocks() {
        let claim = json!({"boot_id": "old", "epoch_key": "old@nomgr", "pid": 999999, "heartbeat_at": 10.0});
        assert_eq!(
            claim_decision(Some(&claim), "new@nomgr", 20.0, 180.0, false),
            ClaimDecision::TakeOverPreviousBoot
        );
        let live =
            json!({"boot_id": "new", "epoch_key": "new@nomgr", "pid": 1, "heartbeat_at": 19.0});
        // A verifiably live owner is never taken over, however old
        // its heartbeat; a dead owner with a fresh heartbeat is not
        // stale yet either; only dead + past the interval is.
        assert_eq!(
            claim_decision(Some(&live), "new@nomgr", 20.0, 180.0, true),
            ClaimDecision::AlreadyRunning
        );
        assert_eq!(
            claim_decision(Some(&live), "new@nomgr", 20.0, 180.0, false),
            ClaimDecision::AlreadyRunning
        );
        assert_eq!(
            claim_decision(Some(&live), "new@nomgr", 500.0, 180.0, false),
            ClaimDecision::TakeOverStale
        );
        assert_eq!(
            claim_decision(None, "new@nomgr", 20.0, 180.0, false),
            ClaimDecision::Take
        );
    }

    #[test]
    fn progress_hash_ignores_checked_at() {
        let one = json!({"checked_at": 1.0, "checks": [{"checked_at": 2.0, "status": "fail"}]});
        let two = json!({"checked_at": 9.0, "checks": [{"checked_at": 8.0, "status": "fail"}]});
        assert_eq!(progress_hash(&one), progress_hash(&two));
        let three = json!({"checked_at": 1.0, "checks": [{"checked_at": 2.0, "status": "pass"}]});
        assert_ne!(progress_hash(&one), progress_hash(&three));
    }

    #[test]
    fn escalate_only_when_down_unknown_free_and_owned() {
        assert!(should_escalate(false, false, false, false));
        assert!(!should_escalate(true, false, false, false));
        assert!(!should_escalate(false, true, false, false));
        // Unknown facts page through the enqueue-only path.
        assert!(should_escalate(false, false, true, false));
        assert!(!should_escalate(false, false, false, true));
    }

    #[test]
    fn smoke_needs_the_exact_final_marker() {
        assert!(smoke_passed(
            "ok\n{\"marker\": \"agentctl-recovery-smoke\"}"
        ));
        assert!(!smoke_passed("{\"marker\": \"other\"}"));
        assert!(!smoke_passed("no marker"));
    }
}
