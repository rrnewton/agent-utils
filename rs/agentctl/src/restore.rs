//! Bring a coordinator team back after the host restarts, in one idempotent command.
//!
//! `agentctl restore` runs these steps in order and writes what each one did to a JSON report
//! named for the boot:
//!
//! 1. Hold `<registry>/.restore.lock`, so a boot unit and a manual run never overlap: the
//!    second waits for the first, then finds its completed report.
//! 2. Stop early, successfully, when this boot's report already says the restore completed.
//! 3. Run the project's slot manager once (`wrkslots image status`), which remounts every
//!    image-backed slot before any agent starts in one.
//! 4. Wait for the Herdr server to answer.
//! 5. Revive the coordinator from its launch spec, then register its new pane in every other
//!    registry that names it (for example a chat bridge's).
//! 6. Revive every other dead agent one at a time with a startup delay; promote a live
//!    `launch_failed` record; record every refusal with its reason.
//! 7. `systemctl --user reset-failed` and `start` each configured unit (the chat bridge).
//! 8. Queue one deterministic recovery notice for the coordinator, once per boot.
//!
//! A live harness is skipped and a published revive is never repeated, so a rerun only does
//! what is still missing. The configuration is `<registry>/restore.json`.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent::{AgentError, DrainOptions};
use crate::profiles::UnknownKeys;
use crate::subagents::{AdoptOptions, ManagedAgents, ManagedApi, StopOptions};

/// File name of the restore configuration inside the agent registry.
pub const CONFIG_FILE: &str = "restore.json";
/// Schema of the restore configuration.
pub const CONFIG_SCHEMA: &str = "agentctl-restore/v1";
/// Schema of the per-boot report.
pub const REPORT_SCHEMA: &str = "agentctl-restore-report/v1";

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// Another registry that names the coordinator, such as a chat bridge's.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AlsoRegistered {
    /// The other agent registry directory.
    pub registry: PathBuf,
    /// The coordinator's name in that registry.
    pub name: String,
}

/// A timer the watch keeps firing.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimerWatch {
    /// The user timer unit, for example `health-tick.timer`.
    pub unit: String,
    /// Alarm when the timer last fired longer ago than this many seconds.
    pub max_age_seconds: u64,
}

fn default_startup_delay() -> u64 {
    20
}

fn default_herdr_wait() -> u64 {
    300
}

/// `<registry>/restore.json`. An unknown top-level key is warned about and ignored rather than
/// refused (see [`RestoreConfig::load`]), so a key a newer agentctl adds never stops an older one
/// restoring the registry; keys inside `also_registered` and `timers` entries stay strict.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RestoreConfig {
    /// Always `agentctl-restore/v1`.
    pub schema: String,
    /// The coordinator's name in this registry; revived first.
    #[serde(default)]
    pub coordinator: Option<String>,
    /// Other registries that name the coordinator.
    #[serde(default)]
    pub also_registered: Vec<AlsoRegistered>,
    /// User units to reset and start after the agents, for example the chat bridge.
    #[serde(default)]
    pub units: Vec<String>,
    /// Registered names restore leaves alone and does not count, for example a stale record.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Seconds between two lane launches (default 20).
    #[serde(default = "default_startup_delay")]
    pub startup_delay_seconds: u64,
    /// Seconds to wait for the Herdr server (default 300).
    #[serde(default = "default_herdr_wait")]
    pub herdr_wait_seconds: u64,
    /// Directory for per-boot reports (default `$XDG_STATE_HOME/agentctl/restore`).
    #[serde(default)]
    pub report_dir: Option<PathBuf>,
    /// The unit that runs the Herdr server, which the watch starts when Herdr is down.
    #[serde(default)]
    pub herdr_unit: Option<String>,
    /// Timers the watch keeps firing.
    #[serde(default)]
    pub timers: Vec<TimerWatch>,
    /// Top-level keys this build does not know, named in a warning and otherwise ignored.
    #[serde(flatten)]
    pub(crate) unknown: UnknownKeys,
}

impl RestoreConfig {
    /// Read and validate `<registry>/restore.json`; an absent file is an empty configuration.
    pub fn load(registry: &Path) -> Result<Self> {
        let path = registry.join(CONFIG_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::empty());
            }
            Err(error) => {
                return Err(fail(format!("cannot read {}: {error}", path.display())));
            }
        };
        let config: Self = serde_json::from_str(&text)
            .map_err(|error| fail(format!("invalid {}: {error}", path.display())))?;
        if let Some(names) = config.unknown.names() {
            eprintln!(
                "agentctl: warning: {} has top-level key(s) this agentctl does not know: \
                 {names}; ignoring them (a newer agentctl may use them)",
                path.display()
            );
        }
        config.validate(&path)?;
        Ok(config)
    }

    /// Take the registry's designated coordinator (`agentctl coordinator set`) when
    /// `restore.json` names none. A `restore.json` coordinator that disagrees with it, or a
    /// designated coordinator that `exclude` lists, is an error, and the configuration keeps its
    /// own coordinator (or none), so a caller can report the problem and carry on.
    pub fn follow_designation(&mut self, designated: Option<String>) -> Result<()> {
        match (&self.coordinator, designated) {
            (None, Some(designated)) if self.exclude.contains(&designated) => Err(fail(format!(
                "{CONFIG_FILE} excludes {designated:?}, the designated coordinator"
            ))),
            (None, designated) => {
                self.coordinator = designated;
                Ok(())
            }
            (Some(named), Some(designated)) if *named != designated => Err(fail(format!(
                "{CONFIG_FILE} names coordinator {named:?} but the registry designates \
                 {designated:?}; make them agree (agentctl coordinator set {named}), or drop the \
                 coordinator key from {CONFIG_FILE}"
            ))),
            _ => Ok(()),
        }
    }

    fn empty() -> Self {
        Self {
            schema: CONFIG_SCHEMA.to_owned(),
            coordinator: None,
            also_registered: Vec::new(),
            units: Vec::new(),
            exclude: Vec::new(),
            startup_delay_seconds: default_startup_delay(),
            herdr_wait_seconds: default_herdr_wait(),
            report_dir: None,
            herdr_unit: None,
            timers: Vec::new(),
            unknown: UnknownKeys::default(),
        }
    }

    fn validate(&self, path: &Path) -> Result<()> {
        let invalid = |detail: &str| fail(format!("invalid {} ({detail})", path.display()));
        if self.schema != CONFIG_SCHEMA {
            return Err(invalid("schema must be agentctl-restore/v1"));
        }
        if self
            .coordinator
            .as_deref()
            .is_some_and(|name| !agent_name(name))
        {
            return Err(invalid("coordinator is not an agent name"));
        }
        if self
            .exclude
            .iter()
            .any(|name| !agent_name(name) || self.coordinator.as_deref() == Some(name.as_str()))
        {
            return Err(invalid("exclude names agents, and never the coordinator"));
        }
        if self
            .also_registered
            .iter()
            .any(|other| !agent_name(&other.name) || !other.registry.is_absolute())
        {
            return Err(invalid(
                "also_registered needs an absolute registry and an agent name",
            ));
        }
        if self
            .units
            .iter()
            .chain(self.herdr_unit.iter())
            .chain(self.timers.iter().map(|timer| &timer.unit))
            .any(|unit| !unit_name(unit))
        {
            return Err(invalid("a unit name is not a plain systemd unit name"));
        }
        if self
            .timers
            .iter()
            .any(|timer| !timer.unit.ends_with(".timer") || timer.max_age_seconds == 0)
        {
            return Err(invalid(
                "timers need a .timer unit and a positive max_age_seconds",
            ));
        }
        if self.startup_delay_seconds > 600 || self.herdr_wait_seconds > 3600 {
            return Err(invalid(
                "startup_delay_seconds is at most 600 and herdr_wait_seconds at most 3600",
            ));
        }
        if self
            .report_dir
            .as_ref()
            .is_some_and(|dir| !dir.is_absolute())
        {
            return Err(invalid("report_dir must be absolute"));
        }
        Ok(())
    }
}

fn agent_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
        && value != "archive"
}

/// A unit name systemctl takes literally: no path, option, or glob.
pub(crate) fn unit_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && value.contains('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"@._-:\\".contains(&byte))
}

