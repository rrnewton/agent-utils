//! Whether a restored team really came back: the VERIFY health checks a
//! restore must pass before anyone calls it done.
//!
//! `agentctl team verify` runs these checks standalone, read-only apart from
//! dismissing a dialog the allowlist names (check `pane-at-prompt`), and the
//! team driver runs the same [`verify_team`] as its VERIFY state. Every row
//! names the agent or unit, the evidence, and the fix, because a row is what
//! a HANDOFF checklist quotes to the team lead. A check with no evidence
//! fails; a check that cannot apply is `skipped` and says why.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::agent::{AgentError, QueueMessageState};
use crate::harness_tui::State as TuiState;
use crate::restore::{
    derived_lifecycle, registered_names, team_driver_dir, CommandOutcome, Host, RestoreConfig,
};
use crate::subagents::{ManagedAgents, ManagedApi};

/// Schema of a verify report.
pub const VERIFY_SCHEMA: &str = "agentctl-team-verify/v1";
/// Default coordinator-ack deadline; the driver's HANDOFF timeout shares it.
pub const DEFAULT_ACK_TIMEOUT_SECONDS: u64 = 1800;
/// Watch health older than this, or from another boot, proves nothing.
const WATCH_FRESH_SECONDS: f64 = 900.0;
/// The first allowlisted benign dialog: revived panes sat behind it on
/// 2026-11-12 with owner messages queued behind them.
pub const TEACH_AUTO_MODE_DIALOG: &str = "Teach auto mode about your environment?";
/// Screen rows read per pane for `pane-at-prompt`.
const PANE_LINES: usize = 60;
/// Most archived-inbox rows one report carries before summarizing the rest.
const MAX_ARCHIVED_ROWS: usize = 20;

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// How a verify runs.
#[derive(Clone, Debug)]
pub struct VerifyOptions {
    /// Agentctl state root holding `watch/health.json` and `team-drivers/`;
    /// `None` derives the XDG state home, as watch does.
    pub state_dir: Option<PathBuf>,
    /// Age past which an unprocessed outage notice fails `coordinator-ack`.
    pub ack_timeout_seconds: u64,
    /// Dismiss an allowlisted dialog instead of only reporting it.
    pub dismiss_allowlisted: bool,
}
impl Default for VerifyOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            ack_timeout_seconds: DEFAULT_ACK_TIMEOUT_SECONDS,
            dismiss_allowlisted: true,
        }
    }
}

/// One check row, the unit HANDOFF quotes.
fn row(
    name: &str,
    status: &str,
    subject: Option<&str>,
    evidence: String,
    fix: Option<String>,
) -> Value {
    json!({
        "name": name,
        "status": status,
        "subject": subject,
        "evidence": one_line(&evidence),
        "fix": fix.map(|text| one_line(&text)),
    })
}

fn one_line(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > 300 {
        format!("{}...", joined.chars().take(300).collect::<String>())
    } else if joined.is_empty() {
        "no evidence recorded".to_owned()
    } else {
        joined
    }
}

/// The desired pool, derived exactly as restore derives it: the coordinator
/// always, then the pinned list when one is configured, else every record
/// that was running at shutdown. Undecidable records return as rows.
pub(crate) struct DesiredSet {
    pub(crate) names: Vec<String>,
    pub(crate) excluded: Vec<String>,
    pub(crate) undecidable: Vec<Value>,
}

pub(crate) fn desired_set(registry: &Path, config: &RestoreConfig) -> Result<DesiredSet> {
    let registered = registered_names(registry)?;
    let mut names: Vec<String> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut undecidable: Vec<Value> = Vec::new();
    if let Some(coordinator) = config.coordinator.as_deref() {
        names.push(coordinator.to_owned());
    }
    for name in &registered {
        if config.coordinator.as_deref() == Some(name.as_str()) {
            continue;
        }
        if config.exclude.contains(name) {
            excluded.push(name.clone());
            continue;
        }
        if let Some(pinned) = config.agents.as_ref() {
            if pinned.contains(name) {
                names.push(name.clone());
            } else {
                excluded.push(name.clone());
            }
            continue;
        }
        match derived_lifecycle(registry, name) {
            Ok(lifecycle) if lifecycle == "running" || lifecycle == "launch_failed" => {
                names.push(name.clone());
            }
            Ok(_) => excluded.push(name.clone()),
            Err(evidence) => undecidable.push(row(
                "desired-live",
                "fail",
                Some(name),
                format!("cannot derive desired state for {name}: {evidence}"),
                Some(format!(
                    "inspect the record of {name}, then restore or exclude it"
                )),
            )),
        }
    }
    Ok(DesiredSet {
        names,
        excluded,
        undecidable,
    })
}