/// What a command printed and how it ended.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CommandOutcome {
    /// Exit status, or `None` when it could not run or ran out of time.
    pub status: Option<i32>,
    /// Captured standard output.
    pub stdout: String,
    /// Captured standard error, or why the command could not run.
    pub stderr: String,
}

/// The host facts and effects restore and watch use, injectable for tests.
pub trait Host {
    /// `/proc/sys/kernel/random/boot_id`.
    fn boot_id(&self) -> Result<String>;
    /// The kernel boot time in Unix seconds (`btime` in `/proc/stat`).
    fn boot_time(&self) -> Option<u64>;
    /// Seconds since the Unix epoch.
    fn now(&self) -> f64;
    /// Pause.
    fn sleep(&self, duration: Duration);
    /// Run a program with a deadline, never through a shell.
    fn run(&self, argv: &[String], cwd: Option<&Path>, timeout: Duration) -> CommandOutcome;
}

/// The real host.
pub struct SystemHost;

impl Host for SystemHost {
    fn boot_id(&self) -> Result<String> {
        let text = fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|error| fail(format!("cannot read the boot id: {error}")))?;
        let value = text.trim();
        if value.len() == 36
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            Ok(value.to_owned())
        } else {
            Err(fail("the boot id is malformed"))
        }
    }

    fn boot_time(&self) -> Option<u64> {
        fs::read_to_string("/proc/stat")
            .ok()?
            .lines()
            .find_map(|line| {
                line.strip_prefix("btime ")
                    .and_then(|value| value.trim().parse().ok())
            })
    }

    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn run(&self, argv: &[String], cwd: Option<&Path>, timeout: Duration) -> CommandOutcome {
        let Some((program, arguments)) = argv.split_first() else {
            return CommandOutcome {
                stderr: "empty command".to_owned(),
                ..CommandOutcome::default()
            };
        };
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return CommandOutcome {
                    stderr: format!("cannot run {program}: {error}"),
                    ..CommandOutcome::default()
                }
            }
        };
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return CommandOutcome {
                        stderr: format!("{program} did not finish within {}s", timeout.as_secs()),
                        ..CommandOutcome::default()
                    };
                }
            }
        }
        match child.wait_with_output() {
            Ok(output) => CommandOutcome {
                status: output.status.code(),
                stdout: bounded_text(&output.stdout),
                stderr: bounded_text(&output.stderr),
            },
            Err(error) => CommandOutcome {
                stderr: format!("cannot read {program}: {error}"),
                ..CommandOutcome::default()
            },
        }
    }
}

/// Keep reports bounded: the last 16 KiB of a command's output.
fn bounded_text(bytes: &[u8]) -> String {
    const LIMIT: usize = 16 << 10;
    let start = bytes.len().saturating_sub(LIMIT);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

/// How restore was asked to run.
#[derive(Clone, Debug, Default)]
pub struct RestoreOptions {
    /// Run even when this boot's report says a restore completed.
    pub force: bool,
    /// Report the plan without reviving, promoting, registering, starting units, or notifying.
    pub dry_run: bool,
    /// Startup deadline for each revived harness.
    pub startup_timeout: Duration,
    /// The slot manager, when the project configures one (see `profiles.json` `wrkslots`).
    pub wrkslots: Option<PathBuf>,
}

/// The default report directory: `$XDG_STATE_HOME/agentctl/restore`, else under the home.
pub fn default_report_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })
        .map(|state| state.join("agentctl/restore"))
}

fn report_path(config: &RestoreConfig, boot_id: &str) -> Result<PathBuf> {
    let directory = config
        .report_dir
        .clone()
        .or_else(default_report_dir)
        .ok_or_else(|| {
            fail("no report directory: set report_dir in restore.json, XDG_STATE_HOME, or HOME")
        })?;
    Ok(directory.join(format!("{boot_id}.json")))
}

/// Read a previous report for this boot, if one exists and parses.
pub fn read_report(path: &Path) -> Option<Value> {
    let mut text = String::new();
    File::open(path)
        .ok()?
        .take(1 << 20)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

fn write_report(path: &Path, report: &Value) -> Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| fail("report path has no directory"))?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| fail(format!("cannot create {}: {error}", directory.display())))?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("report"),
        std::process::id()
    ));
    let mut bytes = serde_json::to_vec_pretty(report).map_err(|error| fail(error.to_string()))?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| fail(format!("cannot write {}: {error}", temporary.display())))?;
    fs::rename(&temporary, path)
        .map_err(|error| fail(format!("cannot publish {}: {error}", path.display())))
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

/// `wrkslots image status`: every wrkslots command remounts unmounted image slots on load.
fn remount_slot_images(host: &dyn Host, wrkslots: Option<&Path>, project: &Path) -> Value {
    let Some(wrkslots) = wrkslots else {
        return json!({"action": "skipped", "reason": "no wrkslots configured or on PATH"});
    };
    let argv = vec![
        wrkslots.display().to_string(),
        "image".to_owned(),
        "status".to_owned(),
    ];
    let outcome = host.run(&argv, Some(project), Duration::from_secs(120));
    let mounted = outcome
        .stderr
        .lines()
        .filter_map(|line| line.strip_prefix("wrkslots: mounted "))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // A failed mount is only a warning with exit status 0, so read the warning itself.
    let warnings = outcome
        .stderr
        .lines()
        .filter(|line| line.contains("slot image not mounted"))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let image_backed = if outcome.status == Some(0) {
        image_backed_slots(&outcome.stdout).map_or(Value::Null, |count| json!(count))
    } else {
        Value::Null
    };
    json!({
        "action": if outcome.status == Some(0) && warnings.is_empty() { "ran" } else { "failed" },
        "command": argv,
        "project": project,
        "status": outcome.status,
        "image_backed": image_backed,
        "mounted": mounted,
        "warnings": warnings,
        "detail": if outcome.status == Some(0) { Value::Null } else { json!(outcome.stderr.trim()) },
    })
}

/// How many image-backed slots `wrkslots image status` listed, when its output says.
///
/// The human format prints `no image-backed slots`, or one unindented
/// `TYPE/NAME at LOCATION (PHASE)` line per slot with indented detail lines under it.
fn image_backed_slots(stdout: &str) -> Option<usize> {
    if stdout
        .lines()
        .any(|line| line.trim() == "no image-backed slots")
    {
        return Some(0);
    }
    let count = stdout
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter(|line| {
            line.split_once(" at ")
                .is_some_and(|(slot, _)| slot.contains('/'))
                && line.trim_end().ends_with(')')
        })
        .count();
    (count > 0).then_some(count)
}