/// `desired-live` rows from per-agent status results (pure, so fixtures need
/// no live client). Live means the probe answered and the record is still
/// running; a blocked prompt is live here and fails `pane-at-prompt` instead.
pub(crate) fn desired_live_rows(
    desired: &[String],
    statuses: &[(String, std::result::Result<Value, String>)],
) -> Vec<Value> {
    desired
        .iter()
        .map(|name| {
            let status = statuses
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, found)| found);
            match status {
                None => row(
                    "desired-live",
                    "fail",
                    Some(name),
                    format!("{name} is desired but has no status (not registered?)"),
                    Some(format!("restore {name} or remove it from the desired set")),
                ),
                Some(Err(error)) => row(
                    "desired-live",
                    "fail",
                    Some(name),
                    format!("status of {name} unreadable: {error}"),
                    Some(format!(
                        "read `agentctl status {name}` and fix what it names"
                    )),
                ),
                Some(Ok(value)) => {
                    if let Some(drift) = value.get("profile_drift").filter(|found| !found.is_null())
                    {
                        return row(
                            "desired-live",
                            "fail",
                            Some(name),
                            format!("{name} runs with profile drift: {drift}"),
                            Some(format!(
                                "restart {name} from its recorded profile at a safe point"
                            )),
                        );
                    }
                    let probe_error = value["probe_error"].as_str().unwrap_or_default();
                    let lifecycle = value["lifecycle"].as_str().unwrap_or("unknown");
                    let agent_status = value["agent_status"].as_str().unwrap_or("unknown");
                    if !probe_error.is_empty()
                        || lifecycle != "running"
                        || matches!(agent_status, "unknown" | "dead" | "missing" | "stopped")
                    {
                        let detail = if probe_error.is_empty() {
                            format!("lifecycle {lifecycle}, status {agent_status}")
                        } else {
                            probe_error.to_owned()
                        };
                        row(
                            "desired-live",
                            "fail",
                            Some(name),
                            format!("{name} is not live: {detail}"),
                            Some(format!("revive {name} (`agentctl revive {name}`)")),
                        )
                    } else {
                        let profile = value["profile"].as_str().unwrap_or("its recorded profile");
                        row(
                            "desired-live",
                            "pass",
                            Some(name),
                            format!("{name} live ({agent_status}) as {profile}, no profile drift"),
                            None,
                        )
                    }
                }
            }
        })
        .collect()
}

/// One `units-bridges` row from a `systemctl is-active` outcome (pure).
pub(crate) fn unit_row(unit: &str, outcome: &CommandOutcome) -> Value {
    let printed = outcome.stdout.trim();
    if outcome.status == Some(0) && printed == "active" {
        row(
            "units-bridges",
            "pass",
            Some(unit),
            format!("{unit} active"),
            None,
        )
    } else {
        let detail = if !printed.is_empty() {
            printed.to_owned()
        } else if !outcome.stderr.trim().is_empty() {
            outcome.stderr.trim().to_owned()
        } else {
            format!("is-active exited {:?}", outcome.status)
        };
        row(
            "units-bridges",
            "fail",
            Some(unit),
            format!("{unit} not active: {detail}"),
            Some(format!(
                "reset and start it (systemctl --user reset-failed {unit}; systemctl --user start {unit})"
            )),
        )
    }
}

/// What a pane screen means for `pane-at-prompt` before any dismiss.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PaneVerdict {
    Pass(String),
    Dismiss(String),
    Fail(String),
    Skipped(String),
}

/// One `pane-at-prompt` row from a verdict that needed no dismiss.
fn pane_row(name: &str, verdict: &PaneVerdict) -> Value {
    match verdict {
        PaneVerdict::Pass(evidence) => {
            row("pane-at-prompt", "pass", Some(name), evidence.clone(), None)
        }
        PaneVerdict::Skipped(evidence) => row(
            "pane-at-prompt",
            "skipped",
            Some(name),
            evidence.clone(),
            None,
        ),
        PaneVerdict::Fail(evidence) | PaneVerdict::Dismiss(evidence) => row(
            "pane-at-prompt",
            "fail",
            Some(name),
            format!("{name}: {evidence}"),
            Some(format!(
                "look at {name} (`agentctl read {name}`) and answer or clear what blocks it"
            )),
        ),
    }
}

/// The `watcher-dead-agents` row from watch health (pure): fresh, this
/// boot, and itself ok. Dead desired agents are `desired-live` rows; this
/// row proves the watcher that would notice them is running at all.
pub(crate) fn watcher_row(health: Option<&Value>, boot_id: &str, now: f64, path: &Path) -> Value {
    let Some(health) = health else {
        return row(
            "watcher-dead-agents",
            "fail",
            None,
            format!("no readable watch health at {}", path.display()),
            Some("start the watcher (`agentctl watch`) or its unit".to_owned()),
        );
    };
    if health["boot_id"].as_str() != Some(boot_id) {
        return row(
            "watcher-dead-agents",
            "fail",
            None,
            format!(
                "watch health belongs to boot {:?}, not this boot",
                health["boot_id"]
            ),
            Some("restart the watcher for this boot".to_owned()),
        );
    }
    let checked = health["checked_at"].as_f64().unwrap_or_default();
    if now - checked > WATCH_FRESH_SECONDS {
        return row(
            "watcher-dead-agents",
            "fail",
            None,
            format!("watch health is {:.0}s old", (now - checked).max(0.0)),
            Some("restart the watcher; its last pass is stale".to_owned()),
        );
    }
    if health["ok"] != json!(true) {
        return row(
            "watcher-dead-agents",
            "fail",
            None,
            "watcher runs but reports not ok; see its findings".to_owned(),
            Some(format!("read {}", path.display())),
        );
    }
    row(
        "watcher-dead-agents",
        "pass",
        None,
        "watcher fresh this boot and ok".to_owned(),
        None,
    )
}

/// The `coordinator-ack` row for one coordinator (pure). `Processed` is the
/// strongest receipt the queue gives (delivery confirmed); there is no read
/// receipt, and the row says so instead of claiming one.
pub(crate) fn ack_row(
    name: &str,
    state: Option<QueueMessageState>,
    notice_age_seconds: Option<f64>,
    ack_timeout_seconds: u64,
) -> Value {
    match state {
        None => row(
            "coordinator-ack",
            "skipped",
            Some(name),
            format!("no restore notice for {name} this boot"),
            None,
        ),
        Some(QueueMessageState::Processed) => row(
            "coordinator-ack",
            "pass",
            Some(name),
            format!("{name} outage notice delivery confirmed (queue processed)"),
            None,
        ),
        Some(state) => {
            let age = notice_age_seconds.map_or_else(
                || "age unknown".to_owned(),
                |seconds| format!("{seconds:.0}s old, deadline {ack_timeout_seconds}s"),
            );
            row(
                "coordinator-ack",
                "fail",
                Some(name),
                format!("{name} outage notice {state:?}, {age}"),
                Some(format!(
                    "check {name} can read its queue (`agentctl drain {name}`)"
                )),
            )
        }
    }
}