/// The first two meaningful lines of a command's error output, for a one-line message.
fn first_error_lines(stderr: &str) -> String {
    const WIDTH: usize = 240;
    let lines = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("wrkslots: mounted "))
        .take(2)
        .map(|line| {
            if line.chars().count() > WIDTH {
                format!("{}...", line.chars().take(WIDTH).collect::<String>())
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "no error output".to_owned()
    } else {
        lines.join(" / ")
    }
}

/// One self-explanatory line about a slot-image check: what ran, what it found, what to do.
fn describe_slot_images(images: &Value) -> String {
    let command = images["command"].as_array().map_or_else(
        || "wrkslots image status".to_owned(),
        |argv| {
            argv.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ")
        },
    );
    let project = images["project"]
        .as_str()
        .unwrap_or("the project directory");
    let mounted = images["mounted"]
        .as_array()
        .map(|rows| rows.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let warnings = images["warnings"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(Value::as_str)
                .map(|line| {
                    line.split_once("slot image not mounted:")
                        .map_or(line, |(_, rest)| rest)
                        .trim()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let remounted = if mounted.is_empty() {
        String::new()
    } else {
        format!(
            "; remounted {} first: {}",
            mounted.len(),
            mounted.join(", ")
        )
    };
    match images["action"].as_str() {
        Some("skipped") => format!(
            "slot images not checked: {}",
            images["reason"].as_str().unwrap_or("no reason recorded")
        ),
        Some("ran") => {
            if !mounted.is_empty() {
                format!(
                    "remounted {} slot image(s): {}",
                    mounted.len(),
                    mounted.join(", ")
                )
            } else {
                match images["image_backed"].as_u64() {
                    Some(0) => "no image-backed slots".to_owned(),
                    Some(count) => {
                        format!("all {count} image-backed slot(s) already mounted")
                    }
                    None => "no slot image needed remounting".to_owned(),
                }
            }
        }
        _ => match images["status"].as_i64() {
            Some(0) => format!(
                "{} slot image warning(s) from `{command}`: {}{remounted}; run `{command}` in {project} for details",
                warnings.len(),
                if warnings.is_empty() {
                    "no warning text".to_owned()
                } else {
                    warnings.join("; ")
                }
            ),
            status => format!(
                "`{command}` {} in {project}: {}{remounted}; slot images were not checked or remounted",
                status.map_or_else(
                    || "did not finish (it could not start or timed out)".to_owned(),
                    |code| format!("exited {code}")
                ),
                first_error_lines(images["detail"].as_str().unwrap_or(""))
            ),
        },
    }
}

fn wait_for_herdr<A: ManagedApi + ?Sized>(client: &A, host: &dyn Host, wait: Duration) -> Value {
    let started = host.now();
    loop {
        match client.panes() {
            Ok(_) => {
                return json!({"ready": true, "waited_seconds": (host.now() - started).round()});
            }
            Err(error) => {
                if host.now() - started >= wait.as_secs_f64() {
                    return json!({
                        "ready": false,
                        "waited_seconds": (host.now() - started).round(),
                        "detail": error.to_string(),
                    });
                }
                host.sleep(Duration::from_secs(2));
            }
        }
    }
}

/// Bring one agent back: revive it when dead, promote it when its failed launch runs.
fn recover_agent<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    name: &str,
    options: &RestoreOptions,
) -> Value {
    let status = manager.status(name).ok();
    let lifecycle = status
        .as_ref()
        .and_then(|value| value["lifecycle"].as_str().map(str::to_owned));
    if lifecycle.as_deref() == Some("launch_failed") {
        if options.dry_run {
            return json!({"name": name, "action": "promote", "dry_run": true});
        }
        let token = status
            .as_ref()
            .and_then(|value| value["token"].as_str().map(str::to_owned));
        return match manager.promote(name, token.as_deref()) {
            Ok(result) => json!({"name": name, "action": "promoted", "pane_id": result["pane_id"]}),
            Err(error) => json!({"name": name, "action": "refused", "reason": error.to_string()}),
        };
    }
    let plan = match manager.revive(name, true, None, options.startup_timeout) {
        Ok(plan) => plan,
        Err(error) => {
            return json!({"name": name, "action": "refused", "reason": error.to_string()});
        }
    };
    // A conversation that cannot be resumed never blocks a restore: the agent starts fresh,
    // and the report, the coordinator's notice and the agent's first prompt say so loudly
    // (owner decision, #686 agent-turnover). A pending revive journal finishes as it began.
    let unresumable = if plan["action"] == "revive" {
        manager.revive_unresumable_reason(name).ok().flatten()
    } else {
        None
    };
    match plan["action"].as_str().unwrap_or_default() {
        "revive" if options.dry_run && unresumable.is_some() => json!({
            "name": name, "action": "revive-fresh", "dry_run": true, "reason": unresumable,
        }),
        "revive" | "recover" if options.dry_run => {
            json!({"name": name, "action": "revive", "dry_run": true})
        }
        "revive" if unresumable.is_some() => {
            let reason = unresumable.unwrap_or_default();
            revive_fresh(manager, name, &plan, &reason, options)
        }
        "revive" | "recover" => {
            let token = plan["token"].as_str().map(str::to_owned);
            match manager.revive(name, false, token.as_deref(), options.startup_timeout) {
                Ok(result) => json!({
                    "name": name,
                    "action": "revived",
                    "pane_id": result["pane_id"],
                    "workspace_id": result["workspace_id"],
                }),
                Err(error) => {
                    json!({"name": name, "action": "refused", "reason": error.to_string()})
                }
            }
        }
        "skip" => json!({"name": name, "action": "skipped", "reason": plan["reason"]}),
        _ => json!({"name": name, "action": "refused", "reason": plan["reason"]}),
    }
}

/// Revive `name` on a fresh conversation because `reason` says its own cannot be resumed; warn
/// the agent in its first prompt and record the lineage.
fn revive_fresh<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    name: &str,
    plan: &Value,
    reason: &str,
    options: &RestoreOptions,
) -> Value {
    let old = manager.status(name).ok();
    let session = match manager.mark_fresh_revive(name, reason) {
        Ok(session) => session,
        Err(error) => {
            return json!({"name": name, "action": "refused",
                          "reason": format!("{reason}; a fresh start was refused: {error}")});
        }
    };
    let token = plan["token"].as_str().map(str::to_owned);
    let result = match manager.revive(name, false, token.as_deref(), options.startup_timeout) {
        Ok(result) => result,
        Err(error) => {
            return json!({"name": name, "action": "refused",
                          "reason": format!("{reason}; the fresh start failed: {error}")});
        }
    };
    let old_session = old.as_ref().and_then(crate::lineage::session_of);
    let warning = format!(
        "WARNING: {name} was started FRESH, with no memory of its previous conversation{}: {reason}. Give it its task and context again.",
        old_session
            .as_deref()
            .map(|session| format!(" ({session})"))
            .unwrap_or_default()
    );
    let prompt = format!(
        "{}\nWARNING: this is a fresh session. Your previous conversation{} could not be resumed after the host restarted ({reason}), so you remember nothing of it. Before doing anything else, read your working tree, your notes and your handoff files, and tell your coordinator what you can and cannot reconstruct.",
        crate::sender_header::render(
            &crate::sender_header::Sender::Other("agentctl-restore".to_owned()),
            crate::sender_header::ROUTE,
        ),
        old_session
            .as_deref()
            .map(|session| format!(" ({session})"))
            .unwrap_or_default()
    );
    let prompted = manager.send_identified(
        name,
        &prompt,
        DrainOptions {
            ready_timeout: Duration::from_secs(60),
            ..DrainOptions::default()
        },
        Some(&format!(
            "restore-fresh-{}",
            &session[..8.min(session.len())]
        )),
    );
    if let Ok(status) = manager.status(name) {
        let token = status["token"].as_str().unwrap_or_default().to_owned();
        let mut entry = serde_json::Map::new();
        for (key, value) in [
            ("kind", json!("generation")),
            ("relation", json!("fresh-after-restore")),
            ("reason", json!(reason)),
            ("token", json!(token)),
            ("session", json!(session)),
            ("harness", status["harness"].clone()),
            ("cwd", status["cwd"].clone()),
            (
                "predecessor",
                json!({"token": plan["token"], "session": old_session}),
            ),
        ] {
            if !value.is_null() {
                entry.insert(key.to_owned(), value);
            }
        }
        if let Err(error) = crate::lineage::append(manager.registry(), name, entry) {
            eprintln!("agentctl: lineage of {name} not recorded: {error}");
        }
    }
    json!({
        "name": name,
        "action": "revived-fresh",
        "pane_id": result["pane_id"],
        "workspace_id": result["workspace_id"],
        "session": session,
        "reason": reason,
        "warning": warning,
        "first_prompt": match prompted {
            Ok(queued) => json!(queued)["outcome"].clone(),
            Err(error) => json!(format!("not queued: {error}")),
        },
    })
}

/// Point another registry's record for the coordinator at the coordinator's live pane.
///
/// The same pane only needs its anchors replaced. A record pinned to a pane the revive left
/// behind is an adopted record whose harness is dead: it is retired (bound to its exact token
/// and bytes) and the live pane adopted under the same name.
fn register_elsewhere<A: ManagedApi + ?Sized>(
    client: &A,
    other: &AlsoRegistered,
    coordinator: &Value,
    dry_run: bool,
) -> Value {
    let pane = coordinator["pane_id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let result = (|| -> Result<Value> {
        let manager = ManagedAgents::new(client, &other.registry)?;
        let record_path =
            agent_queue::registry::directory(&other.registry, &other.name)?.join("agent.json");
        let existing = match fs::read(&record_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(fail(format!(
                    "cannot read {}: {error}",
                    record_path.display()
                )))
            }
        };
        if let Some(bytes) = existing.as_ref() {
            let record: Value = serde_json::from_slice(bytes)
                .map_err(|error| fail(format!("invalid {}: {error}", record_path.display())))?;
            if record["pane_id"] == pane.as_str() {
                if dry_run {
                    return Ok(json!({"action": "anchor", "dry_run": true}));
                }
                manager.anchor(&other.name, true, None)?;
                return Ok(json!({"action": "anchored"}));
            }
            if dry_run {
                return Ok(json!({"action": "re-adopt", "dry_run": true}));
            }
            let token = record["token"].as_str().map(str::to_owned);
            manager.stop_with_options(
                &other.name,
                StopOptions {
                    expected_token: token,
                    retire_dead_adoption: true,
                    expected_record_sha256: Some(format!("{:x}", Sha256::digest(bytes))),
                    ..StopOptions::default()
                },
            )?;
        } else if dry_run {
            return Ok(json!({"action": "adopt", "dry_run": true}));
        }
        let workspace = coordinator["workspace_id"].as_str().unwrap_or_default();
        let label = client.workspace_label(workspace)?;
        manager.adopt(
            &other.name,
            AdoptOptions {
                pane_id: pane.clone(),
                expected_workspace: label,
                cwd: PathBuf::from(coordinator["cwd"].as_str().unwrap_or_default()),
                harness: coordinator["harness"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                session: None,
                // A second registration of the coordinator is no worker of whoever runs restore.
                record_edges: false,
            },
        )?;
        Ok(json!({"action": "adopted"}))
    })();
    let mut row = match result {
        Ok(value) => value,
        Err(error) => json!({"action": "refused", "reason": error.to_string()}),
    };
    row["registry"] = json!(other.registry);
    row["name"] = json!(other.name);
    row["pane_id"] = json!(pane);
    row
}

/// A running agent boxed to a wrkslots slot: `(agent, slot, project)`.
#[derive(Clone, Debug)]
struct SlotAgent {
    name: String,
    slot: String,
    project: PathBuf,
    /// A running record whose harness may have died; a failed launch is not held.
    running: bool,
}

/// The hold restore places on a dead agent's slot until the agent is back and owns it again.
fn reboot_hold_reason(name: &str) -> String {
    format!("reboot: agent {name} waits for agentctl restore")
}

fn slot_agents(registry: &Path, names: &[String], config: &RestoreConfig) -> Vec<SlotAgent> {
    names
        .iter()
        .filter(|name| !config.exclude.contains(name))
        .filter_map(|name| {
            let bytes = fs::read(record_path(registry, name)?).ok()?;
            let record: Value = serde_json::from_slice(&bytes).ok()?;
            let lifecycle = record["lifecycle"].as_str()?;
            matches!(lifecycle, "running" | "launch_failed").then_some(())?;
            Some(SlotAgent {
                name: name.clone(),
                slot: record["slot"].as_str()?.to_owned(),
                project: PathBuf::from(record["slot_project"].as_str()?),
                running: lifecycle == "running",
            })
        })
        .collect()
}

fn wrkslots_argv(wrkslots: &Path, project: &Path, arguments: &[&str]) -> Vec<String> {
    let mut argv = vec![
        wrkslots.display().to_string(),
        "--project-root".to_owned(),
        project.display().to_string(),
    ];
    argv.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    argv
}

/// One slot's active row in `project`, read with `wrkslots status --slot SLOT --format json`.
fn slot_row(host: &dyn Host, wrkslots: &Path, project: &Path, slot: &str) -> Result<Value> {
    let outcome = host.run(
        &wrkslots_argv(
            wrkslots,
            project,
            &["status", "--slot", slot, "--format", "json"],
        ),
        Some(project),
        Duration::from_secs(120),
    );
    if outcome.status != Some(0) {
        return Err(fail(format!(
            "wrkslots status failed: {}",
            outcome.stderr.trim()
        )));
    }
    let document: Value = serde_json::from_str(&outcome.stdout)
        .map_err(|error| fail(format!("wrkslots status printed no JSON: {error}")))?;
    document["active"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["slot"] == slot))
        .cloned()
        .ok_or_else(|| fail(format!("slot {slot} is not active")))
}

/// Make process `pid` the wrkslots owner of `slot` with `wrkslots transfer-owner`.
///
/// wrkslots accepts it only while the recorded owner is unbound or provably dead, and only for
/// a process in this command's ancestry or inside the slot's own box, which is where a slot
/// launch runs its harness. A busy registry lock is waited on for up to 30 seconds. Returns
/// the slot's status row read before the transfer. A refusal is returned as an error carrying
/// wrkslots' own reason; nothing here retries or overrides it.
pub(crate) fn bind_slot_owner(
    host: &dyn Host,
    wrkslots: &Path,
    project: &Path,
    slot: &str,
    agent: &str,
    pid: u64,
) -> Result<Value> {
    let row = slot_row(host, wrkslots, project, slot)?;
    let generation = row["generation"]
        .as_u64()
        .ok_or_else(|| fail("wrkslots status gave no generation"))?
        .to_string();
    // The slot row names the agent wrkslots registered, which may differ from the agentctl
    // name; transfer-owner checks it against the row.
    let slot_agent = row["agent"].as_str().unwrap_or(agent).to_owned();
    let pid_text = pid.to_string();
    let moved = host.run(
        &wrkslots_argv(
            wrkslots,
            project,
            &[
                "--wait-lock",
                "30",
                "transfer-owner",
                slot,
                "--agent",
                &slot_agent,
                "--owner-pid",
                &pid_text,
                "--expected-generation",
                &generation,
            ],
        ),
        Some(project),
        Duration::from_secs(120),
    );
    if moved.status != Some(0) {
        return Err(fail(format!(
            "transfer-owner refused: {}",
            moved.stderr.trim()
        )));
    }
    Ok(row)
}

/// Hold a dead agent's slot so that no cleanup can retire it before the agent is back.
fn hold_slot(host: &dyn Host, wrkslots: &Path, agent: &SlotAgent) -> Value {
    let reason = reboot_hold_reason(&agent.name);
    let outcome = host.run(
        &wrkslots_argv(
            wrkslots,
            &agent.project,
            &["hold", &agent.slot, "--reason", &reason],
        ),
        Some(&agent.project),
        Duration::from_secs(120),
    );
    json!({
        "slot": agent.slot,
        "agent": agent.name,
        "action": if outcome.status == Some(0) { "held" } else { "failed" },
        "detail": if outcome.status == Some(0) { json!(outcome.stdout.trim()) } else { json!(outcome.stderr.trim()) },
    })
}

/// Make the revived harness the slot's owner, then drop restore's own reboot hold.
fn transfer_slot<A: ManagedApi + ?Sized>(
    manager: &ManagedAgents<'_, A>,
    host: &dyn Host,
    wrkslots: &Path,
    agent: &SlotAgent,
) -> Value {
    let result = (|| -> Result<Value> {
        let status = manager.status(&agent.name)?;
        let pid = ["harness_identity", "custom_process_identity"]
            .iter()
            .find_map(|key| status[*key]["pid"].as_u64())
            .ok_or_else(|| fail("the revived harness has no pinned process"))?;
        let row = bind_slot_owner(
            host,
            wrkslots,
            &agent.project,
            &agent.slot,
            &agent.name,
            pid,
        )?;
        let mut unheld = false;
        if row["hold_reason"] == reboot_hold_reason(&agent.name).as_str() {
            let released = host.run(
                &wrkslots_argv(wrkslots, &agent.project, &["unhold", &agent.slot]),
                Some(&agent.project),
                Duration::from_secs(120),
            );
            unheld = released.status == Some(0);
        }
        Ok(json!({"action": "transferred", "owner_pid": pid, "unheld": unheld}))
    })();
    match result {
        Ok(value) => value,
        Err(error) => json!({"action": "refused", "reason": error.to_string()}),
    }
}

fn systemctl(host: &dyn Host, verb: &str, unit: &str) -> CommandOutcome {
    host.run(
        &strings(&["systemctl", "--user", verb, unit]),
        None,
        Duration::from_secs(90),
    )
}

/// The registered names, in either registry layout, sorted; restore's own files never look like
/// names.
fn registered_names(registry: &Path) -> Result<Vec<String>> {
    let mut names = agent_queue::registry::names(registry)
        .map_err(|error| fail(format!("cannot list {}: {error}", registry.display())))?;
    names.retain(|name| record_path(registry, name).is_some_and(|path| path.is_file()));
    Ok(names)
}

/// `agent.json` of agent `name`, in whichever layout holds it; `None` for a name in both.
fn record_path(registry: &Path, name: &str) -> Option<PathBuf> {
    agent_queue::registry::directory(registry, name)
        .ok()
        .map(|directory| directory.join("agent.json"))
}

/// Every boot id the registry's anchors were pinned under.
///
/// Only running records count, and only the process each pinned (a native harness, or a custom
/// pane's process), so a stale or stopped record cannot make an ordinary run look like a restart.
fn anchored_boots(registry: &Path, names: &[String]) -> Vec<String> {
    let mut boots = names
        .iter()
        .filter_map(|name| {
            let bytes = fs::read(record_path(registry, name)?).ok()?;
            let record: Value = serde_json::from_slice(&bytes).ok()?;
            (record["lifecycle"] == "running").then_some(())?;
            ["harness_identity", "custom_process_identity"]
                .iter()
                .find_map(|key| record[*key]["boot_id"].as_str().map(str::to_owned))
        })
        .collect::<Vec<_>>();
    boots.sort();
    boots.dedup();
    boots
}

fn notice_text(report: &Value, boot_time: Option<u64>, path: &Path) -> String {
    let rows = |key: &str| report[key].as_array().cloned().unwrap_or_default();
    let mut agents = rows("agents");
    if !report["coordinator"].is_null() {
        agents.insert(0, report["coordinator"].clone());
    }
    let list = |action: &str, detail: &dyn Fn(&Value) -> String| {
        let items = agents
            .iter()
            .filter(|row| row["action"] == action)
            .map(|row| {
                let name = row["name"].as_str().unwrap_or("?");
                match detail(row) {
                    text if text.is_empty() => name.to_owned(),
                    text => format!("{name} {text}"),
                }
            })
            .collect::<Vec<_>>();
        if items.is_empty() {
            "none".to_owned()
        } else {
            items.join("; ")
        }
    };
    let pane = |row: &Value| format!("-> {}", row["pane_id"].as_str().unwrap_or("?"));
    let reason = |row: &Value| {
        format!(
            "({})",
            row["reason"].as_str().unwrap_or("no reason recorded")
        )
    };
    let units = rows("units")
        .iter()
        .map(|row| {
            format!(
                "{} {}",
                row["unit"].as_str().unwrap_or("?"),
                row["action"].as_str().unwrap_or("?")
            )
        })
        .collect::<Vec<_>>();
    let images = &report["slot_images"];
    let registered = rows("also_registered")
        .iter()
        .map(|row| {
            format!(
                "{} in {} {}",
                row["name"].as_str().unwrap_or("?"),
                row["registry"].as_str().unwrap_or("?"),
                row["action"].as_str().unwrap_or("?")
            )
        })
        .collect::<Vec<_>>();
    let restarted = report["restart_detected"] == true;
    let mut lines = vec![
        if restarted {
            format!(
                "[agentctl restore] The host restarted: boot {}{}.",
                report["boot_id"].as_str().unwrap_or("?"),
                boot_time.map_or_else(String::new, |time| format!(
                    ", kernel boot at Unix time {time}"
                ))
            )
        } else {
            format!(
                "[agentctl restore] Restore on the current boot {}; no restart detected (every running agent was pinned on this boot).",
                report["boot_id"].as_str().unwrap_or("?")
            )
        },
        format!("Revived: {}.", list("revived", &pane)),
        format!(
            "STARTED FRESH, no memory of their previous conversation (give them their task again): {}.",
            list("revived-fresh", &reason)
        ),
        format!("Promoted: {}.", list("promoted", &pane)),
        format!("Refused: {}.", list("refused", &reason)),
        format!(
            "Left as they were: {}.",
            list("skipped", &|_| String::new())
        ),
        format!(
            "Excluded by restore.json: {}.",
            list("excluded", &|_| String::new())
        ),
        format!(
            "Coordinator registered elsewhere: {}.",
            if registered.is_empty() {
                "none".to_owned()
            } else {
                registered.join("; ")
            }
        ),
        format!(
            "Units: {}.",
            if units.is_empty() {
                "none".to_owned()
            } else {
                units.join("; ")
            }
        ),
        format!("Slot images: {}.", describe_slot_images(images)),
    ];
    if restarted {
        lines.push(
            "Session crons and background shells did not survive; re-create them from your notes."
                .to_owned(),
        );
    }
    lines.push(format!("Report: {}", path.display()));
    lines.join("\n")
}

/// Run the restore and return its report; the report is also written unless `dry_run`.
pub fn restore<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &RestoreOptions,
) -> Result<Value> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(registry)
        .map_err(|error| fail(format!("cannot create {}: {error}", registry.display())))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(registry.join(".restore.lock"))
        .map_err(|error| fail(format!("cannot open the restore lock: {error}")))?;
    // A second restore waits for the first, then finds its completed report and stops.
    lock.lock_exclusive()
        .map_err(|error| fail(format!("cannot take the restore lock: {error}")))?;
    let boot_id = host.boot_id()?;
    let path = report_path(config, &boot_id)?;
    if !options.force && !options.dry_run {
        if let Some(previous) = read_report(&path) {
            if previous["complete"] == true {
                return Ok(json!({
                    "schema": REPORT_SCHEMA,
                    "boot_id": boot_id,
                    "action": "already-restored",
                    "report": path,
                }));
            }
        }
    }
    let names = registered_names(registry)?;
    let boots = anchored_boots(registry, &names);
    let mut report = json!({
        "schema": REPORT_SCHEMA,
        "boot_id": boot_id,
        "boot_time": host.boot_time(),
        "previous_boots": boots.iter().filter(|boot| **boot != boot_id).collect::<Vec<_>>(),
        "restart_detected": boots.iter().any(|boot| *boot != boot_id),
        "started_at": host.now(),
        "dry_run": options.dry_run,
        "report": path,
    });
    let project = registry.parent().unwrap_or(registry);
    report["slot_images"] = if options.dry_run {
        json!({"action": "skipped", "reason": "dry run"})
    } else {
        remount_slot_images(host, options.wrkslots.as_deref(), project)
    };
    // After a restart, every running agent boxed to a slot is dead: hold its slot so no
    // cleanup can retire it before the agent is revived and owns it again.
    let boxed = slot_agents(registry, &names, config);
    report["slot_holds"] = match options.wrkslots.as_deref() {
        Some(wrkslots) if report["restart_detected"] == true && !options.dry_run => json!(boxed
            .iter()
            .filter(|agent| agent.running)
            .map(|agent| hold_slot(host, wrkslots, agent))
            .collect::<Vec<_>>()),
        _ => json!([]),
    };
    report["herdr"] = wait_for_herdr(client, host, Duration::from_secs(config.herdr_wait_seconds));
    let herdr_ready = report["herdr"]["ready"] == true;
    let manager = ManagedAgents::new(client, registry)?;
    let mut coordinator_ok = config.coordinator.is_none();
    if herdr_ready {
        if let Some(name) = config.coordinator.as_deref() {
            let row = if names.iter().any(|known| known == name) {
                recover_agent(&manager, name, options)
            } else {
                json!({"name": name, "action": "refused", "reason": "the coordinator is not registered"})
            };
            coordinator_ok = matches!(
                row["action"].as_str(),
                Some("revived" | "promoted" | "skipped" | "revive" | "promote")
            );
            report["coordinator"] = row;
            if coordinator_ok {
                let live = manager.status(name).unwrap_or(Value::Null);
                report["also_registered"] = json!(config
                    .also_registered
                    .iter()
                    .map(|other| register_elsewhere(client, other, &live, options.dry_run))
                    .collect::<Vec<_>>());
            }
        }
        let mut agents = Vec::new();
        // Managers come back before the agents they manage, at every depth of the control
        // tree, so a lieutenant is live before its workers' notices are routed to it.
        let ordered = match manager.control_view() {
            Ok((nodes, _, tree_registry)) => crate::subagents::tree_order(
                &nodes,
                &tree_registry,
                config.coordinator.as_deref(),
                &names,
            ),
            Err(_) => names.clone(),
        };
        for name in ordered
            .iter()
            .filter(|name| config.coordinator.as_deref() != Some(name.as_str()))
        {
            if config.exclude.contains(name) {
                agents.push(json!({"name": name, "action": "excluded", "reason": "listed in restore.json exclude"}));
                continue;
            }
            let mut row = recover_agent(&manager, name, options);
            let launched = matches!(row["action"].as_str(), Some("revived"));
            if let (Some(wrkslots), Some(agent), true) = (
                options.wrkslots.as_deref(),
                boxed.iter().find(|agent| agent.name == *name),
                matches!(row["action"].as_str(), Some("revived" | "promoted")),
            ) {
                row["slot_owner"] = transfer_slot(&manager, host, wrkslots, agent);
            }
            agents.push(row);
            if launched && config.startup_delay_seconds > 0 {
                host.sleep(Duration::from_secs(config.startup_delay_seconds));
            }
        }
        report["agents"] = json!(agents);
    }
    let mut units = Vec::new();
    for unit in &config.units {
        if options.dry_run {
            units.push(json!({"unit": unit, "action": "start", "dry_run": true}));
            continue;
        }
        let _ = systemctl(host, "reset-failed", unit);
        let started = systemctl(host, "start", unit);
        units.push(json!({
            "unit": unit,
            "action": if started.status == Some(0) { "started" } else { "failed" },
            "detail": if started.status == Some(0) { Value::Null } else { json!(started.stderr.trim()) },
        }));
    }
    report["units"] = json!(units);
    let refused = report["agents"].as_array().map_or(0, |rows| {
        rows.iter().filter(|row| row["action"] == "refused").count()
    });
    report["refused"] = json!(refused);
    // Loud: every agent that lost its conversation, the coordinator included.
    let fresh_starts: Vec<Value> = std::iter::once(&report["coordinator"])
        .chain(report["agents"].as_array().into_iter().flatten())
        .filter(|row| row["action"] == "revived-fresh")
        .map(|row| row["warning"].clone())
        .collect();
    report["fresh_starts"] = json!(fresh_starts);
    report["complete"] = json!(herdr_ready && coordinator_ok && !options.dry_run);
    // A dry run never completes; this says whether the same run for real would.
    report["would_complete"] = json!(herdr_ready && coordinator_ok && refused == 0);
    report["notice"] = Value::Null;
    if !options.dry_run && herdr_ready && coordinator_ok {
        if let Some(name) = config.coordinator.as_deref() {
            let text = notice_text(&report, host.boot_time(), &path);
            let message_id = format!("restore-{}", &boot_id[..8.min(boot_id.len())]);
            let options = DrainOptions {
                ready_timeout: Duration::from_secs(60),
                deliver_while_working: true,
                ..DrainOptions::default()
            };
            report["notice"] = match manager.send_identified(
                name,
                &text,
                options,
                Some(&message_id),
            ) {
                Ok(result) => {
                    json!({"message_id": message_id, "outcome": json!(result)["outcome"]})
                }
                Err(error) => {
                    json!({"message_id": message_id, "outcome": "not-queued", "detail": error.to_string()})
                }
            };
        }
    }
    report["finished_at"] = json!(host.now());
    if !options.dry_run {
        write_report(&path, &report)?;
    }
    drop(lock);
    Ok(report)
}