/// Archived inbox evidence for `queued-not-archived`: a desired agent's
/// archived generation holding inbox files archived at or after this boot.
/// Archive generations are `archive/<name>` or `archive/<name>-<digits...>`.
pub(crate) fn archived_rows(
    registry: &Path,
    desired: &[String],
    boot_time: Option<u64>,
) -> Vec<Value> {
    let archive = registry.join("archive");
    let boundary = boot_time.unwrap_or_default();
    let mut found: Vec<(String, PathBuf, u64)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&archive) {
        let mut generations: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|found| found.path()))
            .filter(|path| path.is_dir())
            .collect();
        generations.sort();
        for generation in generations {
            let Some(dir_name) = generation.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(owner) = desired.iter().find(|name| {
                dir_name == name.as_str()
                    || dir_name
                        .strip_prefix(name.as_str())
                        .and_then(|rest| rest.strip_prefix('-'))
                        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
            }) else {
                continue;
            };
            let inbox = generation.join("queue/inbox");
            let Ok(files) = fs::read_dir(&inbox) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = files
                .filter_map(|entry| entry.ok().map(|found| found.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .collect();
            paths.sort();
            for path in paths {
                let modified = fs::metadata(&path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|when| when.duration_since(UNIX_EPOCH).ok())
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or_default();
                // A missing mtime (0) predates any boot boundary, so an
                // unreadable stamp never invents an outage loss.
                if modified >= boundary && (boundary > 0 || modified > 0) {
                    found.push((owner.clone(), path, modified));
                }
            }
        }
    }
    if found.is_empty() {
        return vec![row(
            "queued-not-archived",
            "pass",
            None,
            "no desired agent has inbox files archived this boot".to_owned(),
            None,
        )];
    }
    let total = found.len();
    let mut rows: Vec<Value> = found
        .into_iter()
        .take(MAX_ARCHIVED_ROWS)
        .map(|(owner, path, modified)| {
            row(
                "queued-not-archived",
                "fail",
                Some(&owner),
                format!(
                    "{} archived at unix {modified} ({}); queued prompts must be delivered, never archived",
                    owner,
                    path.display()
                ),
                Some(format!(
                    "recover the prompt from {} and send it to {owner} again",
                    path.display()
                )),
            )
        })
        .collect();
    if total > MAX_ARCHIVED_ROWS {
        rows.push(row(
            "queued-not-archived",
            "fail",
            None,
            format!(
                "and {} more archived inbox files this boot",
                total - MAX_ARCHIVED_ROWS
            ),
            Some(format!("list {}", archive.display())),
        ));
    }
    rows
}

/// Classify one interactive pane's visible screen (pure).
pub(crate) fn classify_pane(harness: Option<&str>, screen: &str) -> PaneVerdict {
    if screen.contains(TEACH_AUTO_MODE_DIALOG) {
        return PaneVerdict::Dismiss(format!(
            "allowlisted dialog {TEACH_AUTO_MODE_DIALOG:?} is up"
        ));
    }
    if let Some(adapter) = crate::harness_tui::harness_tui(harness) {
        let view = adapter.classify(screen);
        return match view.state {
            TuiState::IdleComposer => PaneVerdict::Pass("pane at an input prompt".to_owned()),
            TuiState::Working => {
                PaneVerdict::Pass("pane mid-turn with a follow-up editor".to_owned())
            }
            TuiState::BlockingQuestion => {
                let question = view
                    .question
                    .map(|found| found.text)
                    .unwrap_or_else(|| "a question".to_owned());
                PaneVerdict::Fail(format!("blocked behind a question: {question}"))
            }
            TuiState::MenuOrPicker => {
                PaneVerdict::Fail("blocked behind a menu or picker".to_owned())
            }
            TuiState::Unknown => match crate::stuck::classify(screen) {
                Some(cause) => PaneVerdict::Fail(format!("pane looks stuck: {cause:?}")),
                None => PaneVerdict::Fail("pane layout not recognized as a prompt".to_owned()),
            },
        };
    }
    match crate::stuck::classify(screen) {
        Some(cause) => PaneVerdict::Fail(format!("pane looks stuck: {cause:?}")),
        None => PaneVerdict::Skipped(format!(
            "no TUI adapter for harness {}",
            harness.unwrap_or("unknown")
        )),
    }
}

/// The watch health file for a state root (watch's own default layout).
fn watch_health_path(state_dir: Option<&Path>) -> Option<PathBuf> {
    let root = match state_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::restore::default_report_dir()?
            .parent()?
            .to_path_buf(),
    };
    Some(root.join("watch/health.json"))
}

/// Read the driver's alarm projection, when one is active.
fn read_alarm(registry: &Path, state_dir: Option<&Path>) -> Value {
    let Some(dir) = team_driver_dir(registry, state_dir) else {
        return Value::Null;
    };
    crate::restore::read_report(&dir.join("alarm.json")).unwrap_or(Value::Null)
}

/// Bridge rows: every state directory under `REGISTRY/bridges`, through the
/// same library `chat health` prints, target presence included.
fn bridge_rows<A: ManagedApi + ?Sized>(client: &A, registry: &Path) -> Vec<Value> {
    let bridges = registry.join("bridges");
    let mut dirs: Vec<PathBuf> = match fs::read_dir(&bridges) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok().map(|found| found.path()))
            .filter(|path| path.is_dir())
            .collect(),
        Err(_) => {
            return vec![row(
                "units-bridges",
                "skipped",
                None,
                format!("no bridges directory at {}", bridges.display()),
                None,
            )]
        }
    };
    dirs.sort();
    if dirs.is_empty() {
        return vec![row(
            "units-bridges",
            "skipped",
            None,
            "no bridge state directories".to_owned(),
            None,
        )];
    }
    dirs.iter()
        .map(|dir| {
            let name = dir
                .file_name()
                .and_then(|found| found.to_str())
                .unwrap_or("bridge");
            let subject = format!("bridge {name}");
            let choice = crate::chat_service::health::choose_registry(
                registry,
                crate::chat_service::health::bridge_registry(dir),
            );
            let probed = ManagedAgents::new(client, &choice.probe).and_then(|manager| {
                crate::chat_service::health::health(
                    dir,
                    &manager,
                    crate::chat_service::health::HealthOptions {
                        stale_after: Duration::from_secs(300),
                    },
                    choice,
                )
                .map_err(|error| fail(error.to_string()))
            });
            match probed {
                Ok(health) if health.verdict == "ok" => row(
                    "units-bridges",
                    "pass",
                    Some(&subject),
                    format!("{subject} chat health ok, target present"),
                    None,
                ),
                Ok(health) => row(
                    "units-bridges",
                    "fail",
                    Some(&subject),
                    format!(
                        "{subject} chat health {}: {}",
                        health.verdict,
                        health.text.lines().next().unwrap_or_default()
                    ),
                    Some(format!(
                        "read `agentctl chat health --bridge-state {}`",
                        dir.display()
                    )),
                ),
                Err(error) => row(
                    "units-bridges",
                    "fail",
                    Some(&subject),
                    format!("{subject} health unreadable: {error}"),
                    Some(format!(
                        "read `agentctl chat health --bridge-state {}`",
                        dir.display()
                    )),
                ),
            }
        })
        .collect()
}