/// How `agentctl watch` was asked to run.
#[derive(Clone, Debug)]
pub struct WatchOptions {
    /// Seconds between two passes.
    pub interval: Duration,
    /// Run one pass, print it, and exit.
    pub once: bool,
    /// Report only: start, restart, reset, and remount nothing.
    pub check_only: bool,
    /// The slot manager, when the project configures one.
    pub wrkslots: Option<PathBuf>,
    /// Where `health.json` is written (default `$XDG_STATE_HOME/agentctl/watch`).
    pub state_dir: Option<PathBuf>,
}

/// Seconds a finding waits before the coordinator is told about it again.
const REALERT_SECONDS: f64 = 3600.0;
/// Seconds after boot before a missing restore report is a finding.
const RESTORE_GRACE_SECONDS: u64 = 900;

/// Pause before the one retry of a `systemctl --user` query that got no usable answer.
///
/// Package updates reload or re-execute the user service manager (on 2026-10-26 Chef did so at
/// 03:02 and 08:48 after updating a package that ships user units), and while it does,
/// `systemctl --user` is refused with "Failed to connect to user scope bus via local transport:
/// Connection refused". The same retry also gives a unit in a transient state time to settle.
const SYSTEMCTL_RETRY_DELAY: Duration = Duration::from_secs(5);