/// Run every check for one team's registry and build the report. Read-only
/// apart from an allowlisted dialog dismiss. An `Err` return means the
/// registry or epoch itself could not be read, never a failed check.
pub fn verify_team<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &VerifyOptions,
) -> Result<Value> {
    let epoch = host.epoch()?;
    let boot_id = host.boot_id()?;
    let now = host.now();
    let manager = ManagedAgents::new(client, registry)?;
    let desired = desired_set(registry, config)?;
    let statuses: Vec<(String, std::result::Result<Value, String>)> = desired
        .names
        .iter()
        .map(|name| {
            (
                name.clone(),
                manager.status(name).map_err(|error| error.to_string()),
            )
        })
        .collect();
    let mut checks: Vec<Value> = Vec::new();
    checks.extend(desired.undecidable.iter().cloned());
    checks.extend(desired_live_rows(&desired.names, &statuses));
    for unit in &config.units {
        let outcome = host.run(
            &[
                "systemctl".to_owned(),
                "--user".to_owned(),
                "is-active".to_owned(),
                unit.clone(),
            ],
            None,
            Duration::from_secs(30),
        );
        checks.push(unit_row(unit, &outcome));
    }
    if config.units.is_empty() {
        checks.push(row(
            "units-bridges",
            "skipped",
            None,
            "no units configured".to_owned(),
            None,
        ));
    }
    checks.extend(bridge_rows(client, registry));
    checks.extend(archived_rows(registry, &desired.names, host.boot_time()));
    let health_path = watch_health_path(options.state_dir.as_deref());
    let health = health_path.as_deref().and_then(crate::restore::read_report);
    checks.push(watcher_row(
        health.as_ref(),
        &boot_id,
        now,
        health_path
            .as_deref()
            .unwrap_or(Path::new("watch/health.json")),
    ));
    for name in &desired.names {
        let status = statuses
            .iter()
            .find(|(known, _)| known == name)
            .and_then(|(_, found)| found.as_ref().ok());
        let interactive = status.is_some_and(|value| {
            value["pane_id"]
                .as_str()
                .is_some_and(|pane| !pane.is_empty())
        });
        if !interactive {
            checks.push(row(
                "pane-at-prompt",
                "skipped",
                Some(name),
                format!("{name} has no interactive pane to read"),
                None,
            ));
            continue;
        }
        let harness = status.and_then(|value| value["harness"].as_str());
        let screen = manager.watch_visible(name, PANE_LINES);
        let verdict = match screen {
            Ok((_, text)) => classify_pane(harness, &text),
            Err(error) => {
                checks.push(row(
                    "pane-at-prompt",
                    "fail",
                    Some(name),
                    format!("{name} pane unreadable: {error}"),
                    Some(format!("inspect {name} in Herdr directly")),
                ));
                continue;
            }
        };
        if let PaneVerdict::Dismiss(evidence) = &verdict {
            if options.dismiss_allowlisted {
                // The one write verify may do: answer the allowlisted
                // dialog at its default, then prove it gone by re-reading.
                let dismissed = manager
                    .prompts(name, Some((None, None)))
                    .map_err(|error| error.to_string())
                    .and_then(|_| {
                        manager
                            .watch_visible(name, PANE_LINES)
                            .map_err(|error| error.to_string())
                    });
                match dismissed {
                    Ok((_, text)) if !text.contains(TEACH_AUTO_MODE_DIALOG) => {
                        checks.push(pane_row(name, &classify_pane(harness, &text)));
                    }
                    Ok(_) => checks.push(row(
                        "pane-at-prompt",
                        "fail",
                        Some(name),
                        format!("{name}: {evidence}; still up after a dismiss attempt"),
                        Some(format!("clear the dialog in {name} by hand")),
                    )),
                    Err(error) => checks.push(row(
                        "pane-at-prompt",
                        "fail",
                        Some(name),
                        format!("{name}: {evidence}; dismiss refused: {error}"),
                        Some(format!("clear the dialog in {name} by hand")),
                    )),
                }
                continue;
            }
        }
        checks.push(pane_row(name, &verdict));
    }
    match config.coordinator.as_deref() {
        None => checks.push(row(
            "coordinator-ack",
            "skipped",
            None,
            "no coordinator configured".to_owned(),
            None,
        )),
        Some(coordinator) => {
            let notice_id = format!("restore-{}", &boot_id[..8.min(boot_id.len())]);
            let report_dir = config
                .report_dir
                .clone()
                .unwrap_or_else(|| registry.join("restore-reports"));
            let finished = crate::restore::read_report(&report_dir.join(format!("{boot_id}.json")))
                .and_then(|found| found["finished_at"].as_f64());
            let age = finished.map(|when| (now - when).max(0.0));
            match manager.message_state(coordinator, &notice_id) {
                Ok(state) => checks.push(ack_row(
                    coordinator,
                    state,
                    age,
                    options.ack_timeout_seconds,
                )),
                Err(error) => checks.push(row(
                    "coordinator-ack",
                    "fail",
                    Some(coordinator),
                    format!("notice state for {coordinator} unreadable: {error}"),
                    Some(format!("inspect the queue of {coordinator}")),
                )),
            }
        }
    }
    let ok = checks.iter().all(|check| check["status"] != "fail");
    Ok(json!({
        "schema": VERIFY_SCHEMA,
        "registry": registry,
        "epoch": epoch,
        "boot_id": boot_id,
        "checked_at": now,
        "ok": ok,
        "desired": desired.names,
        "excluded": desired.excluded,
        "alarm": read_alarm(registry, options.state_dir.as_deref()),
        "checks": checks,
    }))
}