fn finding(
    check: &str,
    subject: &str,
    ok: bool,
    detail: impl Into<String>,
    repair: Value,
) -> Value {
    json!({"check": check, "subject": subject, "ok": ok, "detail": detail.into(), "repair": repair})
}

/// A subject the watch could not check, because systemctl gave no usable answer even after one
/// retry. It is neither healthy nor failing (`ok` is null): it is never repaired, never alerted
/// on, and is reported as a warning about the watch itself.
fn unchecked(check: &str, subject: &str, reason: impl Into<String>, repair: Value) -> Value {
    json!({
        "check": check,
        "subject": subject,
        "ok": Value::Null,
        "warning": "could-not-check",
        "detail": format!("could not check: {}", reason.into()),
        "repair": repair,
    })
}

/// Parse `systemctl show --timestamp=unix` `KEY=VALUE` lines.
fn properties(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// Why a systemctl command gave no usable answer.
fn no_answer(command: &str, outcome: &CommandOutcome) -> String {
    let stderr = outcome.stderr.trim();
    match (outcome.status, stderr.is_empty()) {
        (_, false) => format!("{command}: {stderr}"),
        (Some(status), true) => format!("{command}: exit status {status}"),
        (None, true) => format!("{command}: did not run"),
    }
}

/// `systemctl --user show UNIT -p KEY...`, once.
///
/// `show` exits 0 and prints every requested property, even for a unit that does not exist
/// (`LoadState=not-found`), so a nonzero exit, no exit, or a missing property means systemctl
/// could not reach the service manager or was cut off: the answer says nothing about the unit.
fn show_once(
    host: &dyn Host,
    unit: &str,
    keys: &[&str],
) -> std::result::Result<BTreeMap<String, String>, String> {
    let mut argv = strings(&["systemctl", "--user", "show", unit, "--timestamp=unix"]);
    for key in keys {
        argv.push("-p".to_owned());
        argv.push((*key).to_owned());
    }
    let command = format!("systemctl --user show {unit}");
    let outcome = host.run(&argv, None, Duration::from_secs(30));
    if outcome.status != Some(0) {
        return Err(no_answer(&command, &outcome));
    }
    let state = properties(&outcome.stdout);
    match keys.iter().find(|key| !state.contains_key(**key)) {
        Some(missing) => Err(format!("{command}: the answer has no {missing}=")),
        None => Ok(state),
    }
}

/// [`show_once`], retried once after [`SYSTEMCTL_RETRY_DELAY`]; `Err` means "could not check".
fn show(
    host: &dyn Host,
    unit: &str,
    keys: &[&str],
) -> std::result::Result<BTreeMap<String, String>, String> {
    show_once(host, unit, keys).or_else(|_| {
        host.sleep(SYSTEMCTL_RETRY_DELAY);
        show_once(host, unit, keys).map_err(|reason| {
            format!(
                "{reason} (also {}s earlier)",
                SYSTEMCTL_RETRY_DELAY.as_secs()
            )
        })
    })
}

/// A unit's `ActiveState`, or why it could not be checked.
fn active_state(host: &dyn Host, unit: &str) -> std::result::Result<String, String> {
    show(host, unit, &["ActiveState"]).map(|state| state["ActiveState"].clone())
}

/// A check that may not get an answer: `Ok(None)` healthy, `Ok(Some(problem))` a problem the
/// service manager confirmed, `Err(reason)` could not check.
type Check = std::result::Result<Option<String>, String>;

/// Why a timer is not keeping its schedule.
fn timer_problem(host: &dyn Host, timer: &TimerWatch) -> Check {
    let state = show(
        host,
        &timer.unit,
        &[
            "ActiveState",
            "NextElapseUSecRealtime",
            "NextElapseUSecMonotonic",
            "LastTriggerUSec",
        ],
    )?;
    let active = state["ActiveState"].as_str();
    if active != "active" {
        return confirm_inactive_timer(host, timer, active);
    }
    let scheduled = |key: &str| {
        state
            .get(key)
            .is_some_and(|value| !value.is_empty() && value != "infinity" && value != "0")
    };
    // While the service it triggered runs, a timer is in SubState=running and has no next
    // elapse; systemd computes one when the service stops.
    if !scheduled("NextElapseUSecRealtime")
        && !scheduled("NextElapseUSecMonotonic")
        && show(host, &timer.unit, &["SubState"])?["SubState"] != "running"
    {
        return Ok(Some(
            "timer is active but will never fire again (no next elapse)".to_owned(),
        ));
    }
    let last = state
        .get("LastTriggerUSec")
        .and_then(|value| value.strip_prefix('@'))
        .and_then(|value| value.parse::<f64>().ok());
    Ok(match last {
        Some(last) if host.now() - last > timer.max_age_seconds as f64 => Some(format!(
            "last fired {:.0} seconds ago, more than {}",
            host.now() - last,
            timer.max_age_seconds
        )),
        None if host
            .boot_time()
            .is_some_and(|boot| host.now() - boot as f64 > timer.max_age_seconds as f64) =>
        {
            Some("has not fired since boot".to_owned())
        }
        _ => None,
    })
}

/// Look again, after a pause, before calling a timer inactive or unknown.
///
/// A reloading service manager can briefly answer for a timer it has not loaded yet, so the state
/// must hold [`SYSTEMCTL_RETRY_DELAY`] later, and a timer the manager calls not loaded must also be
/// missing from `list-timers --all`.
fn confirm_inactive_timer(host: &dyn Host, timer: &TimerWatch, first: &str) -> Check {
    host.sleep(SYSTEMCTL_RETRY_DELAY);
    let state = show(host, &timer.unit, &["LoadState", "ActiveState"])?;
    let (load, active) = (state["LoadState"].as_str(), state["ActiveState"].as_str());
    if active == "active" {
        return Err(format!(
            "{} was {first}, then active {}s later; checked again next pass",
            timer.unit,
            SYSTEMCTL_RETRY_DELAY.as_secs()
        ));
    }
    if load == "loaded" {
        return Ok(Some(format!("timer is {active}")));
    }
    let command = "systemctl --user list-timers --all";
    let listed = host.run(
        &strings(&[
            "systemctl",
            "--user",
            "list-timers",
            "--all",
            "--no-legend",
            "--plain",
            &timer.unit,
        ]),
        None,
        Duration::from_secs(30),
    );
    if listed.status != Some(0) {
        return Err(no_answer(command, &listed));
    }
    if listed
        .stdout
        .split_whitespace()
        .any(|word| word == timer.unit)
    {
        return Err(format!(
            "systemctl --user show {} says LoadState={load}, but {command} lists it",
            timer.unit
        ));
    }
    Ok(Some(format!(
        "timer is not loaded (LoadState={}) and {command} does not list it",
        if load.is_empty() { "unknown" } else { load }
    )))
}

/// Why the service a timer triggers failed on its last run.
///
/// A timer can keep a perfect schedule while its service fails on every run. Restarting either
/// cannot repair that, so this is reported, never repaired. `Result=` already honours the
/// service's `SuccessExitStatus=`.
fn service_failure(host: &dyn Host, timer: &TimerWatch) -> Check {
    let triggers = show(host, &timer.unit, &["Triggers"])?;
    let Some(service) = triggers.get("Triggers").filter(|value| unit_name(value)) else {
        return Ok(None);
    };
    let state = show(host, service, &["ActiveState", "Result", "ExecMainStatus"])?;
    let result = state["Result"].as_str();
    let active = state["ActiveState"].as_str();
    Ok(
        (active == "failed" || !matches!(result, "" | "success")).then(|| {
            format!(
                "{service} failed on its last run (Result={}, exit status {})",
                if result.is_empty() { "unknown" } else { result },
                if state["ExecMainStatus"].is_empty() {
                    "unknown"
                } else {
                    state["ExecMainStatus"].as_str()
                }
            )
        }),
    )
}

/// One pass of checks, repairs, and findings.
///
/// Only a problem the service manager confirmed is repaired. A unit or timer that systemctl could
/// not check, even after one retry, gets an [`unchecked`] finding and is left alone.
pub fn watch_once<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &WatchOptions,
) -> Vec<Value> {
    let mut findings = Vec::new();
    let herdr_up = client.panes().is_ok();
    if herdr_up {
        findings.push(finding("herdr", "server", true, "answers", Value::Null));
    } else {
        let repair = config.herdr_unit.as_deref().filter(|_| !options.check_only).map(|unit| {
            let outcome = systemctl(host, "start", unit);
            json!({"action": format!("systemctl --user start {unit}"), "status": outcome.status})
        });
        let recovered = repair.is_some() && client.panes().is_ok();
        findings.push(finding(
            "herdr",
            "server",
            recovered,
            if recovered {
                "was down; started"
            } else {
                "does not answer"
            },
            repair.unwrap_or(Value::Null),
        ));
    }
    for unit in &config.units {
        let state = match active_state(host, unit) {
            Err(reason) => {
                findings.push(unchecked("unit", unit, reason, Value::Null));
                continue;
            }
            Ok(state) if state == "active" => {
                findings.push(finding("unit", unit, true, "active", Value::Null));
                continue;
            }
            Ok(state) => state,
        };
        // A unit that is activating or reloading, or a manager mid-reload, settles in seconds.
        host.sleep(SYSTEMCTL_RETRY_DELAY);
        let state = match active_state(host, unit) {
            Err(reason) => {
                findings.push(unchecked(
                    "unit",
                    unit,
                    format!("{unit} was {state}, then {reason}"),
                    Value::Null,
                ));
                continue;
            }
            Ok(now) if now == "active" => {
                findings.push(finding(
                    "unit",
                    unit,
                    true,
                    format!(
                        "active ({state} {}s earlier)",
                        SYSTEMCTL_RETRY_DELAY.as_secs()
                    ),
                    Value::Null,
                ));
                continue;
            }
            Ok(now) => now,
        };
        if options.check_only {
            findings.push(finding(
                "unit",
                unit,
                false,
                format!("not active ({state})"),
                Value::Null,
            ));
            continue;
        }
        let _ = systemctl(host, "reset-failed", unit);
        let started = systemctl(host, "start", unit);
        let repair = json!({"action": format!("systemctl --user reset-failed {unit} && systemctl --user start {unit}"), "status": started.status});
        findings.push(match active_state(host, unit) {
            Ok(now) if now == "active" => {
                finding("unit", unit, true, format!("was {state}; started"), repair)
            }
            Ok(now) => finding(
                "unit",
                unit,
                false,
                format!("not active ({now}): {}", started.stderr.trim()),
                repair,
            ),
            Err(reason) => unchecked(
                "unit",
                unit,
                format!("{unit} was {state} and was started, then {reason}"),
                repair,
            ),
        });
    }
    for timer in &config.timers {
        let problem = match timer_problem(host, timer) {
            Err(reason) => {
                findings.push(unchecked("timer", &timer.unit, reason, Value::Null));
                continue;
            }
            Ok(None) => {
                findings.push(match service_failure(host, timer) {
                    Err(reason) => unchecked("timer", &timer.unit, reason, Value::Null),
                    Ok(Some(failure)) => finding("timer", &timer.unit, false, failure, Value::Null),
                    Ok(None) => finding(
                        "timer",
                        &timer.unit,
                        true,
                        "keeps its schedule",
                        Value::Null,
                    ),
                });
                continue;
            }
            Ok(Some(problem)) => problem,
        };
        if options.check_only {
            findings.push(finding("timer", &timer.unit, false, problem, Value::Null));
            continue;
        }
        let _ = systemctl(host, "restart", &timer.unit);
        let mut repair = vec![format!("systemctl --user restart {}", timer.unit)];
        let mut remaining = timer_problem(host, timer);
        if matches!(remaining, Ok(Some(_))) {
            // A timer whose only trigger counts from its service's last run has no base until
            // that service runs once in this service manager.
            if let Ok(triggers) = show(host, &timer.unit, &["Triggers"]) {
                if let Some(service) = triggers.get("Triggers").filter(|value| unit_name(value)) {
                    let _ = systemctl(host, "start", service);
                    repair.push(format!("systemctl --user start {service}"));
                    remaining = timer_problem(host, timer);
                }
            }
        }
        let remaining = match remaining {
            Ok(None) => service_failure(host, timer),
            other => other,
        };
        findings.push(match remaining {
            Ok(None) => finding(
                "timer",
                &timer.unit,
                true,
                format!("{problem}; repaired"),
                json!({"actions": repair}),
            ),
            Ok(Some(still)) => finding(
                "timer",
                &timer.unit,
                false,
                still,
                json!({"actions": repair}),
            ),
            Err(reason) => unchecked(
                "timer",
                &timer.unit,
                format!(
                    "{problem}; repaired with {}, then {reason}",
                    repair.join(" && ")
                ),
                json!({"actions": repair}),
            ),
        });
    }
    let project = registry.parent().unwrap_or(registry);
    // Any wrkslots command remounts, so a check-only pass leaves slot images alone.
    if let Some(wrkslots) = options.wrkslots.as_deref().filter(|_| !options.check_only) {
        let images = remount_slot_images(host, Some(wrkslots), project);
        let ok = images["action"] == "ran";
        findings.push(finding(
            "slot-images",
            "wrkslots",
            ok,
            describe_slot_images(&images),
            images,
        ));
    }
    if let Ok(boot_id) = host.boot_id() {
        let complete = report_path(config, &boot_id)
            .ok()
            .and_then(|path| read_report(&path))
            .is_some_and(|report| report["complete"] == true);
        let early = host
            .boot_time()
            .is_some_and(|boot| host.now() - (boot as f64) < RESTORE_GRACE_SECONDS as f64);
        if config.coordinator.is_some() {
            findings.push(finding(
                "restore",
                &boot_id,
                complete || early,
                if complete {
                    "this boot's restore completed"
                } else if early {
                    "boot is recent; restore may still run"
                } else {
                    "no completed restore report for this boot"
                },
                Value::Null,
            ));
        }
    }
    if let (Some(name), true) = (config.coordinator.as_deref(), herdr_up) {
        if let Ok(manager) = ManagedAgents::new(client, registry) {
            let plan = manager.revive(name, true, None, Duration::from_secs(30));
            let alive = plan.as_ref().is_ok_and(|plan| {
                plan["action"] == "skip"
                    && plan["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("alive"))
            });
            findings.push(finding(
                "coordinator",
                name,
                alive,
                match &plan {
                    Ok(plan) if alive => "harness alive".to_owned(),
                    Ok(plan) => format!(
                        "{}: {}",
                        plan["action"].as_str().unwrap_or("?"),
                        plan["reason"].as_str().unwrap_or("dead")
                    ),
                    Err(error) => error.to_string(),
                },
                Value::Null,
            ));
        }
    }
    findings
}

fn health_path(options: &WatchOptions) -> Option<PathBuf> {
    options
        .state_dir
        .clone()
        .or_else(|| {
            default_report_dir().and_then(|dir| dir.parent().map(|parent| parent.join("watch")))
        })
        .map(|dir| dir.join("health.json"))
}

/// Tell the coordinator about failing findings, each at most once an hour, when it can hear.
fn alert<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    host: &dyn Host,
    findings: &[Value],
    alerted: &mut BTreeMap<String, f64>,
) -> Value {
    let now = host.now();
    let due = findings
        .iter()
        .filter(|row| row["ok"] == false)
        .filter(|row| {
            let key = format!(
                "{}:{}",
                row["check"].as_str().unwrap_or(""),
                row["subject"].as_str().unwrap_or("")
            );
            alerted
                .get(&key)
                .is_none_or(|last| now - last >= REALERT_SECONDS)
        })
        .collect::<Vec<_>>();
    if due.is_empty() {
        return json!({"sent": false, "reason": "nothing new to report"});
    }
    let coordinator_alive = findings
        .iter()
        .any(|row| row["check"] == "coordinator" && row["ok"] == true);
    let Some(name) = config.coordinator.as_deref().filter(|_| coordinator_alive) else {
        return json!({"sent": false, "reason": "the coordinator cannot hear: Herdr is down, it is dead, or none is configured"});
    };
    let lines = due
        .iter()
        .map(|row| {
            format!(
                "- {} {}: {}",
                row["check"].as_str().unwrap_or("?"),
                row["subject"].as_str().unwrap_or("?"),
                row["detail"].as_str().unwrap_or("")
            )
        })
        .collect::<Vec<_>>();
    let text = format!(
        "[agentctl watch] {} problem(s) it could not repair:\n{}",
        due.len(),
        lines.join("\n")
    );
    let message_id = format!("watch-{}", now as u64);
    let result = ManagedAgents::new(client, registry).and_then(|manager| {
        manager.send_identified(
            name,
            &text,
            DrainOptions {
                ready_timeout: Duration::from_secs(30),
                deliver_while_working: true,
                ..DrainOptions::default()
            },
            Some(&message_id),
        )
    });
    for row in &due {
        let key = format!(
            "{}:{}",
            row["check"].as_str().unwrap_or(""),
            row["subject"].as_str().unwrap_or("")
        );
        alerted.insert(key, now);
    }
    match result {
        Ok(result) => {
            json!({"sent": true, "message_id": message_id, "outcome": json!(result)["outcome"]})
        }
        Err(error) => json!({"sent": false, "message_id": message_id, "reason": error.to_string()}),
    }
}

/// Run the watch: one pass with `once`, otherwise forever, one pass per interval.
pub fn watch<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &WatchOptions,
) -> Result<Value> {
    let path = health_path(options);
    let mut alerted: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["alerted"].clone()).ok())
        .unwrap_or_default();
    let mut pass: u64 = 0;
    loop {
        // Every wrkslots command loads the whole slot registry, so slot images are checked on
        // the first pass and then every tenth.
        let pass_options = WatchOptions {
            wrkslots: options.wrkslots.clone().filter(|_| pass % 10 == 0),
            ..options.clone()
        };
        pass += 1;
        let findings = watch_once(client, registry, host, config, &pass_options);
        let notice = if options.check_only {
            json!({"sent": false, "reason": "check only"})
        } else {
            alert(client, registry, config, host, &findings, &mut alerted)
        };
        let line = |row: &Value| {
            format!(
                "{} {}: {}",
                row["check"].as_str().unwrap_or("?"),
                row["subject"].as_str().unwrap_or("?"),
                row["detail"].as_str().unwrap_or("")
            )
        };
        for row in findings.iter().filter(|row| row["ok"] == false) {
            eprintln!("agentctl watch: {}", line(row));
        }
        // What the watch itself could not check: its own health, not the subject's.
        let warnings = findings
            .iter()
            .filter(|row| row["warning"].is_string())
            .map(line)
            .collect::<Vec<_>>();
        for warning in &warnings {
            eprintln!("agentctl watch: warning: {warning}");
        }
        let health = json!({
            "schema": "agentctl-watch-health/v1",
            "checked_at": host.now(),
            "boot_id": host.boot_id().ok(),
            "ok": findings.iter().all(|row| row["ok"] == true),
            "warnings": warnings,
            "findings": findings,
            "notice": notice,
            "alerted": alerted,
        });
        if let Some(path) = path.as_deref() {
            if let Err(error) = write_report(path, &health) {
                eprintln!("agentctl watch: {error}");
            }
        }
        if options.once {
            return Ok(health);
        }
        host.sleep(options.interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_top_level_keys_are_ignored_but_duplicates_and_nested_keys_are_refused() {
        let directory =
            std::env::temp_dir().join(format!("restore-unknown-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        let load = |text: &str| {
            fs::write(&path, text).unwrap();
            RestoreConfig::load(&directory)
        };
        let config = load(
            r#"{"schema": "agentctl-restore/v1", "units": ["bridge.service"], "zz_future": 1, "a_future": {}}"#,
        )
        .unwrap();
        assert_eq!(config.units, ["bridge.service"]);
        assert_eq!(config.unknown.names().unwrap(), "a_future, zz_future");
        for refused in [
            r#"{"schema": "agentctl-restore/v1", "timers": [{"unit": "t.timer", "max_age_seconds": 60, "surprise": 1}]}"#,
            r#"{"schema": "agentctl-restore/v1", "units": ["a.service"], "units": ["b.service"]}"#,
            r#"{"schema": "agentctl-restore/v1", "also_registered": [{"registry": "/r", "name": "a", "name": "b"}]}"#,
            r#"{"schema": "agentctl-restore/v1", "x": 1, "x": 2}"#,
        ] {
            assert!(load(refused).is_err(), "{refused}");
        }
        let error = load(
            "{\"schema\": \"agentctl-restore/v1\",\n \"timers\": [{\"unit\": \"t.timer\", \"max_age_seconds\": 60, \"surprise\": 1}]}",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("line 2"), "{error}");
        fs::remove_dir_all(&directory).unwrap();
    }
}