/// Whether a verify report passed (no failed row).
pub fn verify_ok(report: &Value) -> bool {
    report["ok"] == json!(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn temp_registry(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("team-verify-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("agents")).unwrap();
        dir
    }

    fn write_record(registry: &Path, name: &str, lifecycle: &str) {
        let dir = registry.join("agents").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("agent.json"),
            format!("{{\"name\": \"{name}\", \"lifecycle\": \"{lifecycle}\"}}"),
        )
        .unwrap();
    }

    fn config(registry: &Path, body: &str) -> RestoreConfig {
        fs::write(registry.join("restore.json"), body).unwrap();
        RestoreConfig::load(registry).unwrap()
    }

    #[test]
    fn desired_set_derives_running_and_skips_stopped_and_excluded() {
        let registry = temp_registry("desired");
        write_record(&registry, "coord", "running");
        write_record(&registry, "lane-a", "running");
        write_record(&registry, "lane-b", "stopped");
        write_record(&registry, "lane-c", "launch_failed");
        let found = config(
            &registry,
            r#"{"schema": "agentctl-restore/v1", "coordinator": "coord", "exclude": ["lane-c"]}"#,
        );
        let desired = desired_set(&registry, &found).unwrap();
        assert_eq!(desired.names, ["coord", "lane-a"]);
        assert!(desired.excluded.contains(&"lane-b".to_owned()));
        assert!(desired.excluded.contains(&"lane-c".to_owned()));
        assert!(desired.undecidable.is_empty());
        let _ = fs::remove_dir_all(&registry);
    }

    #[test]
    fn desired_live_fails_dead_drifted_and_missing() {
        let desired = vec![
            "up".to_owned(),
            "down".to_owned(),
            "drift".to_owned(),
            "gone".to_owned(),
        ];
        let statuses = vec![
            (
                "up".to_owned(),
                Ok(json!({"lifecycle": "running", "agent_status": "idle", "profile": "p1"})),
            ),
            (
                "down".to_owned(),
                Ok(
                    json!({"lifecycle": "running", "agent_status": "unknown", "probe_error": "pane gone"}),
                ),
            ),
            (
                "drift".to_owned(),
                Ok(
                    json!({"lifecycle": "running", "agent_status": "idle", "profile_drift": "effort differs"}),
                ),
            ),
        ];
        let rows = desired_live_rows(&desired, &statuses);
        assert_eq!(rows[0]["status"], "pass");
        assert_eq!(rows[1]["status"], "fail");
        assert!(rows[1]["evidence"].as_str().unwrap().contains("pane gone"));
        assert_eq!(rows[2]["status"], "fail");
        assert_eq!(rows[3]["status"], "fail");
    }

    #[test]
    fn unit_row_needs_active() {
        let active = CommandOutcome {
            status: Some(0),
            stdout: "active\n".to_owned(),
            stderr: String::new(),
        };
        assert_eq!(unit_row("bridge.service", &active)["status"], "pass");
        // A bridge whose target is missing restart-loops: never active.
        let looping = CommandOutcome {
            status: Some(3),
            stdout: "activating\n".to_owned(),
            stderr: String::new(),
        };
        let found = unit_row("bridge.service", &looping);
        assert_eq!(found["status"], "fail");
        assert!(found["fix"].as_str().unwrap().contains("reset-failed"));
    }

    #[test]
    fn pane_behind_teach_dialog_is_dismiss_never_pass() {
        let screen = format!("...\n{TEACH_AUTO_MODE_DIALOG}\n1. Yes\n2. No\n");
        assert!(matches!(
            classify_pane(Some("claude"), &screen),
            PaneVerdict::Dismiss(_)
        ));
        // Without a dismiss the row fails and names the dialog.
        let verdict = classify_pane(Some("claude"), &screen);
        let found = pane_row("lane-a", &verdict);
        assert_eq!(found["status"], "fail");
        assert!(found["evidence"]
            .as_str()
            .unwrap()
            .contains("Teach auto mode"));
    }

    #[test]
    fn watcher_row_needs_this_boot_and_fresh() {
        let path = Path::new("/state/watch/health.json");
        let fresh = json!({"boot_id": "b1", "checked_at": 1000.0, "ok": true});
        assert_eq!(
            watcher_row(Some(&fresh), "b1", 1100.0, path)["status"],
            "pass"
        );
        let old_boot = json!({"boot_id": "b0", "checked_at": 1000.0, "ok": true});
        assert_eq!(
            watcher_row(Some(&old_boot), "b1", 1100.0, path)["status"],
            "fail"
        );
        assert_eq!(
            watcher_row(Some(&fresh), "b1", 5000.0, path)["status"],
            "fail"
        );
        assert_eq!(watcher_row(None, "b1", 1100.0, path)["status"], "fail");
    }

    #[test]
    fn ack_row_maps_queue_states() {
        assert_eq!(
            ack_row("coord", Some(QueueMessageState::Processed), Some(5.0), 1800)["status"],
            "pass"
        );
        assert_eq!(
            ack_row("coord", Some(QueueMessageState::Pending), Some(5.0), 1800)["status"],
            "fail"
        );
        assert_eq!(ack_row("coord", None, None, 1800)["status"], "skipped");
    }

    #[test]
    fn archived_inbox_this_boot_fails_older_does_not() {
        let registry = temp_registry("archive");
        let inbox = registry.join("archive/lane-a-20261112/queue/inbox");
        fs::create_dir_all(&inbox).unwrap();
        let lost = inbox.join("m1.json");
        fs::write(&lost, "{}").unwrap();
        let file = fs::File::open(&lost).unwrap();
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(2000))
            .unwrap();
        let desired = vec!["lane-a".to_owned()];
        let rows = archived_rows(&registry, &desired, Some(1000));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "fail");
        assert_eq!(rows[0]["subject"], "lane-a");
        // Archived before this boot is history, not an outage loss.
        let rows = archived_rows(&registry, &desired, Some(3000));
        assert_eq!(rows[0]["status"], "pass");
        let _ = fs::remove_dir_all(&registry);
    }

    #[test]
    fn wake_argv_is_one_transient_timer_per_team() {
        let request = crate::restore::WakeRequest {
            team_key: "0123abcd".to_owned(),
            registry: PathBuf::from("/team/.agentctl"),
            deadline_unix: 1900.0,
            argv: vec![
                "/bin/agentctl".to_owned(),
                "--registry".to_owned(),
                "/team/.agentctl".to_owned(),
                "team".to_owned(),
                "kick".to_owned(),
            ],
        };
        let argv = crate::restore::wake_argv(&request, 1000.0);
        assert_eq!(
            argv[..5],
            [
                "systemd-run",
                "--user",
                "--on-active=900s",
                "--unit",
                "agentctl-team-wake@0123abcd"
            ]
        );
        assert_eq!(argv[5..], request.argv[..]);
        // A past deadline still wakes promptly.
        assert!(crate::restore::wake_argv(&request, 5000.0).contains(&"--on-active=1s".to_owned()));
    }

    #[test]
    fn run_stdin_pipes_the_incident() {
        let host = crate::restore::SystemHost;
        let outcome = host.run_stdin(
            &["cat".to_owned()],
            None,
            Duration::from_secs(10),
            "incident text",
        );
        assert_eq!(outcome.status, Some(0));
        assert_eq!(outcome.stdout, "incident text");
        // Plain run keeps a null stdin: cat sees EOF at once.
        let outcome = host.run(&["cat".to_owned()], None, Duration::from_secs(10));
        assert_eq!(outcome.status, Some(0));
        assert_eq!(outcome.stdout, "");
    }
}
