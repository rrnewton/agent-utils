//! Bring a coordinator team back after the host restarts, in one idempotent command.
//!
//! `agentctl restore` runs these steps in order and writes what each one did to a JSON report
//! named for the boot:
//!
//! 1. Hold `<registry>/.restore.lock`, so a boot unit and a manual run never overlap: the
//!    second waits for the first, then finds its completed report.
//! 2. Replay this epoch's recorded report when it already says the restore completed. The
//!    epoch is the kernel boot plus the user manager's identity (#1405), so a user-manager
//!    restart in the same boot runs a real restore instead of replaying the old report.
//! 3. Run the slot manager once per wrkslots project (`wrkslots image status` in the
//!    restore project and in every distinct project recorded by a slotted agent), which
//!    remounts every image-backed slot before any agent starts in one.
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
use serde::{Deserialize, Serialize};
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
/// Watch settings file inside the registry, read on every watch pass (#1145).
pub const WATCH_CONFIG_FILE: &str = "watch.json";
/// Schema of the watch configuration.
pub const WATCH_CONFIG_SCHEMA: &str = "agentctl-watch/v1";
/// Keys whose home is `watch.json`; still read from `restore.json` as fallback.
pub const WATCH_KEYS: [&str; 5] = [
    "duties",
    "herdr_unit",
    "timers",
    "bare_shell_grace_seconds",
    "close_bare_shells",
];
/// File a watch daemon writes at pass start and heartbeats while it runs (#1900):
/// a standalone `pass-progress.json` in its state directory. Older builds ignore it.
pub(crate) const PASS_PROGRESS_FILE: &str = "pass-progress.json";
/// Schema of `pass-progress.json`.
pub(crate) const PASS_PROGRESS_SCHEMA: &str = "agentctl-watch-progress/v1";

/// A watch daemon's progress (#1900, #1914): written at daemon start, before any
/// pass, and a background thread keeps `heartbeat_at` advancing (every 5 s) until
/// this is dropped, so a services health check can prove a daemon that is still
/// in startup, waiting on a lock, or in a long first pass is alive instead of
/// waiting for a pass to complete. `phase` is `starting` until `begin` records
/// the first pass, then `pass`; `daemon_started_at` is when this process's
/// heartbeat began and `pass_started_at` carries it until a pass begins.
/// Best-effort: a failed write is never a watch failure.
pub(crate) struct PassProgress {
    path: Option<PathBuf>,
    daemon_started: f64,
    pass_started: std::sync::Arc<std::sync::Mutex<f64>>,
    phase: std::sync::Arc<std::sync::Mutex<&'static str>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PassProgress {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        let daemon_started = unix_now();
        let pass_started = std::sync::Arc::new(std::sync::Mutex::new(daemon_started));
        let phase = std::sync::Arc::new(std::sync::Mutex::new("starting"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread = path.as_ref().map(|path| {
            Self::write(
                path,
                daemon_started,
                daemon_started,
                daemon_started,
                "starting",
            );
            let thread_pass_started = pass_started.clone();
            let thread_phase = phase.clone();
            let thread_stop = stop.clone();
            let thread_path = path.clone();
            std::thread::spawn(move || {
                while !thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                    for _ in 0..20 {
                        if thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                    let at = *thread_pass_started
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    let phase = *thread_phase
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    Self::write(&thread_path, daemon_started, at, unix_now(), phase);
                }
            })
        });
        Self {
            path,
            daemon_started,
            pass_started,
            phase,
            stop,
            thread,
        }
    }

    /// Record that a pass began at `at` (seconds since the epoch, the caller's clock).
    pub(crate) fn begin(&self, at: f64) {
        *self
            .pass_started
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = at;
        *self
            .phase
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = "pass";
        if let Some(path) = &self.path {
            Self::write(path, self.daemon_started, at, at, "pass");
        }
    }

    fn write(
        path: &Path,
        daemon_started_at: f64,
        pass_started_at: f64,
        heartbeat_at: f64,
        phase: &str,
    ) {
        let report = json!({
            "schema": PASS_PROGRESS_SCHEMA,
            "daemon_started_at": daemon_started_at,
            "pass_started_at": pass_started_at,
            "heartbeat_at": heartbeat_at,
            "phase": phase,
        });
        let _ = write_report(path, &report);
    }
}

impl Drop for PassProgress {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The `pass-progress.json` path for `agentctl watch` with state directory
/// `state_dir` (its default when `None`), beside its `health.json`.
pub(crate) fn watch_progress_path(state_dir: &Option<PathBuf>) -> Option<PathBuf> {
    let directory = state_dir.clone().or_else(|| {
        default_report_dir().and_then(|dir| dir.parent().map(|parent| parent.join("watch")))
    })?;
    Some(directory.join(PASS_PROGRESS_FILE))
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// Per-duty switches for `agentctl watch` (#1145 Phase 1).
pub const DUTY_NAMES: [&str; 8] = [
    "notify",
    "status_requests",
    "tab_hygiene",
    "unit_repair",
    "herdr",
    "slot_images",
    "restore_check",
    "queue_drain",
];

/// Parse one `--duty NAME=on|off` override; unknown names refused.
pub fn parse_duty_override(spec: &str) -> Result<(String, bool)> {
    let (name, value) = spec
        .split_once('=')
        .ok_or_else(|| fail(format!("duty override {spec:?} is not NAME=on|off")))?;
    if !DUTY_NAMES.contains(&name) {
        return Err(fail(format!(
            "unknown duty {name:?}; duties are {}",
            DUTY_NAMES.join(", ")
        )));
    }
    match value {
        "on" => Ok((name.to_owned(), true)),
        "off" => Ok((name.to_owned(), false)),
        _ => Err(fail(format!(
            "duty override {spec:?}: value must be on or off"
        ))),
    }
}

/// Watch duties, each defaulting on (today's behaviour). Unknown keys refused.
/// `status_requests` is accepted but owned by the notifier (`watch-agents`) in
/// Phase 1: it gates nothing `watch` itself does yet.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WatchDuties {
    /// Coordinator alerting about unrepaired findings.
    #[serde(default = "default_true")]
    pub notify: bool,
    /// Status-request pages; notifier-owned in Phase 1 (see type docs).
    #[serde(default = "default_true")]
    pub status_requests: bool,
    /// Canonical tab order and bare-shell closing.
    #[serde(default = "default_true")]
    pub tab_hygiene: bool,
    /// Reset/start configured units and restart/start timers.
    #[serde(default = "default_true")]
    pub unit_repair: bool,
    /// Herdr server check and start of `herdr_unit`.
    #[serde(default = "default_true")]
    pub herdr: bool,
    /// Slot image status/remount.
    #[serde(default = "default_true")]
    pub slot_images: bool,
    /// This boot's restore report check.
    #[serde(default = "default_true")]
    pub restore_check: bool,
    /// Typing queued prompts and stale-queue findings.
    #[serde(default = "default_true")]
    pub queue_drain: bool,
}
impl Default for WatchDuties {
    fn default() -> Self {
        Self {
            notify: true,
            status_requests: true,
            tab_hygiene: true,
            unit_repair: true,
            herdr: true,
            slot_images: true,
            restore_check: true,
            queue_drain: true,
        }
    }
}
impl WatchDuties {
    /// Whether duty `name` is on.
    pub fn enabled(&self, name: &str) -> bool {
        match name {
            "notify" => self.notify,
            "status_requests" => self.status_requests,
            "tab_hygiene" => self.tab_hygiene,
            "unit_repair" => self.unit_repair,
            "herdr" => self.herdr,
            "slot_images" => self.slot_images,
            "restore_check" => self.restore_check,
            "queue_drain" => self.queue_drain,
            _ => false,
        }
    }
    /// Apply one `NAME=on|off` override.
    pub fn apply_override(&mut self, spec: &str) -> Result<()> {
        let (name, on) = parse_duty_override(spec)?;
        match name.as_str() {
            "notify" => self.notify = on,
            "status_requests" => self.status_requests = on,
            "tab_hygiene" => self.tab_hygiene = on,
            "unit_repair" => self.unit_repair = on,
            "herdr" => self.herdr = on,
            "slot_images" => self.slot_images = on,
            "restore_check" => self.restore_check = on,
            "queue_drain" => self.queue_drain = on,
            _ => unreachable!(),
        }
        Ok(())
    }
    /// Duties as a JSON object for `health.json`.
    pub fn as_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// `<registry>/watch.json` (schema `agentctl-watch/v1`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct WatchFile {
    schema: String,
    #[serde(default)]
    duties: Option<WatchDuties>,
    #[serde(default)]
    herdr_unit: Option<String>,
    #[serde(default)]
    timers: Option<Vec<TimerWatch>>,
    #[serde(default)]
    bare_shell_grace_seconds: Option<u64>,
    #[serde(default)]
    close_bare_shells: Option<bool>,
}

type Result<T> = std::result::Result<T, AgentError>;

fn fail(message: impl Into<String>) -> AgentError {
    AgentError::Delivery(message.into())
}

/// Another registry that names the coordinator, such as a chat bridge's.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AlsoRegistered {
    /// The other agent registry directory.
    pub registry: PathBuf,
    /// The coordinator's name in that registry.
    pub name: String,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub unknown: crate::forward_compat::KeptKeys,
}

/// A timer the watch keeps firing.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct TimerWatch {
    /// The user timer unit, for example `health-tick.timer`.
    pub unit: String,
    /// Alarm when the timer last fired longer ago than this many seconds.
    pub max_age_seconds: u64,
    /// Restart this timer even when an operator has stopped it (default false:
    /// an inactive timer is a deliberate stop, reported once and never started
    /// by the watch; #1277 watch-honour-stopped-timer).
    #[serde(default)]
    pub repair_stopped: bool,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub unknown: crate::forward_compat::KeptKeys,
}

/// One scheduled command from the team spec (#743): `agentctl install-units
/// --scheduled` renders it as a persistent `agentctl-scheduled-NAME.service` plus
/// `.timer` user unit pair, so a team's periodic job survives a reboot instead of
/// living in a `systemd-run` transient.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ScheduledUnit {
    /// Slug naming the unit pair (lowercase letters, digits, hyphens).
    pub name: String,
    /// systemd `OnCalendar=` value, for example `daily` or `Mon 06:00`.
    pub on_calendar: String,
    /// The command the timer runs, as an argv whose first element is absolute.
    pub command: Vec<String>,
    /// `TimeoutStartSec=` for the oneshot, in seconds: long enough for a cold
    /// snapshot build plus per-unit health windows on a turnover entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_start_seconds: Option<u64>,
    /// `Environment=` pairs the service needs, such as PATH, HOME and a
    /// designed notify identity (`AGENTCTL_AGENT`).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub environment: std::collections::BTreeMap<String, String>,
    /// Units this service is ordered after (`After=`), such as the local
    /// mirror sync job a turnover must not race.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    /// Exit codes systemd counts as success (`SuccessExitStatus=`): a
    /// turnover's 0 and 74 are both completed outcomes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub success_exit_status: Vec<i32>,
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub unknown: crate::forward_compat::KeptKeys,
}

/// Whether `value` is a calendar schedule systemd can take literally: one line of
/// letters, digits, and the punctuation calendar syntax uses, with no specifier,
/// quote, or command separator.
pub(crate) fn calendar_schedule(value: &str) -> bool {
    // `_` and `+` appear in timezone names (`America/New_York`,
    // `Etc/GMT+5`); `/` separates the area from the locality.
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" -,.:/*_+".contains(&byte))
}

fn default_startup_delay() -> u64 {
    20
}

fn default_herdr_wait() -> u64 {
    300
}

fn default_bare_shell_grace() -> u64 {
    600
}

fn default_true() -> bool {
    true
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
    /// Desired state (#742): when present, only these agents (plus the coordinator) are
    /// restored; every other registered name stays retired, for example finished lanes.
    /// When absent, the desired set is registry-derived (#1129): every record that was
    /// running at shutdown, with `exclude` as the only hand-kept control.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents: Option<Vec<String>>,
    /// User certificate file that must exist before any agent can start (#742): restore
    /// checks it first and, when missing, names the fetch command instead of reviving.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_file: Option<PathBuf>,
    /// Command that fetches the user certificate, printed verbatim when it is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_fetch: Option<String>,
    /// Seconds to wait for a usable user certificate before reviving nothing
    /// (#1137; default: `herdr_wait_seconds`). At boot the certificate is
    /// often renewed moments after the restore unit starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_wait_seconds: Option<u64>,
    /// Expected Herdr version (#742): restore reports a mismatch against the version
    /// the configured Herdr binary prints, so an in-place update cannot lie silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub herdr_version: Option<String>,
    /// Seconds between two lane launches (default 20).
    #[serde(default = "default_startup_delay")]
    pub startup_delay_seconds: u64,
    /// Seconds to wait for the Herdr server (default 300).
    #[serde(default = "default_herdr_wait")]
    pub herdr_wait_seconds: u64,
    /// Directory for per-boot reports (default `REGISTRY/restore/reports`).
    #[serde(default)]
    pub report_dir: Option<PathBuf>,
    /// The unit that runs the Herdr server, which the watch starts when Herdr is down.
    #[serde(default)]
    pub herdr_unit: Option<String>,
    /// Timers the watch keeps firing.
    #[serde(default)]
    pub timers: Vec<TimerWatch>,
    /// Scheduled commands `install-units --scheduled` renders as persistent
    /// timer/service user unit pairs (#743).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scheduled: Vec<ScheduledUnit>,
    /// Seconds an unclaimed bare shell may idle before the watch closes it
    /// (workspace hygiene; default 600).
    #[serde(default = "default_bare_shell_grace")]
    pub bare_shell_grace_seconds: u64,
    /// Whether the watch closes idle bare shells at all (default true;
    /// hand-opened shells are included when true).
    #[serde(default = "default_true")]
    pub close_bare_shells: bool,
    /// Optional per-lane resume brief template (#1131): rendered once per
    /// revived or promoted lane and queued under a per-boot message ID.
    /// Variables are double-curly-braced names drawn only from the facts
    /// restore knows (see the resume-brief variable list); an unknown one
    /// refuses the configuration at load. Absent or blank means no briefs,
    /// exactly as before this key existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_brief: Option<String>,
    /// Watch duties (#1145 Phase 1); home is `watch.json`, read here as fallback.
    #[serde(default)]
    pub duties: WatchDuties,
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
        let unknown = config.unknown_paths();
        if !unknown.is_empty() {
            eprintln!(
                "agentctl: warning: {} has key(s) this agentctl does not know: {}; ignoring \
                 them (a newer agentctl may use them)",
                path.display(),
                unknown.join(", ")
            );
        }
        config.validate(&path)?;
        Ok(config)
    }

    /// Effective configuration for `agentctl watch`: `restore.json` plus
    /// `watch.json` (#1145). `watch.json` wins per key; a watch key set in both
    /// files is refused naming both paths; keys only in `restore.json` still
    /// work with one warning naming `watch.json` as their home. `restore`
    /// itself uses [`Self::load`] and ignores `watch.json`.
    pub fn load_for_watch(registry: &Path) -> Result<Self> {
        let restore_path = registry.join(CONFIG_FILE);
        let watch_path = registry.join(WATCH_CONFIG_FILE);
        let keys_in = |path: &Path| -> Vec<&'static str> {
            let Ok(text) = fs::read_to_string(path) else {
                return Vec::new();
            };
            let Ok(doc) = serde_json::from_str::<Value>(&text) else {
                return Vec::new();
            };
            WATCH_KEYS
                .iter()
                .copied()
                .filter(|k| doc.get(*k).is_some())
                .collect()
        };
        let restore_keys = keys_in(&restore_path);
        let mut config = Self::load(registry)?;
        let watch_text = match fs::read_to_string(&watch_path) {
            Ok(text) => Some(text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(fail(format!("cannot read {}: {e}", watch_path.display()))),
        };
        let Some(watch_text) = watch_text else {
            if !restore_keys.is_empty() {
                eprintln!("agentctl: warning: watch settings ({}) are in {}; their home is now {} (schema {WATCH_CONFIG_SCHEMA}); move them there", restore_keys.join(", "), restore_path.display(), watch_path.display());
            }
            return Ok(config);
        };
        let watch_keys = keys_in(&watch_path);
        for key in &watch_keys {
            if restore_keys.contains(key) {
                return Err(fail(format!("{key} is set in both {} and {}; keep it in one file ({WATCH_CONFIG_FILE} is its home)", watch_path.display(), restore_path.display())));
            }
        }
        if !restore_keys.is_empty() {
            eprintln!("agentctl: warning: watch settings ({}) are in {}; their home is now {} (schema {WATCH_CONFIG_SCHEMA}); move them there", restore_keys.join(", "), restore_path.display(), watch_path.display());
        }
        let watch: WatchFile = serde_json::from_str(&watch_text)
            .map_err(|e| fail(format!("invalid {}: {e}", watch_path.display())))?;
        if watch.schema != WATCH_CONFIG_SCHEMA {
            return Err(fail(format!(
                "invalid {} (schema must be {WATCH_CONFIG_SCHEMA})",
                watch_path.display()
            )));
        }
        // Presence comes from the raw document, so an explicit null/absent
        // distinction for herdr_unit is not needed: Some in file wins.
        if watch_keys.contains(&"duties") {
            if let Some(d) = watch.duties {
                config.duties = d;
            }
        }
        if watch_keys.contains(&"herdr_unit") {
            config.herdr_unit = watch.herdr_unit;
        }
        if watch_keys.contains(&"timers") {
            if let Some(t) = watch.timers {
                config.timers = t;
            }
        }
        if watch_keys.contains(&"bare_shell_grace_seconds") {
            if let Some(v) = watch.bare_shell_grace_seconds {
                config.bare_shell_grace_seconds = v;
            }
        }
        if watch_keys.contains(&"close_bare_shells") {
            if let Some(v) = watch.close_bare_shells {
                config.close_bare_shells = v;
            }
        }
        config.validate(&watch_path)?;
        Ok(config)
    }

    /// The dotted paths of the keys this build does not know, at the top and inside each of
    /// `also_registered`, `scheduled` and `timers` (#809 chat-bridge-json-forward-compat), sorted.
    fn unknown_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self.unknown.keys().cloned().collect();
        for (index, entry) in self.also_registered.iter().enumerate() {
            paths.extend(
                entry
                    .unknown
                    .0
                    .keys()
                    .map(|key| format!("also_registered[{index}].{key}")),
            );
        }
        for (index, entry) in self.scheduled.iter().enumerate() {
            paths.extend(
                entry
                    .unknown
                    .0
                    .keys()
                    .map(|key| format!("scheduled[{index}].{key}")),
            );
        }
        for (index, timer) in self.timers.iter().enumerate() {
            paths.extend(
                timer
                    .unknown
                    .0
                    .keys()
                    .map(|key| format!("timers[{index}].{key}")),
            );
        }
        paths.sort();
        paths
    }

    /// Resolve a configured coordinator that is a live rename alias of the
    /// designated agent to the agent's current name (#1777), so a renamed
    /// coordinator does not make restore refuse its own configuration.
    /// Anything else is left for [`Self::follow_designation`] to judge.
    pub fn resolve_coordinator_alias<A: ManagedApi + ?Sized>(
        &mut self,
        manager: &ManagedAgents<'_, A>,
        designated: Option<&str>,
    ) {
        if let (Some(named), Some(current)) = (&self.coordinator, designated) {
            if let Ok(resolved) = manager.canonical_name(named) {
                if resolved == current {
                    self.coordinator = Some(resolved);
                }
            }
        }
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
            agents: None,
            certificate_file: None,
            certificate_fetch: None,
            certificate_wait_seconds: None,
            herdr_version: None,
            startup_delay_seconds: default_startup_delay(),
            herdr_wait_seconds: default_herdr_wait(),
            report_dir: None,
            herdr_unit: None,
            timers: Vec::new(),
            scheduled: Vec::new(),
            bare_shell_grace_seconds: default_bare_shell_grace(),
            close_bare_shells: default_true(),
            resume_brief: None,
            duties: WatchDuties::default(),
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
        if let Some(agents) = self.agents.as_ref() {
            if agents.iter().any(|name| !agent_name(name)) {
                return Err(invalid("agents names agents"));
            }
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
        let mut scheduled_names = std::collections::BTreeSet::new();
        for entry in &self.scheduled {
            if !agent_name(&entry.name) || !scheduled_names.insert(&entry.name) {
                return Err(invalid("scheduled entries need unique agent-style names"));
            }
            if !calendar_schedule(&entry.on_calendar) {
                return Err(invalid(
                    "scheduled on_calendar is not a plain calendar schedule",
                ));
            }
            let Some(program) = entry.command.first() else {
                return Err(invalid("scheduled command must not be empty"));
            };
            if !Path::new(program).is_absolute()
                || entry.command.iter().any(|argument| {
                    argument.is_empty()
                        || argument
                            .chars()
                            .any(|c| c.is_control() || c == '\n' || c == '\r')
                })
            {
                return Err(invalid(
                    "scheduled command needs an absolute program and plain arguments",
                ));
            }
            if entry
                .timeout_start_seconds
                .is_some_and(|seconds| seconds == 0 || seconds > 86_400)
            {
                return Err(invalid(
                    "scheduled timeout_start_seconds is between 1 and 86400",
                ));
            }
            if entry.after.iter().any(|unit| !unit_name(unit))
                || entry
                    .success_exit_status
                    .iter()
                    .any(|code| !(0..=255).contains(code))
                || entry.environment.iter().any(|(name, value)| {
                    name.is_empty()
                        || !name
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                        || value
                            .chars()
                            .any(|c| c.is_control() || c == '"' || c == '\\')
                })
            {
                return Err(invalid(
                    "scheduled after, success_exit_status and environment must be plain unit names, exit codes 0-255, and NAME=value pairs systemd can carry",
                ));
            }
        }
        if self.startup_delay_seconds > 600
            || self.herdr_wait_seconds > 3600
            || self
                .certificate_wait_seconds
                .is_some_and(|wait| wait > 3600)
        {
            return Err(invalid(
                "startup_delay_seconds is at most 600, herdr_wait_seconds and certificate_wait_seconds at most 3600",
            ));
        }
        if self
            .report_dir
            .as_ref()
            .is_some_and(|dir| !dir.is_absolute())
        {
            return Err(invalid("report_dir must be absolute"));
        }
        if let Some(template) = self.resume_brief.as_deref() {
            resume_brief_variables(template).map_err(|detail| invalid(&detail))?;
        }
        Ok(())
    }
}

/// The only variables a `resume_brief` template may name (#1131): the facts
/// restore itself knows. `report` is the per-boot report's path.
pub(crate) const RESUME_BRIEF_VARIABLES: [&str; 8] = [
    "boot_id",
    "boot_time",
    "finished_at",
    "agent",
    "coordinator",
    "slot",
    "slot_outcome",
    "report",
];

/// Every double-curly-braced variable in a resume-brief template, in order.
///
/// An unclosed opening or a variable outside `RESUME_BRIEF_VARIABLES` is an
/// error naming the variable, so a bad template is refused at configuration
/// load instead of being delivered half-rendered.
pub(crate) fn resume_brief_variables(template: &str) -> std::result::Result<Vec<String>, String> {
    let mut variables = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            return Err("resume_brief has an unclosed {{variable}}".to_owned());
        };
        let name = after[..close].trim();
        if !RESUME_BRIEF_VARIABLES.contains(&name) {
            return Err(format!(
                "resume_brief names unknown variable {{{{{name}}}}}; the variables are {}",
                RESUME_BRIEF_VARIABLES.join(", ")
            ));
        }
        variables.push(name.to_owned());
        rest = &after[close + 2..];
    }
    Ok(variables)
}

/// The resume-brief template in force, when configured and not blank.
pub(crate) fn resume_brief_template(config: &RestoreConfig) -> Option<&str> {
    config
        .resume_brief
        .as_deref()
        .filter(|template| !template.trim().is_empty())
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

/// One restore epoch (#1405 restore-epoch-manager-restart): the kernel boot
/// plus the systemd user manager running in it. A user-manager restart stops
/// every user unit without changing the boot id, so the boot id alone replays
/// a report recorded before the restart and revives nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RestoreEpoch {
    /// `/proc/sys/kernel/random/boot_id`.
    pub boot_id: String,
    /// The user manager's identity in this boot (`invocation:<id>` from
    /// `user@UID.service`, else `pid:<pid>:<start-ticks>`); `None` when no
    /// manager identity can be read (no systemd, or a test host).
    pub user_manager: Option<String>,
}

impl RestoreEpoch {
    /// The epoch as recorded in a report under `epoch`.
    pub fn as_json(&self) -> Value {
        json!({"boot_id": self.boot_id, "user_manager": self.user_manager})
    }

    /// The epoch a report records. A report written before epochs were
    /// recorded has only its top-level `boot_id`, so its manager identity
    /// reads as unknown rather than as any current manager.
    pub fn from_report(report: &Value) -> Option<Self> {
        if let Some(epoch) = report.get("epoch").filter(|epoch| epoch.is_object()) {
            return Some(Self {
                boot_id: epoch["boot_id"].as_str()?.to_owned(),
                user_manager: epoch["user_manager"].as_str().map(str::to_owned),
            });
        }
        Some(Self {
            boot_id: report["boot_id"].as_str()?.to_owned(),
            user_manager: None,
        })
    }

    /// Whether a recorded report belongs to this epoch and may be replayed.
    /// An unknown manager identity on either side matches only an unknown
    /// one on the other, so a report that cannot prove it was recorded
    /// under this manager is never replayed under a known one.
    pub fn matches_report(&self, report: &Value) -> bool {
        Self::from_report(report).is_some_and(|recorded| recorded == *self)
    }

    /// As [`RestoreEpoch::matches_report`], plus the manager-start proof: a
    /// report written before epochs were recorded (no manager identity)
    /// belongs to this epoch when the current manager had already started
    /// when the report finished, which proves it was written under this
    /// manager. Answers `Miss` when the report finished before this
    /// manager started (a restart has happened since), and
    /// `PredatesKeying` when neither can be proven (start or finish time
    /// unknown), so watch can say so distinctly instead of alarming.
    pub fn legacy_match(&self, report: &Value, manager_started: Option<f64>) -> LegacyMatch {
        let Some(recorded) = Self::from_report(report) else {
            return LegacyMatch::Miss;
        };
        if recorded == *self {
            return LegacyMatch::Matches;
        }
        if recorded.boot_id != self.boot_id {
            return LegacyMatch::Miss;
        }
        if recorded.user_manager.is_some() {
            return if recorded == *self {
                LegacyMatch::Matches
            } else {
                LegacyMatch::Miss
            };
        }
        match (manager_started, report["finished_at"].as_f64()) {
            (Some(started), Some(finished)) if finished > started => LegacyMatch::Matches,
            (Some(started), Some(finished)) if finished < started => LegacyMatch::Miss,
            _ => LegacyMatch::PredatesKeying,
        }
    }
}

/// How a recorded report stands against the current epoch, reports
/// written before manager identity was recorded
/// (written before manager identity was recorded) included.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyMatch {
    /// Proven to belong to this epoch.
    Matches,
    /// A pre-epoch report for this boot that nothing can prove either way.
    PredatesKeying,
    /// A different boot or epoch, or no readable epoch at all.
    Miss,
}

/// A one-shot re-entry wake for a team-driver deadline: run `argv` (an absolute
/// agentctl kick) at `deadline_unix` on the [`Host::now`] clock.
#[derive(Clone, Debug, PartialEq)]
pub struct WakeRequest {
    /// Opaque per-team key (a hash prefix); names no team, host, or person.
    pub team_key: String,
    /// The team's canonical registry directory.
    pub registry: PathBuf,
    /// When the wake must fire, in Unix seconds.
    pub deadline_unix: f64,
    /// The command the wake runs, first element absolute.
    pub argv: Vec<String>,
}

/// How arming a [`WakeRequest`] ended. Anything but `Armed` means the caller
/// must wait resident instead of exiting (team-restore liveness).
#[derive(Clone, Debug, PartialEq)]
pub enum WakeOutcome {
    /// A timer will fire, including one already armed for this deadline.
    Armed {
        /// The transient timer unit that will fire.
        unit: String,
    },
    /// No user manager answers, so no timer can be armed against it.
    NoManager,
    /// This host cannot arm timers (the trait default, used by fakes).
    Unsupported,
    /// Arming was attempted and failed; says why.
    Failed {
        /// Why arming failed.
        detail: String,
    },
}

/// The host facts and effects restore and watch use, injectable for tests.
pub trait Host {
    /// `/proc/sys/kernel/random/boot_id`.
    fn boot_id(&self) -> Result<String>;
    /// The current user manager's identity (see [`RestoreEpoch`]), when one
    /// can be read. Defaults to unknown, which keys the epoch on the boot
    /// alone, exactly as builds before #1405 did.
    fn user_manager_id(&self) -> Option<String> {
        None
    }
    /// When the current user manager started, in Unix seconds, when that
    /// can be read. It is the proof a pre-epoch report was (or was not)
    /// written under this manager (see [`RestoreEpoch::legacy_match`]).
    /// Defaults to unknown.
    fn user_manager_started(&self) -> Option<f64> {
        None
    }
    /// The user manager's `/proc` start ticks (field 22), when a
    /// manager process can be found. This is the single epoch-key
    /// derivation (S3): never systemctl, never btime. The default
    /// scans `/proc` for this uid's `systemd --user` process; test
    /// hosts override it to pin an epoch without a real manager.
    fn manager_start_ticks(&self) -> Option<String> {
        let uid = unsafe { libc::getuid() };
        manager_pid_start(uid)
            .and_then(|id| id.rsplit(':').next().map(str::to_owned))
            .filter(|ticks| !ticks.is_empty())
    }
    /// Whether this process runs in the initial (host) PID namespace.
    /// Scope-creating recovery work requires it; test hosts run there by
    /// default.
    fn in_initial_pid_namespace(&self) -> bool {
        true
    }
    /// The current restore epoch: this boot under its current user manager.
    /// The team driver records it with [`RestoreEpoch::as_json`] and compares
    /// by equality only.
    fn epoch(&self) -> Result<RestoreEpoch> {
        Ok(RestoreEpoch {
            boot_id: self.boot_id()?,
            user_manager: self.user_manager_id(),
        })
    }
    /// The kernel boot time in Unix seconds (`btime` in `/proc/stat`).
    fn boot_time(&self) -> Option<u64>;
    /// Seconds since the Unix epoch.
    fn now(&self) -> f64;
    /// Pause; ended early once termination is requested (`terminated`).
    fn sleep(&self, duration: Duration);
    /// Whether this process was asked to stop (SIGTERM or SIGINT): the watch starts no other pass
    /// (#909 watch-sigterm-stop). Never, unless the host says so.
    fn terminated(&self) -> bool {
        false
    }
    /// Run a program with a deadline, never through a shell.
    fn run(&self, argv: &[String], cwd: Option<&Path>, timeout: Duration) -> CommandOutcome;
    /// Run a program with `stdin` piped to it (escalation commands read the
    /// incident that way; plain [`Host::run`] gives a null stdin). The default
    /// fails closed, so a fake must opt in before its escalation cases mean
    /// anything.
    fn run_stdin(
        &self,
        _argv: &[String],
        _cwd: Option<&Path>,
        _timeout: Duration,
        _stdin: &str,
    ) -> CommandOutcome {
        CommandOutcome {
            stderr: "this host cannot run a command with stdin".to_owned(),
            ..CommandOutcome::default()
        }
    }
    /// Arm a one-shot timer that runs `request.argv` at its deadline, so a
    /// driver that exits with a pending deadline is woken at it. Arming the
    /// same team and deadline twice is still `Armed`. Default: unsupported.
    fn arm_wake(&self, _request: &WakeRequest) -> WakeOutcome {
        WakeOutcome::Unsupported
    }
}

/// The `systemctl` program every unit operation launches through:
/// `AGENTCTL_SYSTEMCTL` when set (tests point it at a recording
/// fake so no drill ever reaches a real user manager), else the
/// plain name resolved on PATH.
pub fn systemctl_program() -> String {
    resolve_program(
        std::env::var("AGENTCTL_SYSTEMCTL").ok().as_deref(),
        std::env::var_os("AGENTCTL_TEST_HARNESS").is_some(),
        "systemctl",
    )
}

/// The `systemd-run` program wake timers are armed through:
/// `AGENTCTL_SYSTEMD_RUN` when set, else `systemd-run`.
pub fn systemd_run_program() -> String {
    resolve_program(
        std::env::var("AGENTCTL_SYSTEMD_RUN").ok().as_deref(),
        std::env::var_os("AGENTCTL_TEST_HARNESS").is_some(),
        "systemd-run",
    )
}

/// Resolve one launcher program: the explicit seam value wins; under
/// a test harness (`AGENTCTL_TEST_HARNESS` set) an unset seam refuses
/// by resolving to a path that cannot run, so no test can ever reach
/// a real user manager by forgetting the seam.
pub fn resolve_program(seam: Option<&str>, test_harness: bool, default: &str) -> String {
    match seam {
        Some(value) if !value.is_empty() => value.to_owned(),
        _ if test_harness => "/nonexistent/agentctl-test-harness-refusal".to_owned(),
        _ => default.to_owned(),
    }
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

    fn user_manager_id(&self) -> Option<String> {
        // The manager's own unit in the system instance carries a fresh
        // invocation id at every manager start.
        let uid = unsafe { libc::getuid() };
        let outcome = self.run(
            &strings(&[
                &systemctl_program(),
                "show",
                &format!("user@{uid}.service"),
                "--property=InvocationID",
                "--value",
            ]),
            None,
            Duration::from_secs(10),
        );
        if outcome.status == Some(0) {
            let value = outcome.stdout.trim();
            if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Some(format!("invocation:{value}"));
            }
        }
        manager_pid_start(uid)
    }

    fn user_manager_started(&self) -> Option<f64> {
        // Monotonic microseconds since boot, placed on the wall clock
        // with the kernel boot time: no timestamp parsing involved.
        let uid = unsafe { libc::getuid() };
        let outcome = self.run(
            &strings(&[
                &systemctl_program(),
                "show",
                &format!("user@{uid}.service"),
                "--property=ActiveEnterTimestampMonotonic",
                "--value",
            ]),
            None,
            Duration::from_secs(10),
        );
        if outcome.status != Some(0) {
            return None;
        }
        let micros: u64 = outcome.stdout.trim().parse().ok()?;
        if micros == 0 {
            return None;
        }
        Some(self.boot_time()? as f64 + micros as f64 / 1_000_000.0)
    }

    fn in_initial_pid_namespace(&self) -> bool {
        fs::read_to_string("/proc/self/status")
            .is_ok_and(|status| initial_pid_namespace_status(&status))
    }

    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
    }

    fn sleep(&self, duration: Duration) {
        // A plain sleep resumes after the signal that sets the termination flag, so the watch
        // slept out its interval through SIGTERM (#909 watch-sigterm-stop).
        crate::agent::sleep_unless_terminated(duration);
    }

    fn terminated(&self) -> bool {
        crate::agent::termination_requested()
    }

    fn run(&self, argv: &[String], cwd: Option<&Path>, timeout: Duration) -> CommandOutcome {
        run_captured(argv, cwd, timeout, None)
    }

    fn run_stdin(
        &self,
        argv: &[String],
        cwd: Option<&Path>,
        timeout: Duration,
        stdin: &str,
    ) -> CommandOutcome {
        run_captured(argv, cwd, timeout, Some(stdin))
    }

    fn arm_wake(&self, request: &WakeRequest) -> WakeOutcome {
        if request.team_key.is_empty()
            || !request
                .team_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return WakeOutcome::Failed {
                detail: "team key is not a plain unit suffix".to_owned(),
            };
        }
        // No answering bus means no timer can be armed; the caller waits
        // resident, which is always safe, instead of exiting unwoken.
        let probe = self.run(
            &strings(&[&systemctl_program(), "--user", "show-environment"]),
            None,
            Duration::from_secs(15),
        );
        if probe.status != Some(0) {
            return WakeOutcome::NoManager;
        }
        let stem = wake_unit_stem(&request.team_key);
        let timer = format!("{stem}.timer");
        let service = format!("{stem}.service");
        // One wake per team: retiring any previous deadline first is what
        // makes re-arming idempotent instead of cumulative.
        let _ = self.run(
            &strings(&[&systemctl_program(), "--user", "stop", &timer]),
            None,
            Duration::from_secs(30),
        );
        let _ = self.run(
            &strings(&[
                &systemctl_program(),
                "--user",
                "reset-failed",
                &timer,
                &service,
            ]),
            None,
            Duration::from_secs(30),
        );
        let mut argv = wake_argv(request, self.now());
        argv[0] = systemd_run_program();
        let outcome = self.run(&argv, None, Duration::from_secs(30));
        match outcome.status {
            Some(0) => WakeOutcome::Armed { unit: timer },
            _ => WakeOutcome::Failed {
                detail: first_error_lines(&outcome.stderr),
            },
        }
    }
}

/// The transient wake unit stem for one team key; the timer is `{stem}.timer`.
pub(crate) fn wake_unit_stem(team_key: &str) -> String {
    format!("agentctl-team-wake@{team_key}")
}

/// The `systemd-run` argv that arms `request`, pure so tests pin it exactly.
/// The delay is whole seconds from `now`, at least one: a past deadline
/// still wakes promptly instead of never.
pub(crate) fn wake_argv(request: &WakeRequest, now: f64) -> Vec<String> {
    let delay = ((request.deadline_unix - now).ceil() as i64).max(1);
    let mut argv = vec![
        "systemd-run".to_owned(),
        "--user".to_owned(),
        format!("--on-active={delay}s"),
        "--unit".to_owned(),
        wake_unit_stem(&request.team_key),
    ];
    argv.extend(request.argv.iter().cloned());
    argv
}

fn run_captured(
    argv: &[String],
    cwd: Option<&Path>,
    timeout: Duration,
    stdin: Option<&str>,
) -> CommandOutcome {
    let Some((program, arguments)) = argv.split_first() else {
        return CommandOutcome {
            stderr: "empty command".to_owned(),
            ..CommandOutcome::default()
        };
    };
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
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
    // Feed stdin from its own thread for the same reason both outputs
    // are drained below: a child that fills a pipe before reading must
    // never deadlock the writer. A broken pipe just ends the feed.
    let stdin_writer = match (stdin, child.stdin.take()) {
        (Some(text), Some(mut pipe)) => {
            let owned = text.to_owned();
            Some(std::thread::spawn(move || {
                let _ = pipe.write_all(owned.as_bytes());
            }))
        }
        _ => None,
    };
    // Drain both pipes while the child runs. Reading only after exit
    // deadlocks a child that fills a pipe buffer, and keeping only a
    // tail of the capture corrupts the documents callers parse from
    // stdout (#1322 slot-bind-status-parse).
    let stdout_reader = child
        .stdout
        .take()
        .map(|mut pipe| std::thread::spawn(move || drain_capped(&mut pipe)));
    let stderr_reader = child
        .stderr
        .take()
        .map(|mut pipe| std::thread::spawn(move || drain_capped(&mut pipe)));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdin_writer.map(|writer| writer.join());
                let _ = stdout_reader.map(|reader| reader.join());
                let _ = stderr_reader.map(|reader| reader.join());
                return CommandOutcome {
                    stderr: format!("{program} did not finish within {}s", timeout.as_secs()),
                    ..CommandOutcome::default()
                };
            }
        }
    };
    let _ = stdin_writer.map(|writer| writer.join());
    let stdout = stdout_reader.and_then(|reader| reader.join().ok());
    let stderr = stderr_reader.and_then(|reader| reader.join().ok());
    let (stdout, stdout_overflow) = stdout.unwrap_or_default();
    let (stderr, stderr_overflow) = stderr.unwrap_or_default();
    if stdout_overflow || stderr_overflow {
        let stream = if stdout_overflow { "stdout" } else { "stderr" };
        return CommandOutcome {
                stderr: format!(
                    "{program} printed more than {MAX_CAPTURED_OUTPUT} bytes on {stream}; output not captured"
                ),
                ..CommandOutcome::default()
            };
    }
    CommandOutcome {
        status: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    }
}

/// Whether `/proc/self/status` places this process in the initial PID
/// namespace: `NSpid` lists the process ID at every namespace level, so
/// exactly one entry means the host namespace and more than one means a
/// descendant team namespace.
fn initial_pid_namespace_status(status: &str) -> bool {
    status.lines().find_map(|line| {
        line.strip_prefix("NSpid:")
            .map(|pids| pids.split_whitespace().count() == 1)
    }) == Some(true)
}

/// Fallback user-manager identity (#1405): the `systemd --user` process
/// owned by `uid`, as `pid:<pid>:<start-ticks>` from `/proc/<pid>/stat`, so
/// a restarted manager reads differently even if its PID is reused.
fn manager_pid_start(uid: u32) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u64>() else {
            continue;
        };
        let path = entry.path();
        if path.metadata().map(|meta| meta.uid()).ok() != Some(uid) {
            continue;
        }
        let Ok(cmdline) = fs::read(path.join("cmdline")) else {
            continue;
        };
        let mut argv = cmdline.split(|byte| *byte == 0);
        let program = argv.next().unwrap_or_default();
        if !program.ends_with(b"systemd") || argv.next() != Some(b"--user".as_slice()) {
            continue;
        }
        // Field 22 of the stat line; the comm in parentheses may itself
        // hold spaces or parentheses, so count from the last ')': the
        // remainder starts at field 3, making start time index 19.
        let Ok(stat) = fs::read_to_string(path.join("stat")) else {
            continue;
        };
        let Some(start) = stat
            .rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().nth(19).map(str::to_owned))
        else {
            continue;
        };
        return Some(format!("pid:{pid}:{start}"));
    }
    None
}

/// Most bytes one captured stream may hold. Past it the run fails loudly
/// (see `SystemHost::run`) instead of silently keeping a tail that callers
/// then misparse (#1322). Mirrors the Herdr control capture's stdout cap.
const MAX_CAPTURED_OUTPUT: usize = 8 * 1024 * 1024;

/// Read `pipe` to end, retaining at most [`MAX_CAPTURED_OUTPUT`] bytes and
/// reporting whether the stream ran past that. Draining never stops at the
/// cap, so the child never blocks on a full pipe.
fn drain_capped(pipe: &mut impl Read) -> (Vec<u8>, bool) {
    let mut kept = Vec::new();
    let mut overflow = false;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) => return (kept, overflow),
            Ok(count) => {
                let room = MAX_CAPTURED_OUTPUT.saturating_sub(kept.len());
                kept.extend_from_slice(&buffer[..count.min(room)]);
                overflow |= count > room;
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return (kept, overflow),
        }
    }
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
    /// The Herdr binary whose `--version` the version pin checks.
    pub herdr_bin: PathBuf,
}

/// The last day on which readers accept reports at the pre-#1359 locations
/// (`REGISTRY/restore-reports/`, `REGISTRY/agents/restore-reports/` and the
/// shared XDG state directory below).
pub const LEGACY_REPORT_SUPPORT_ENDS: &str = "2027-02-12";

/// Where new reports live by default (#1359 restore-report-under-agents):
/// `REGISTRY/restore/reports/`, the reports' own collection directory,
/// outside `agents/` (the agent collection, whose every entry `list` reads
/// as an agent) and off the registry's top level.
pub fn reports_directory(registry: &Path) -> PathBuf {
    registry.join("restore").join("reports")
}

/// The report directories older builds wrote to, in read order after the
/// current one: the #975 registry subdirectory, the directory found inside
/// `agents/` in the wild (#1359), then the shared XDG state directory.
/// Readers accept these until [`LEGACY_REPORT_SUPPORT_ENDS`]; an explicit
/// `report_dir` names the one location, so there are no fallback reads.
fn legacy_report_directories(registry: &Path, config: &RestoreConfig) -> Vec<PathBuf> {
    if config.report_dir.is_some() {
        return Vec::new();
    }
    let mut directories = vec![
        registry.join("restore-reports"),
        registry.join("agents").join("restore-reports"),
    ];
    if let Some(directory) = default_report_dir() {
        directories.push(directory);
    }
    directories
}

/// The XDG state report directory: `$XDG_STATE_HOME/agentctl/restore`, else under
/// the home. New reports default to the registry's own `restore/reports/` instead
/// (#975 boot-restore-retry: one host state directory shared by every registry made
/// two teams' same-boot reports overwrite each other, and the boot evidence belonged
/// with the registry it describes); this location is still read so a report written
/// by an older build in the same boot counts, and the watch/health state paths are
/// derived from its parent.
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

/// The per-team driver key: first 16 hex of the canonical registry path's
/// SHA-256. It names state directories and wake units without naming any
/// team, host, or person.
pub fn team_key(registry: &Path) -> String {
    let canonical = fs::canonicalize(registry).unwrap_or_else(|_| registry.to_path_buf());
    let digest = format!(
        "{:x}",
        Sha256::digest(canonical.to_string_lossy().as_bytes())
    );
    digest[..16].to_owned()
}

/// One team's driver state directory, `<state>/agentctl/team-drivers/<key>`,
/// a collection directory of its own. `state_dir` overrides the XDG state
/// home (fixtures and the driver's `--state-dir`); `None` derives it the
/// same way as the report directory.
pub fn team_driver_dir(registry: &Path, state_dir: Option<&Path>) -> Option<PathBuf> {
    let base = match state_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_report_dir()?.parent()?.to_path_buf(),
    };
    Some(base.join("team-drivers").join(team_key(registry)))
}

fn report_path(registry: &Path, config: &RestoreConfig, boot_id: &str) -> PathBuf {
    let directory = config
        .report_dir
        .clone()
        .unwrap_or_else(|| reports_directory(registry));
    directory.join(format!("{boot_id}.json"))
}

/// Every path a report for this boot may live at, in read order: the
/// configured or current default location first, then each earlier one
/// (#1359). `restore-teams` reads the same order.
pub fn candidate_report_paths(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
) -> Vec<PathBuf> {
    let file = format!("{boot_id}.json");
    let mut paths = vec![report_path(registry, config, boot_id)];
    for directory in legacy_report_directories(registry, config) {
        let path = directory.join(&file);
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// This boot's existing report and where it was found: the current location
/// first, then the earlier ones, so a same-boot report from an older build
/// still satisfies the already-restored and watch checks.
fn read_boot_report(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
) -> Option<(PathBuf, Value)> {
    candidate_report_paths(registry, config, boot_id)
        .into_iter()
        .find_map(|path| read_report(&path).map(|report| (path, report)))
}

/// The completed report for `epoch`, scanning every candidate location in
/// read order: the first report that is complete and belongs to this epoch
/// wins. Restore's replay and the watch restore check share this pick, so
/// an incomplete or earlier-epoch report at the current location can never
/// shadow a completed one at an earlier location (a partial report from a
/// failed attempt under a new build, the completed one still under
/// `agents/restore-reports/` from the build that actually restored).
pub fn completed_boot_report(
    registry: &Path,
    config: &RestoreConfig,
    epoch: &RestoreEpoch,
    manager_started: Option<f64>,
) -> Option<(PathBuf, Value)> {
    match boot_report_state(registry, config, epoch, manager_started) {
        BootReportState::Completed(found) => Some(found),
        BootReportState::Incomplete(_)
        | BootReportState::PredatesKeying(_)
        | BootReportState::Missing => None,
    }
}

/// What this boot's recorded restore evidence proves, for readers that
/// must tell an unprovable earlier-format report apart from no restore at all
/// (the watch check: the former is reported distinctly, never alarmed).
#[derive(Clone, Debug, PartialEq)]
pub enum BootReportState {
    /// A completed report proven to belong to this epoch, manager-start proof
    /// by manager start included.
    Completed((PathBuf, Value)),
    /// A report for this boot exists but did not complete (#2018): it is
    /// evidence a restore ran, never the same finding as no report at all.
    Incomplete((PathBuf, Value)),
    /// Only a pre-epoch completed report, which nothing can prove.
    PredatesKeying(PathBuf),
    /// No report file for this boot exists at any candidate location.
    Missing,
}

/// Why an incomplete report did not complete, in the report's own terms
/// (#2018): its recorded error, else its coordinator refusal, else its
/// refused lanes, else the bare fact.
fn incomplete_reason(report: &Value) -> String {
    if let Some(error) = report["error"].as_str().filter(|error| !error.is_empty()) {
        return error.to_owned();
    }
    let coordinator = &report["coordinator"];
    if coordinator["action"] == "refused" {
        if let Some(reason) = coordinator["reason"].as_str().filter(|r| !r.is_empty()) {
            return format!("coordinator refused: {reason}");
        }
        return "coordinator refused".to_owned();
    }
    if let Some(lanes) = report["refused_lanes"]
        .as_array()
        .filter(|lanes| !lanes.is_empty())
    {
        let names: Vec<&str> = lanes.iter().filter_map(Value::as_str).collect();
        if !names.is_empty() {
            return format!("refused lanes: {}", names.join(", "));
        }
    }
    "restore did not complete".to_owned()
}

/// [`BootReportState`] for this boot, scanning candidates in read order.
pub fn boot_report_state(
    registry: &Path,
    config: &RestoreConfig,
    epoch: &RestoreEpoch,
    manager_started: Option<f64>,
) -> BootReportState {
    let mut predating: Option<PathBuf> = None;
    let mut incomplete: Option<(PathBuf, Value)> = None;
    let mut incomplete_predating: Option<(PathBuf, Value)> = None;
    for path in candidate_report_paths(registry, config, &epoch.boot_id) {
        let Some(report) = read_report(&path) else {
            continue;
        };
        if report["complete"] != true {
            // #2018: an existing incomplete report is not "no report": keep
            // the first one proven for this epoch, and an unprovable one
            // only when nothing better is found.
            match epoch.legacy_match(&report, manager_started) {
                LegacyMatch::Matches => {
                    incomplete.get_or_insert((path, report));
                }
                LegacyMatch::PredatesKeying => {
                    incomplete_predating.get_or_insert((path, report));
                }
                LegacyMatch::Miss => {}
            };
            continue;
        }
        match epoch.legacy_match(&report, manager_started) {
            LegacyMatch::Matches => return BootReportState::Completed((path, report)),
            LegacyMatch::PredatesKeying => predating.get_or_insert(path),
            LegacyMatch::Miss => continue,
        };
    }
    if let Some(found) = incomplete {
        return BootReportState::Incomplete(found);
    }
    match predating {
        Some(path) => BootReportState::PredatesKeying(path),
        None => match incomplete_predating {
            Some(found) => BootReportState::Incomplete(found),
            None => BootReportState::Missing,
        },
    }
}

/// Move every report at an earlier registry location into the current
/// report directory (#1359): `agentctl restore --migrate-reports`, dry-run
/// by default. A report already at the destination is never overwritten;
/// the old copy is left and named under `kept`. Running it again moves
/// nothing more. The shared XDG directory is never a source: it held
/// every registry's reports, so none of them can be attributed to this
/// registry; it stays a read fallback until the deprecation date.
pub fn migrate_reports(registry: &Path, config: &RestoreConfig, apply: bool) -> Result<Value> {
    let destination = config
        .report_dir
        .clone()
        .unwrap_or_else(|| reports_directory(registry));
    let mut sources = vec![
        registry.join("restore-reports"),
        registry.join("agents").join("restore-reports"),
    ];
    sources.retain(|directory| *directory != destination);
    let (moved_key, mut moved) = (if apply { "moved" } else { "would_move" }, Vec::new());
    let mut kept = Vec::new();
    if apply {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&destination)
            .map_err(|error| fail(format!("cannot create {}: {error}", destination.display())))?;
    }
    for source in &sources {
        let Ok(entries) = fs::read_dir(source) else {
            continue;
        };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        names.sort();
        for path in names {
            let Some(name) = path.file_name() else {
                continue;
            };
            let target = destination.join(name);
            if target.exists() {
                kept.push(json!({
                    "path": path.display().to_string(),
                    "reason": format!("a report already exists at {}", target.display()),
                }));
                continue;
            }
            if apply {
                match fs::rename(&path, &target) {
                    Ok(()) => moved.push(json!(path.display().to_string())),
                    Err(error) => kept.push(json!({
                        "path": path.display().to_string(),
                        "reason": format!("cannot move it to {}: {error}", target.display()),
                    })),
                }
            } else {
                moved.push(json!(path.display().to_string()));
            }
        }
    }
    let mut report = json!({
        "registry": registry.display().to_string(),
        "apply": apply,
        "destination": destination.display().to_string(),
        "kept": kept,
        "legacy_support_ends": LEGACY_REPORT_SUPPORT_ENDS,
    });
    report[moved_key] = json!(moved);
    Ok(report)
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

/// Best-effort progress for a restore still running (#1934): the same
/// report shape as the final one, never complete, with the lanes finished
/// so far under `agents`. A unit timeout kills the run, not the evidence:
/// the restarted run resumes from these rows instead of starting over.
fn write_progress_report(path: &Path, report: &Value, at: f64) {
    let mut partial = report.clone();
    partial["complete"] = json!(false);
    partial["in_progress"] = json!(true);
    partial["finished_at"] = json!(at);
    let _ = write_report(path, &partial);
}

/// Best-effort evidence for a restore that failed before its final report
/// (#975: at boot the journal may be unreadable, so this file is the only
/// record of why nothing came back). Never marks the boot complete, so the
/// already-restored check and the unit's bounded retry still apply.
fn write_partial_report(path: &Path, report: &Value, finished_at: f64, error: &str) {
    let mut partial = report.clone();
    partial["complete"] = json!(false);
    partial["error"] = json!(error);
    partial["finished_at"] = json!(finished_at);
    let _ = write_report(path, &partial);
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
    // Both wordings count (#1130 D3): a per-slot image and the shared worktrees image.
    let warnings = outcome
        .stderr
        .lines()
        .filter(|line| {
            line.contains("slot image not mounted") || line.contains("worktrees image not mounted")
        })
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
        "slots": if outcome.status == Some(0) { image_slot_entries(&outcome.stdout) } else { Vec::new() },
        "mounted": mounted,
        "warnings": warnings,
        "detail": if outcome.status == Some(0) { Value::Null } else { json!(outcome.stderr.trim()) },
    })
}

/// Canonical form of a project path for comparison and ordering; an
/// unresolvable path compares as written.
fn canonical_project(project: &Path) -> PathBuf {
    fs::canonicalize(project).unwrap_or_else(|_| project.to_path_buf())
}

/// Every wrkslots project a restore must remount (#2089): the restore
/// project first, then the distinct canonicalised projects recorded by
/// the desired slotted agents (`slot_agents`, self-referential records
/// already healed), sorted for determinism. One project's failure never
/// blocks the others because each gets its own `remount_slot_images` run.
fn slot_image_projects(boxed: &[SlotAgent], restore_project: &Path) -> Vec<PathBuf> {
    let restore_canonical = canonical_project(restore_project);
    let mut rest: Vec<PathBuf> = boxed
        .iter()
        .map(|agent| canonical_project(&agent.project))
        .filter(|project| *project != restore_canonical)
        .collect();
    rest.sort();
    rest.dedup();
    let mut projects = vec![restore_project.to_path_buf()];
    projects.extend(rest);
    projects
}

/// The `slot_images_by_project` row for one project: the remount row
/// fields plus its `project` (the remount row already carries it).
fn project_image_row<'a>(by_project: &'a [Value], project: &Path) -> Option<&'a Value> {
    let wanted = canonical_project(project);
    by_project.iter().find(|row| {
        row["project"]
            .as_str()
            .is_some_and(|recorded| canonical_project(Path::new(recorded)) == wanted)
    })
}

/// Outer budget for one slot in the single Eden verb run (#1977): the
/// verb itself caps each `eden info` at 15s and each `eden start` at
/// 60s, and one slot can cost info + start + verify + re-query.
const EDEN_PER_SLOT_TIMEOUT_SECONDS: u64 = 105;

/// The slot directory of `slot` as restore can derive it without a
/// wrkslots call (#1977): the nearest ancestor of an agent's recorded
/// working directory named for the slot, or an image-status location
/// named for the slot (the same location `verify_slot_bind` checks),
/// provided it canonicalises inside the restore project. An Eden
/// mount outside the project is never mistaken for its slot
/// directory. `None` means restore cannot tell where the slot lives.
fn eden_slot_dir(
    slot: &str,
    agents: &[&SlotAgent],
    images: &Value,
    restore_canonical: &Path,
) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for agent in agents {
        if let Some(cwd) = agent.cwd.as_deref() {
            for ancestor in cwd.ancestors() {
                if ancestor.file_name().and_then(|name| name.to_str()) == Some(slot) {
                    candidates.push(ancestor.to_path_buf());
                    break;
                }
            }
        }
    }
    if let Some(entries) = images["slots"].as_array() {
        for entry in entries {
            if let Some(location) = entry["location"].as_str() {
                let location = Path::new(location);
                if location.file_name().and_then(|name| name.to_str()) == Some(slot) {
                    candidates.push(location.to_path_buf());
                }
            }
        }
    }
    candidates.into_iter().find(|directory| {
        fs::canonicalize(directory).is_ok_and(|canonical| canonical.starts_with(restore_canonical))
    })
}

/// Restart the per-slot Eden daemons of this registry's own slotted agents
/// (#1977, #2089): one `wrkslots eden restart-slot-daemons` run per
/// covered project, naming every Eden slot of that project at once, with
/// that project as the verb's root and working directory. The covered
/// projects are the per-project image rows (`slot_images_by_project`:
/// the restore project plus every distinct project recorded by a
/// slotted agent). Launching wrkslots loads every active record, so a
/// run per slot — git slots included — was pure cost before the slot
/// holds on every boot. Slots restore can prove non-Eden itself never
/// reach the verb: when the slot directory is derivable (see
/// `eden_slot_dir`, canonicalised against the slot's own project) and
/// `<slot>/.eden-state` is absent, exactly as the verb classifies it,
/// the slot's `skipped` row is synthesised here, so a git-only project
/// produces no invocation at all. A slot whose directory cannot be
/// derived is passed to the verb, which classifies it. Because this verb
/// starts processes, a slot recorded under no covered project gets a
/// `refused` row without any invocation, and a slot whose project's
/// image row did not run (its status call failed) is refused without an
/// invocation for that project, while other projects restart normally.
/// The wrkslots binary is the one already resolved for `image status`
/// (`RestoreOptions::wrkslots`). The outer timeout scales with the
/// slots in each run (`EDEN_PER_SLOT_TIMEOUT_SECONDS` each; the verb
/// caps every call inside). Every other failure — no binary configured,
/// a failed or unparsable verb run, an Eden slot the verb did not
/// report — degrades to a per-slot `refused` row here and never fails
/// restore. The verb's own rows pass through in full (every checkout's
/// row included) with each slot's agents named; only a slot passed to
/// the verb with no row at all earns a synthesised refusal, so a
/// `skipped` git slot is never counted as an Eden refusal.
fn restart_eden_daemons(
    host: &dyn Host,
    wrkslots: Option<&Path>,
    boxed: &[SlotAgent],
    images_by_project: &[Value],
) -> Vec<Value> {
    if boxed.is_empty() {
        return Vec::new();
    }
    // Covered projects in report order, each with its image row.
    let covered: Vec<(PathBuf, &Value)> = images_by_project
        .iter()
        .filter_map(|row| {
            row["project"]
                .as_str()
                .map(|project| (PathBuf::from(project), row))
        })
        .collect();
    let cover_for = |project: &Path| -> Option<&(PathBuf, &Value)> {
        let wanted = canonical_project(project);
        covered
            .iter()
            .find(|(recorded, _)| canonical_project(recorded) == wanted)
    };
    let mut slot_agents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut slot_records: BTreeMap<String, Vec<&SlotAgent>> = BTreeMap::new();
    let mut slot_project: BTreeMap<String, PathBuf> = BTreeMap::new();
    for agent in boxed {
        slot_agents
            .entry(agent.slot.clone())
            .or_default()
            .push(agent.name.clone());
        slot_records
            .entry(agent.slot.clone())
            .or_default()
            .push(agent);
        slot_project
            .entry(agent.slot.clone())
            .or_insert_with(|| agent.project.clone());
    }
    // The project each slot is settled or invoked under (its covered
    // project's recorded path, or its own recorded project when refused
    // as foreign), for the rows assembled below.
    let mut slot_row_project: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut settled: BTreeMap<String, Value> = BTreeMap::new();
    let mut verb_slots: Vec<(PathBuf, Vec<String>)> = covered
        .iter()
        .map(|(project, _)| (project.clone(), Vec::new()))
        .collect();
    for (slot, agents) in &slot_agents {
        let recorded_project = &slot_project[slot];
        let Some((row_project, images)) = cover_for(recorded_project) else {
            slot_row_project.insert(slot.clone(), recorded_project.clone());
            settled.insert(
                slot.clone(),
                json!({
                    "slot": slot,
                    "agents": agents,
                    "project": recorded_project,
                    "status": "refused",
                    "reason": format!(
                        "slot project {} is not among the projects this restore covers; the slot was not passed to wrkslots",
                        recorded_project.display()
                    ),
                }),
            );
            continue;
        };
        let row_project = (*row_project).clone();
        slot_row_project.insert(slot.clone(), row_project.clone());
        let project_canonical = canonical_project(&row_project);
        let has_state = eden_slot_dir(slot, &slot_records[slot], images, &project_canonical)
            .map(|directory| directory.join(".eden-state").symlink_metadata().is_ok());
        if has_state == Some(false) {
            settled.insert(
                slot.clone(),
                json!({
                    "slot": slot,
                    "checkout": null,
                    "mount": null,
                    "config_dir": null,
                    "agents": agents,
                    "project": row_project,
                    "status": "skipped",
                    "reason": "slot has no Eden checkout; no Eden daemon to restart",
                }),
            );
            continue;
        }
        if wrkslots.is_none() {
            settled.insert(
                slot.clone(),
                json!({
                    "slot": slot,
                    "agents": agents,
                    "project": row_project,
                    "status": "refused",
                    "reason": "no wrkslots configured or on PATH; the slot's Eden daemon was not restarted",
                }),
            );
            continue;
        }
        if images["action"].as_str() != Some("ran") {
            settled.insert(
                slot.clone(),
                json!({
                    "slot": slot,
                    "agents": agents,
                    "project": row_project,
                    "status": "refused",
                    "reason": format!(
                        "slot images for project {} did not run successfully (action {}); the slot's Eden daemon was not restarted",
                        row_project.display(),
                        images["action"].as_str().unwrap_or("unknown")
                    ),
                }),
            );
            continue;
        }
        if let Some((_, slots)) = verb_slots
            .iter_mut()
            .find(|(project, _)| *project == row_project)
        {
            slots.push(slot.clone());
        }
    }
    let mut verb_rows: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut verb_outcomes: BTreeMap<String, (Option<i32>, String)> = BTreeMap::new();
    for (project, slots) in &verb_slots {
        if slots.is_empty() {
            continue;
        }
        let wrkslots = wrkslots.expect("verb slots imply a wrkslots binary");
        eprintln!(
            "agentctl restore: restarting Eden daemons for slots {}",
            slots.join(", ")
        );
        let mut argv = wrkslots_argv(wrkslots, project, &["eden", "restart-slot-daemons"]);
        for slot in slots {
            argv.push("--slot".to_owned());
            argv.push(slot.clone());
        }
        argv.push("--format".to_owned());
        argv.push("json".to_owned());
        let outcome = host.run(
            &argv,
            Some(project),
            Duration::from_secs(EDEN_PER_SLOT_TIMEOUT_SECONDS * slots.len() as u64),
        );
        let reported: Vec<Value> = serde_json::from_str::<Value>(&outcome.stdout)
            .ok()
            .and_then(|document| document["eden_daemons"].as_array().cloned())
            .unwrap_or_default();
        for mut row in reported {
            let row_slot = row["slot"].as_str().unwrap_or_default().to_owned();
            if let Some(agents) = slot_agents.get(&row_slot) {
                row["agents"] = json!(agents);
            }
            row["project"] = json!(project);
            verb_rows.entry(row_slot).or_default().push(row);
        }
        for slot in slots {
            verb_outcomes.insert(slot.clone(), (outcome.status, outcome.stderr.clone()));
        }
    }
    let mut rows: Vec<Value> = Vec::new();
    for (slot, agents) in &slot_agents {
        if let Some(row) = settled.remove(slot) {
            rows.push(row);
            continue;
        }
        let row_project = slot_row_project
            .get(slot)
            .cloned()
            .unwrap_or_else(|| slot_project[slot].clone());
        match verb_rows.remove(slot) {
            Some(mut mine) if !mine.is_empty() => rows.append(&mut mine),
            _ => {
                let detail = match verb_outcomes.get(slot) {
                    Some((Some(status), stderr)) => {
                        format!("exit {status}: {}", first_error_lines(stderr))
                    }
                    Some((None, stderr)) => {
                        format!("it did not finish: {}", first_error_lines(stderr))
                    }
                    None => "the verb was not run".to_owned(),
                };
                rows.push(json!({
                    "slot": slot,
                    "agents": agents,
                    "project": row_project,
                    "status": "refused",
                    "reason": format!(
                        "wrkslots eden restart-slot-daemons did not report slot {slot}: {detail}"
                    ),
                }));
            }
        }
    }
    for (_, mut extra) in verb_rows {
        rows.append(&mut extra);
    }
    rows
}

/// Each image-backed slot `wrkslots image status` listed, with its mounted location
/// and phase, for the pre-revive bind check (#1130 D3).
fn image_slot_entries(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.starts_with(char::is_whitespace))
        .filter_map(|line| {
            let (slot, rest) = line.split_once(" at ")?;
            if !slot.contains('/') || !line.trim_end().ends_with(')') {
                return None;
            }
            let (location, phase) = rest.rsplit_once(" (")?;
            Some(json!({
                "slot": slot,
                "location": location,
                "phase": phase.trim_end_matches(')'),
            }))
        })
        .collect()
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
                        .or_else(|| line.split_once("worktrees image not mounted:"))
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

/// Wait, bounded, for the configured user certificate to be usable (#1137):
/// at boot it is often renewed moments after the restore unit starts, and
/// launching before it exists burns every lane on a certificate it cannot
/// use. Returns whether agents may start and the report row. Unconfigured
/// means no gate, as before; a dry run samples once and never waits.
fn wait_for_certificate(host: &dyn Host, config: &RestoreConfig, dry_run: bool) -> (bool, Value) {
    let Some(path) = config.certificate_file.as_ref() else {
        return (true, json!({"present": null, "detail": "not configured"}));
    };
    let wait = if dry_run {
        0
    } else {
        config
            .certificate_wait_seconds
            .unwrap_or(config.herdr_wait_seconds)
    };
    let started = host.now();
    loop {
        let now_millis = (host.now() * 1000.0) as i64;
        match crate::credentials::certificate_state(path, now_millis) {
            crate::credentials::CertificateState::Valid => {
                return (
                    true,
                    json!({
                        "present": true,
                        "valid": true,
                        "path": path,
                        "waited_seconds": (host.now() - started).round(),
                    }),
                );
            }
            state => {
                if host.now() - started >= wait as f64 {
                    let (present, detail) = match state {
                        crate::credentials::CertificateState::Missing => (false, "missing"),
                        crate::credentials::CertificateState::Unusable(detail) => (true, detail),
                        crate::credentials::CertificateState::Valid => unreachable!(),
                    };
                    let remedy = config.certificate_fetch.clone().unwrap_or_else(|| {
                        format!("fetch the user certificate to {}", path.display())
                    });
                    eprintln!(
                        "agentctl restore: user certificate unusable at {} ({detail}); run: {remedy}",
                        path.display()
                    );
                    return (
                        false,
                        json!({
                            "present": present,
                            "valid": false,
                            "path": path,
                            "detail": detail,
                            "remedy": remedy,
                            "waited_seconds": (host.now() - started).round(),
                        }),
                    );
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
                    "profile_refresh": result["profile_refresh"],
                    "queued_delivered": result["queued_delivered"].as_array().map_or(0, Vec::len),
                    "queued_archived": result["queued_archived"].as_array().map_or(0, Vec::len),
                    "queued_uncertain": result["queued_uncertain"].as_array().map_or(0, Vec::len),
                    "queued_pending": result["queued_pending"].as_array().map_or(0, Vec::len),
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
        None,
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
        "queued_delivered": result["queued_delivered"].as_array().map_or(0, Vec::len),
        "queued_archived": result["queued_archived"].as_array().map_or(0, Vec::len),
        "queued_uncertain": result["queued_uncertain"].as_array().map_or(0, Vec::len),
        "queued_pending": result["queued_pending"].as_array().map_or(0, Vec::len),
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
                profile: None,
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
    /// The recorded working directory, checked against the slot's mounted
    /// location before a revive (#1130 D3), never used to resolve the slot.
    cwd: Option<PathBuf>,
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
            // Heal a self-referential slot_project for the hold/transfer
            // calls too (#1140): forcing the slot directory as its own
            // project root reads the slot's config copy and double-nests.
            let recorded = PathBuf::from(record["slot_project"].as_str()?);
            let cwd = Path::new(record["cwd"].as_str()?);
            let project = if fs::canonicalize(&recorded).ok().as_deref() == Some(cwd) {
                crate::subagents::discovered_slot_project(cwd).unwrap_or(recorded)
            } else {
                recorded
            };
            Some(SlotAgent {
                name: name.clone(),
                slot: record["slot"].as_str()?.to_owned(),
                project,
                cwd: record["cwd"].as_str().map(PathBuf::from),
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
pub(crate) fn slot_row(
    host: &dyn Host,
    wrkslots: &Path,
    project: &Path,
    slot: &str,
) -> Result<Value> {
    // Wait on a busy state lock instead of dropping the bind (#786): `--wait-lock 30`
    // plus two retries cover the separate config lock, which reports the same refusal.
    let mut last_error = String::new();
    for attempt in 0..3 {
        if attempt > 0 {
            host.sleep(Duration::from_secs(2));
        }
        let outcome = host.run(
            &wrkslots_argv(
                wrkslots,
                project,
                &[
                    "--wait-lock",
                    "30",
                    "status",
                    "--slot",
                    slot,
                    "--format",
                    "json",
                ],
            ),
            Some(project),
            Duration::from_secs(120),
        );
        if outcome.status == Some(0) {
            return slot_row_document(&outcome.stdout, slot);
        }
        last_error = outcome.stderr.trim().to_owned();
        if !last_error.contains("lock is busy") {
            break;
        }
    }
    Err(fail(format!("wrkslots status failed: {last_error}")))
}

fn slot_row_document(stdout: &str, slot: &str) -> Result<Value> {
    let document: Value = serde_json::from_str(stdout).map_err(|error| {
        // Name the first bytes seen, so a capture or format break is
        // self-explaining instead of a bare position (#1322).
        let preview: String = stdout.chars().take(120).collect();
        fail(format!(
            "wrkslots status printed no JSON: {error}; first bytes: {preview:?}"
        ))
    })?;
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
    // Retry a busy config lock, which `--wait-lock` on the state lock does not cover (#786).
    for attempt in 0..3 {
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
        if moved.status == Some(0) {
            return Ok(row);
        }
        let reason = moved.stderr.trim().to_owned();
        if attempt == 2 || !reason.contains("lock is busy") {
            return Err(fail(format!("transfer-owner refused: {reason}")));
        }
        host.sleep(Duration::from_secs(2));
    }
    unreachable!()
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
        Ok(json!({
            "action": "transferred",
            "slot": agent.slot,
            "owner_pid": pid,
            "unheld": unheld,
        }))
    })();
    match result {
        Ok(value) => value,
        // #1130: a refused transfer is named with the exact remedy an
        // operator (or the lane itself) runs to rebind from outside the box.
        Err(error) => json!({
            "action": "refused",
            "slot": agent.slot,
            "reason": error.to_string(),
            "remedy": format!("agentctl rebind-slot {}", agent.name),
        }),
    }
}

/// Whether row `action` means the lane's harness came back in this restore.
fn lane_revived(row: &Value) -> bool {
    matches!(
        row["action"].as_str(),
        Some("revived" | "revived-fresh" | "promoted")
    )
}

/// Record the slot-owner outcome on a revived lane's row (#1130): transfer
/// through the configured slot manager only, and say so when none is
/// configured instead of staying silent.
fn record_slot_owner<A: ManagedApi + ?Sized>(
    row: &mut Value,
    manager: &ManagedAgents<'_, A>,
    host: &dyn Host,
    wrkslots: Option<&Path>,
    boxed: &[SlotAgent],
) {
    if !lane_revived(row) {
        return;
    }
    let Some(name) = row["name"].as_str() else {
        return;
    };
    let Some(agent) = boxed.iter().find(|agent| agent.name == name) else {
        return;
    };
    row["slot_owner"] = match wrkslots {
        Some(wrkslots) => transfer_slot(manager, host, wrkslots, agent),
        None => json!({
            "action": "skipped",
            "slot": agent.slot,
            "reason": "no wrkslots configured; the slot owner was not transferred",
            "remedy": format!("agentctl rebind-slot {}", agent.name),
        }),
    };
}

/// Verify, before a revive, that an image-backed slot's bind is actually
/// mounted at the location the lane will start in (#1130 D3, #2089).
///
/// The evidence is the `wrkslots image status` run for the agent's own
/// recorded project plus one `findmnt` on the mounted location. A missing
/// project row, or a project row whose status call failed, refuses the
/// lane: its bind was never checked, so it must not be waved through. A
/// slot the project's (successful) image status does not list is not
/// image-backed — a plain worktree — so there is no bind to verify. The
/// slot is resolved from its recorded name and project (see
/// `slot_agents`), never from the stored working directory; the directory
/// is only checked to be the mounted slot itself, not a bare mount point
/// or a stale original.
fn verify_slot_bind(
    host: &dyn Host,
    agent: &SlotAgent,
    images: Option<&Value>,
) -> std::result::Result<Value, String> {
    let refuse = |detail: String| {
        format!(
            "slot bind not verified for {} (slot {}): {detail}",
            agent.name, agent.slot
        )
    };
    let Some(images) = images else {
        return Err(refuse(format!(
            "no wrkslots image status row for the slot's project {}; the slot bind was not verified",
            agent.project.display()
        )));
    };
    let warnings = images["warnings"]
        .as_array()
        .map(|rows| rows.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(line) = warnings
        .iter()
        .find(|line| line.contains("worktrees image not mounted"))
    {
        return Err(refuse(format!(
            "the worktrees image is not mounted, so no slot bind exists: {line}"
        )));
    }
    if let Some(line) = warnings
        .iter()
        .find(|line| line.contains("slot image not mounted") && line.contains(agent.slot.as_str()))
    {
        return Err(refuse(format!("the slot image is not mounted: {line}")));
    }
    if images["action"].as_str() != Some("ran") {
        let detail = images["detail"]
            .as_str()
            .filter(|detail| !detail.is_empty())
            .or_else(|| images["reason"].as_str())
            .unwrap_or("no detail recorded");
        return Err(refuse(format!(
            "wrkslots image status for the slot's project {} did not run successfully (action {}): {detail}",
            agent.project.display(),
            images["action"].as_str().unwrap_or("unknown")
        )));
    }
    let entry = images["slots"].as_array().and_then(|rows| {
        rows.iter().find(|row| {
            let listed = row["slot"].as_str().unwrap_or_default();
            listed == agent.slot
                || listed.rsplit('/').next() == Some(agent.slot.as_str())
                || row["location"]
                    .as_str()
                    .and_then(|location| Path::new(location).file_name())
                    .and_then(|name| name.to_str())
                    == Some(agent.slot.as_str())
        })
    });
    let Some(entry) = entry else {
        return Ok(json!({
            "action": "verified",
            "slot": agent.slot,
            "detail": "the slot is not image-backed in wrkslots image status; no bind to verify",
        }));
    };
    let location = entry["location"].as_str().unwrap_or_default();
    let phase = entry["phase"].as_str().unwrap_or_default();
    // Real `wrkslots image status` reports a mounted, usable image as
    // phase `ready` (provisioning/discarding are not usable); accept it
    // alongside an explicit mounted wording, refuse everything else.
    if phase.contains("not mounted") || !(phase == "ready" || phase.contains("mounted")) {
        return Err(refuse(format!(
            "wrkslots image status reports the slot's bind at {location} as {phase}"
        )));
    }
    if let Some(cwd) = agent.cwd.as_deref() {
        if !cwd.starts_with(Path::new(location)) {
            return Err(refuse(format!(
                "the recorded working directory {} is not the mounted slot at {location}",
                cwd.display()
            )));
        }
    }
    let outcome = host.run(
        &strings(&["findmnt", "-n", "-o", "TARGET,SOURCE", location]),
        None,
        Duration::from_secs(30),
    );
    let mounted = outcome.status == Some(0)
        && outcome
            .stdout
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            == Some(location);
    if !mounted {
        return Err(refuse(format!(
            "no bind mount is present at {location} (findmnt: {})",
            match outcome.status {
                Some(status) => format!("exit {status}"),
                None => outcome.stderr.trim().to_owned(),
            }
        )));
    }
    Ok(json!({
        "action": "verified",
        "slot": agent.slot,
        "location": location,
        "phase": phase,
    }))
}

fn systemctl(host: &dyn Host, verb: &str, unit: &str) -> CommandOutcome {
    host.run(
        &strings(&[&systemctl_program(), "--user", verb, unit]),
        None,
        Duration::from_secs(90),
    )
}

/// The registered names, in either registry layout, sorted; restore's own files never look like
/// names.
pub(crate) fn registered_names(registry: &Path) -> Result<Vec<String>> {
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

/// The recorded lifecycle of one agent, for registry-derived desired state (#1129).
///
/// An unreadable, unparseable, or lifecycle-less record is undecidable: the caller refuses
/// it with this evidence instead of silently excluding it, because derivation cannot tell
/// whether it was running at shutdown.
pub(crate) fn derived_lifecycle(
    registry: &Path,
    name: &str,
) -> std::result::Result<String, String> {
    let path = record_path(registry, name)
        .ok_or_else(|| format!("{name} has no readable record path in either layout"))?;
    let bytes =
        fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let record: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{} is not valid JSON: {error}", path.display()))?;
    record["lifecycle"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{} records no lifecycle", path.display()))
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
    let manager_restarted = report["manager_restart_detected"] == true;
    let mut lines = vec![
        if manager_restarted {
            format!(
                "[agentctl restore] The user manager restarted on boot {}; every user unit stopped, so this run restored for real instead of replaying the earlier epoch's report.",
                report["boot_id"].as_str().unwrap_or("?")
            )
        } else if restarted {
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
        format!(
            "Desired state: {}.",
            match report["desired_state_source"].as_str() {
                Some("pinned-list") => "pinned-list (restore.json agents, plus the coordinator)",
                _ => "registry-derived (every record running at shutdown; restore.json exclude left alone)",
            }
        ),
        format!("Revived: {}.", list("revived", &pane)),
        format!(
            "Queued prompts after revive: {}.",
            {
                let items = agents
                    .iter()
                    .filter(|row| {
                        row["queued_delivered"].as_u64().unwrap_or(0) > 0
                            || row["queued_archived"].as_u64().unwrap_or(0) > 0
                    })
                    .map(|row| {
                        format!(
                            "{} ({} delivered, {} archived)",
                            row["name"].as_str().unwrap_or("?"),
                            row["queued_delivered"].as_u64().unwrap_or(0),
                            row["queued_archived"].as_u64().unwrap_or(0),
                        )
                    })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    "none".to_owned()
                } else {
                    items.join("; ")
                }
            }
        ),
        format!(
            "STARTED FRESH, no memory of their previous conversation (give them their task again): {}.",
            list("revived-fresh", &reason)
        ),
        format!("Promoted: {}.", list("promoted", &pane)),
        format!(
            "Died right after launch (this boot stays incomplete; a retry will attempt them again): {}.",
            list("died-after-launch", &reason)
        ),
        format!("Refused: {}.", list("refused", &reason)),
        format!(
            "Slot owner transfer refused (the lane is up; run its remedy): {}.",
            {
                let items = agents
                    .iter()
                    .filter(|row| row["slot_owner"]["action"] == "refused")
                    .map(|row| {
                        format!(
                            "{} ({}; {})",
                            row["name"].as_str().unwrap_or("?"),
                            row["slot_owner"]["reason"].as_str().unwrap_or("?"),
                            row["slot_owner"]["remedy"].as_str().unwrap_or("?")
                        )
                    })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    "none".to_owned()
                } else {
                    items.join("; ")
                }
            }
        ),
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
    // #1977: Eden refusals are visible in the notice without ever
    // changing the restore exit class; the lanes still revived.
    let mut eden_refused_slots: Vec<String> = Vec::new();
    for row in rows("eden_daemons")
        .iter()
        .filter(|row| row["status"] == "refused")
        .filter_map(|row| row["slot"].as_str().map(str::to_owned))
    {
        if !eden_refused_slots.contains(&row) {
            eden_refused_slots.push(row);
        }
    }
    if !eden_refused_slots.is_empty() {
        lines.push(format!(
            "Eden daemon restarts refused: {} ({}); the affected lanes were still revived, and the daemons can be restarted with 'wrkslots eden restart-slot-daemons'.",
            eden_refused_slots.len(),
            eden_refused_slots.join(", ")
        ));
    }
    if restarted {
        lines.push(
            "Session crons and background shells did not survive; re-create them from your notes."
                .to_owned(),
        );
    }
    // #1131: the notice and the report agree on who got a resume brief.
    if let Some(briefs) = report["resume_briefs"].as_array() {
        let briefed = briefs
            .iter()
            .filter(|row| matches!(row["action"].as_str(), Some("queued" | "already-queued")))
            .filter_map(|row| row["name"].as_str())
            .collect::<Vec<_>>();
        let coordinator = report["coordinator"]["name"].as_str();
        let unbriefed = agents
            .iter()
            .filter(|row| row["name"].as_str() != coordinator)
            .filter(|row| {
                matches!(
                    row["action"].as_str(),
                    Some("revived" | "revived-fresh" | "promoted")
                )
            })
            .filter_map(|row| row["name"].as_str())
            .filter(|name| !briefed.contains(name))
            .collect::<Vec<_>>();
        let join_or_none = |names: &[&str]| {
            if names.is_empty() {
                "none".to_owned()
            } else {
                names.join("; ")
            }
        };
        lines.push(format!(
            "Resume briefs queued: {}. Not briefed: {}.",
            join_or_none(&briefed),
            join_or_none(&unbriefed)
        ));
    }
    lines.push(format!("Report: {}", path.display()));
    lines.join("\n")
}

/// Run the restore and return its report; the report is also written unless `dry_run`.
/// Refresh the desired-state `agents` list in `<registry>/restore.json` from the live,
/// non-coordinator records (#742 spec slice, #810). Membership follows live state, not
/// the recorded lifecycle alone: a record whose pane exists, whose harness process is
/// alive, or whose harness is in the foreground is never dropped. Preview by default;
/// `apply` refuses while record/live mismatches are unresolved, and otherwise writes
/// atomically, preserving every other key. Creates a minimal file when none exists.
pub fn spec_write_agents<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    apply: bool,
) -> Result<Value> {
    let config = RestoreConfig::load(registry)?;
    // The designation follows the agent by token, so resolve it the way the
    // restore itself does rather than reading the file's (possibly old) name.
    // A configured coordinator kept as a rename alias resolves the same way.
    let manager = crate::subagents::ManagedAgents::new(client, registry);
    let coordinator = match config.coordinator.clone() {
        Some(name) => match &manager {
            Ok(manager) => manager.canonical_name(&name).ok().or(Some(name)),
            Err(_) => Some(name),
        },
        None => manager
            .and_then(|manager| manager.designated_coordinator())
            .unwrap_or_default(),
    };
    let live_panes: std::collections::BTreeSet<String> = client
        .panes()
        .map(|panes| panes.into_iter().map(|pane| pane.pane_id).collect())
        .unwrap_or_default();
    let mut desired: Vec<String> = Vec::new();
    let mut mismatches: Vec<Value> = Vec::new();
    for name in registered_names(registry)? {
        if coordinator.as_deref() == Some(name.as_str()) {
            continue;
        }
        let directory = agent_queue::registry::directory(registry, &name)
            .map_err(|error| fail(error.to_string()))?;
        let Ok(text) = fs::read_to_string(directory.join("agent.json")) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let recorded_running = record["lifecycle"] == "running";
        let pane_alive = record["pane_id"]
            .as_str()
            .is_some_and(|pane| live_panes.contains(pane));
        let foreground = client.agent_pane(&name).is_ok();
        let identity = ["harness_identity", "custom_process_identity"]
            .into_iter()
            .find_map(|key| {
                serde_json::from_value::<crate::client::CustomProcessIdentity>(record[key].clone())
                    .ok()
            });
        let process = identity
            .as_ref()
            .map(|identity| client.process_liveness(identity));
        let process_alive = process == Some(crate::client::ProcessLiveness::Alive);
        let alive = pane_alive || foreground || process_alive;
        if recorded_running != alive
            && (alive || process == Some(crate::client::ProcessLiveness::Dead))
        {
            mismatches.push(json!({
                "name": name,
                "recorded_lifecycle": record["lifecycle"],
                "evidence": {
                    "pane_alive": pane_alive,
                    "foreground": foreground,
                    "process": match process {
                        Some(crate::client::ProcessLiveness::Alive) => "alive",
                        Some(crate::client::ProcessLiveness::Dead) => "dead",
                        _ => "unknown",
                    },
                },
                "resolution": "repair the record (agentctl status/anchor), or edit restore.json agents explicitly to keep or drop it",
            }));
        }
        if recorded_running || alive {
            desired.push(name);
        }
    }
    desired.sort();
    if apply && !mismatches.is_empty() {
        return Err(fail(format!(
            "restore --write-agents --apply refused: {} record/live mismatch(es) unresolved ({}); repair the record or edit restore.json agents explicitly",
            mismatches.len(),
            mismatches
                .iter()
                .filter_map(|row| row["name"].as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let before = config.agents.clone().unwrap_or_default();
    let added: Vec<&String> = desired.iter().filter(|n| !before.contains(n)).collect();
    let removed: Vec<&String> = before.iter().filter(|n| !desired.contains(n)).collect();
    if apply {
        let path = registry.join(CONFIG_FILE);
        let mut document: Value = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| {
                let mut document = json!({"schema": CONFIG_SCHEMA});
                if let Some(name) = coordinator.as_ref() {
                    document["coordinator"] = json!(name);
                }
                document
            });
        document["agents"] = json!(desired);
        crate::agent::atomic_json(&path, &document)?;
    }
    Ok(json!({
        "action": if apply { "written" } else { "preview" },
        "coordinator": coordinator,
        "before": before,
        "after": desired,
        "added": added,
        "removed": removed,
        "mismatches": mismatches,
    }))
}

/// The process exit class for a restore report (#1132): 0 only when the
/// (recorded, for a replay) run completed with no refusal, or for a dry run;
/// 74 when it completed except for refused lanes (named in `refused_lanes`,
/// not a failure a boot unit should retry); 75 otherwise. An
/// `already-restored` replay has no class of its own: it follows the
/// recorded run it replayed.
pub fn restore_exit_code(report: &Value, dry_run: bool) -> i32 {
    if dry_run || (report["complete"] == true && report["refused"] == 0) {
        0
    } else if report["complete"] == true && report["refused"] != 0 {
        74
    } else {
        75
    }
}

/// Repair whatever differs from the desired state: revive the coordinator, then the
/// agents, then start the units, and write the per-boot report.
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
    // A second restore waits for the first, then replays its completed report.
    lock.lock_exclusive()
        .map_err(|error| fail(format!("cannot take the restore lock: {error}")))?;
    let boot_id = host.boot_id()?;
    let epoch = RestoreEpoch {
        boot_id: boot_id.clone(),
        user_manager: host.user_manager_id(),
    };
    let path = report_path(registry, config, &boot_id);
    let previous = read_boot_report(registry, config, &boot_id);
    // #1405: a recorded report from an earlier epoch (same boot, restarted
    // user manager) is evidence of the restart, never a replay candidate.
    let previous_epoch = previous
        .as_ref()
        .and_then(|(_, report)| RestoreEpoch::from_report(report));
    let epoch_changed = previous_epoch
        .as_ref()
        .is_some_and(|recorded| *recorded != epoch);
    // Lanes a killed attempt already revived this boot (#1934): a partial
    // report from this same epoch names them, and the restarted run
    // carries those rows forward after re-proving each lane alive,
    // instead of reviving it again. Only the revived class resumes;
    // refused, excluded and skipped rows are decided afresh.
    let prior_revived: std::collections::BTreeMap<String, Value> = if options.dry_run {
        Default::default()
    } else if let Some((_, prior)) = &previous {
        if prior["complete"] != true && RestoreEpoch::from_report(prior).as_ref() == Some(&epoch) {
            std::iter::once(&prior["coordinator"])
                .chain(prior["agents"].as_array().into_iter().flatten())
                .filter(|row| {
                    matches!(
                        row["action"].as_str(),
                        Some("revived" | "promoted" | "revived-fresh")
                    )
                })
                .filter_map(|row| {
                    row["name"]
                        .as_str()
                        .map(|name| (name.to_owned(), row.clone()))
                })
                .collect()
        } else {
            Default::default()
        }
    } else {
        Default::default()
    };
    if !options.force && !options.dry_run {
        // The replay candidate is the shared completed pick, not merely
        // the first readable report: a partial report at the current
        // location must not shadow this epoch's completed legacy one.
        let completed =
            completed_boot_report(registry, config, &epoch, host.user_manager_started());
        if let Some((previous_path, previous)) = &completed {
            {
                let previous_epoch = RestoreEpoch::from_report(previous);
                // #1132: replay the recorded outcomes (rows, refusals and
                // completion fields) labelled as a replay, so a rerun can
                // never read as a fresh success for a boot that refused
                // lanes. The caller maps this to the recorded exit class.
                let mut replayed = previous.clone();
                if let Some(object) = replayed.as_object_mut() {
                    object.entry("coordinator").or_insert(Value::Null);
                    object.entry("agents").or_insert_with(|| json!([]));
                    if !object.contains_key("refused") || !object.contains_key("refused_lanes") {
                        let lanes: Vec<Value> = object
                            .get("agents")
                            .and_then(Value::as_array)
                            .map(|rows| {
                                rows.iter()
                                    .filter(|row| row["action"] == "refused")
                                    .filter_map(|row| row["name"].as_str().map(|name| json!(name)))
                                    .collect()
                            })
                            .unwrap_or_default();
                        object
                            .entry("refused")
                            .or_insert_with(|| json!(lanes.len()));
                        object
                            .entry("refused_lanes")
                            .or_insert_with(|| json!(lanes));
                    }
                    if !object.contains_key("would_complete") {
                        let clean = object.get("refused") == Some(&json!(0));
                        object.insert("would_complete".to_owned(), json!(clean));
                    }
                    object
                        .entry("schema")
                        .or_insert_with(|| json!(REPORT_SCHEMA));
                    object.insert("boot_id".to_owned(), json!(boot_id));
                    object.insert("epoch".to_owned(), epoch.as_json());
                    object.insert("action".to_owned(), json!("already-restored"));
                    object.insert("replayed".to_owned(), json!(true));
                    object.insert("report".to_owned(), json!(previous_path));
                    // #1405: a replay names the report and epoch it replayed.
                    object.insert(
                        "replayed_report".to_owned(),
                        json!(previous_path.display().to_string()),
                    );
                    if let Some(recorded) = &previous_epoch {
                        object.insert("replayed_epoch".to_owned(), recorded.as_json());
                    }
                }
                return Ok(replayed);
            }
        }
    }
    let names = match registered_names(registry) {
        Ok(names) => names,
        Err(error) => {
            if !options.dry_run {
                write_partial_report(
                    &path,
                    &json!({
                        "schema": REPORT_SCHEMA,
                        "boot_id": boot_id,
                        "epoch": epoch.as_json(),
                        "started_at": host.now(),
                        "dry_run": false,
                        "report": path,
                    }),
                    host.now(),
                    &error.to_string(),
                );
            }
            return Err(error);
        }
    };
    let boots = anchored_boots(registry, &names);
    let mut report = json!({
        "schema": REPORT_SCHEMA,
        "boot_id": boot_id,
        "epoch": epoch.as_json(),
        // #1405: the recorded report this run supersedes belonged to an
        // earlier epoch of the same boot (a restarted user manager).
        "previous_epoch": if epoch_changed { previous_epoch.as_ref().map(RestoreEpoch::as_json).unwrap_or(Value::Null) } else { Value::Null },
        "manager_restart_detected": epoch_changed,
        "boot_time": host.boot_time(),
        "previous_boots": boots.iter().filter(|boot| **boot != boot_id).collect::<Vec<_>>(),
        "restart_detected": boots.iter().any(|boot| *boot != boot_id) || epoch_changed,
        "started_at": host.now(),
        "dry_run": options.dry_run,
        // #1129: why each lane was in or out of the desired set.
        "desired_state_source": if config.agents.is_some() { "pinned-list" } else { "registry-derived" },
        "report": path,
    });
    // Progress is durable from the start (#1934): a killed attempt leaves
    // the lanes it finished in the boot's report, and the restart resumes
    // from them. Best-effort, and never written for a dry run.
    let write_progress = |report: &Value| {
        if !options.dry_run {
            write_progress_report(&path, report, host.now());
        }
    };
    write_progress(&report);
    let project = registry.parent().unwrap_or(registry);
    // Read-only, so it is computed before the remount (#2089): the
    // remount must cover every project a desired slotted agent records.
    let boxed = slot_agents(registry, &names, config);
    let projects = slot_image_projects(&boxed, project);
    // `slot_images` keeps its exact single-project shape (the restore
    // project's row, first in `projects`); `slot_images_by_project` is
    // the additive per-project form, one row per project in the same
    // order. One project's failure never blocks the others or restore.
    let (slot_images, images_by_project): (Value, Vec<Value>) = if options.dry_run {
        (
            json!({"action": "skipped", "reason": "dry run"}),
            projects
                .iter()
                .map(
                    |project| json!({"action": "skipped", "reason": "dry run", "project": project}),
                )
                .collect(),
        )
    } else {
        let rows: Vec<Value> = projects
            .iter()
            .map(|project| {
                let mut row = remount_slot_images(host, options.wrkslots.as_deref(), project);
                row["project"] = json!(project);
                row
            })
            .collect();
        (
            rows.first().cloned().unwrap_or_else(
                || json!({"action": "skipped", "reason": "no wrkslots configured or on PATH"}),
            ),
            rows,
        )
    };
    report["slot_images"] = slot_images;
    report["slot_images_by_project"] = json!(images_by_project);
    // After a restart, every running agent boxed to a slot is dead: hold its slot so no
    // cleanup can retire it before the agent is revived and owns it again.
    // #1977, #2089: restart the covered projects' slots' Eden daemons
    // after the image remount and before the holds, one verb run per
    // project. Like the holds below, the step runs only when a restart
    // was detected: on an ordinary same-boot run a stopped daemon was
    // stopped on purpose and must not come back. A refused daemon is a
    // report row (`eden_refused` counts them across projects), never a
    // failed restore.
    let eden_rows = if options.dry_run || report["restart_detected"] != true {
        Vec::new()
    } else {
        restart_eden_daemons(
            host,
            options.wrkslots.as_deref(),
            &boxed,
            &images_by_project,
        )
    };
    report["eden_refused"] = json!(eden_rows
        .iter()
        .filter(|row| row["status"] == "refused")
        .count());
    report["eden_daemons"] = json!(eden_rows);
    report["slot_holds"] = match options.wrkslots.as_deref() {
        Some(wrkslots) if report["restart_detected"] == true && !options.dry_run => json!(boxed
            .iter()
            .filter(|agent| agent.running)
            .map(|agent| hold_slot(host, wrkslots, agent))
            .collect::<Vec<_>>()),
        _ => json!([]),
    };
    // The certificate comes first (#742, #1137): without a usable one every
    // harness refuses to start, so wait bounded for it, then name the exact
    // fetch command and revive nothing.
    let (certificate_ok, certificate) = wait_for_certificate(host, config, options.dry_run);
    report["certificate"] = certificate;
    // Version pin (#742): name a Herdr update instead of restoring against it silently.
    if let Some(expected) = config.herdr_version.as_deref() {
        if !options.herdr_bin.as_os_str().is_empty() {
            let argv = vec![
                options.herdr_bin.display().to_string(),
                "--version".to_owned(),
            ];
            let observed = host.run(&argv, None, Duration::from_secs(15));
            let observed_text = observed.stdout.trim().to_owned();
            let ok = observed.status == Some(0) && observed_text.contains(expected);
            report["herdr_version"] = json!({
                "expected": expected,
                "observed": observed_text,
                "ok": ok,
            });
            if !ok {
                eprintln!(
                    "agentctl restore: warning: herdr version mismatch: expected {expected}, observed {observed_text:?}"
                );
            }
        }
    }
    report["herdr"] = wait_for_herdr(client, host, Duration::from_secs(config.herdr_wait_seconds));
    // Desired state includes Herdr up (#742): start its unit and wait again.
    if report["herdr"]["ready"] != true {
        if let Some(unit) = config.herdr_unit.as_deref() {
            if !options.dry_run {
                let _ = systemctl(host, "start", unit);
                report["herdr"]["started_unit"] = json!(unit);
                report["herdr"] =
                    wait_for_herdr(client, host, Duration::from_secs(config.herdr_wait_seconds));
                report["herdr"]["started_unit"] = json!(unit);
            }
        }
    }
    let herdr_ready = certificate_ok && report["herdr"]["ready"] == true;
    let manager = match ManagedAgents::new(client, registry) {
        Ok(manager) => manager,
        Err(error) => {
            if !options.dry_run {
                write_partial_report(&path, &report, host.now(), &error.to_string());
            }
            return Err(error);
        }
    };
    let mut coordinator_ok = config.coordinator.is_none();
    if herdr_ready {
        if let Some(name) = config.coordinator.as_deref() {
            // #1130 D3: a slotted coordinator gets the same bind check and
            // slot-owner transfer as every other lane.
            let bind = match boxed.iter().find(|agent| agent.name == name) {
                Some(agent) if options.wrkslots.is_some() && !options.dry_run => {
                    Some(verify_slot_bind(
                        host,
                        agent,
                        project_image_row(&images_by_project, &agent.project),
                    ))
                }
                _ => None,
            };
            let mut row = if !names.iter().any(|known| known == name) {
                json!({"name": name, "action": "refused", "reason": "the coordinator is not registered"})
            } else if let Some(Err(reason)) = &bind {
                json!({"name": name, "action": "refused", "reason": reason, "slot_bind": {"action": "refused"}})
            } else {
                recover_agent(&manager, name, options)
            };
            if let Some(Ok(verified)) = &bind {
                row["slot_bind"] = verified.clone();
            }
            record_slot_owner(
                &mut row,
                &manager,
                host,
                options.wrkslots.as_deref(),
                &boxed,
            );
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
            write_progress(&report);
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
                report["agents"] = json!(agents.clone());
                write_progress(&report);
                continue;
            }
            if let Some(desired) = config.agents.as_ref() {
                if !desired.iter().any(|wanted| wanted == name) {
                    agents.push(json!({"name": name, "action": "excluded", "reason": "not listed in restore.json agents (desired state)"}));
                    report["agents"] = json!(agents.clone());
                    write_progress(&report);
                    continue;
                }
            } else {
                // Registry-derived desired state (#1129): only a record that was
                // running at shutdown (or a launch_failed record the promote path
                // can still prove live) is derived. Stopped, archived, and other
                // lifecycles are skipped, never revived; an undecidable record is
                // refused with its evidence, never silently excluded.
                match derived_lifecycle(registry, name) {
                    Err(evidence) => {
                        agents.push(json!({
                            "name": name,
                            "action": "refused",
                            "reason": format!("cannot derive desired state for {name}: {evidence}"),
                        }));
                        report["agents"] = json!(agents.clone());
                        write_progress(&report);
                        continue;
                    }
                    Ok(lifecycle) if lifecycle != "running" && lifecycle != "launch_failed" => {
                        agents.push(json!({
                            "name": name,
                            "action": "skipped",
                            "reason": format!(
                                "lifecycle is {lifecycle}, not running at shutdown; registry-derived restore leaves it down"
                            ),
                        }));
                        report["agents"] = json!(agents.clone());
                        write_progress(&report);
                        continue;
                    }
                    Ok(_) => {}
                }
            }
            // #1130 D3: never revive a slot lane into a slot whose bind
            // mount is missing; refuse that lane and carry on with the rest.
            let bind = match boxed.iter().find(|agent| agent.name == *name) {
                Some(agent) if options.wrkslots.is_some() && !options.dry_run => {
                    Some(verify_slot_bind(
                        host,
                        agent,
                        project_image_row(&images_by_project, &agent.project),
                    ))
                }
                _ => None,
            };
            if let Some(Err(reason)) = &bind {
                agents.push(json!({
                    "name": name,
                    "action": "refused",
                    "reason": reason,
                    "slot_bind": {"action": "refused"},
                }));
                report["agents"] = json!(agents.clone());
                write_progress(&report);
                continue;
            }
            // Resume (#1934): a lane the killed attempt already revived
            // this boot, still provably alive, is carried forward, not
            // revived again. The dry-run plan is the proof, and only
            // its alive skip proves life: a lifecycle skip ("record
            // lifecycle is not running") proves nothing, so such a lane
            // is recovered normally, never carried forward (#1934 D3).
            if let Some(prior) = prior_revived.get(name.as_str()) {
                let alive = manager
                    .revive(name, true, None, options.startup_timeout)
                    .is_ok_and(|plan| {
                        plan["action"] == "skip"
                            && plan["reason"] == "recorded harness process is alive"
                    });
                if alive {
                    let mut row = prior.clone();
                    row["resumed"] = json!(true);
                    agents.push(row);
                    report["agents"] = json!(agents.clone());
                    write_progress(&report);
                    continue;
                }
            }
            eprintln!("agentctl restore: restoring {name}");
            let mut row = recover_agent(&manager, name, options);
            if let Some(Ok(verified)) = &bind {
                row["slot_bind"] = verified.clone();
            }
            let launched = matches!(row["action"].as_str(), Some("revived"));
            record_slot_owner(
                &mut row,
                &manager,
                host,
                options.wrkslots.as_deref(),
                &boxed,
            );
            agents.push(row);
            report["agents"] = json!(agents.clone());
            write_progress(&report);
            if launched && config.startup_delay_seconds > 0 {
                host.sleep(Duration::from_secs(config.startup_delay_seconds));
            }
        }
        report["agents"] = json!(agents);
    }
    // Survival re-check (#1137): a launch is counted started at first
    // detection, but a harness can still die seconds later (an unusable
    // certificate is the boot case). Re-resolve every lane this run
    // launched before the boot is called restored; one whose runtime is
    // already gone is died-after-launch and keeps the boot incomplete, so
    // the bounded unit retry attempts it again instead of a later run
    // reading already-restored.
    let mut launched_survived = true;
    if !options.dry_run && herdr_ready {
        fn launched(row: &Value) -> bool {
            matches!(
                row["action"].as_str(),
                Some("revived" | "promoted" | "revived-fresh")
            )
        }
        let mut recheck = |row: &mut Value| {
            if !launched(row) {
                return;
            }
            let Some(name) = row["name"].as_str().map(str::to_owned) else {
                return;
            };
            if let Err(error) = manager.pane_info(&name) {
                row["action"] = json!("died-after-launch");
                row["reason"] = json!(format!("launched but its runtime did not survive: {error}"));
                launched_survived = false;
            }
        };
        if report["coordinator"].is_object() {
            recheck(&mut report["coordinator"]);
            if report["coordinator"]["action"] == "died-after-launch" {
                coordinator_ok = false;
            }
        }
        if let Some(rows) = report["agents"].as_array_mut() {
            for row in rows.iter_mut() {
                recheck(row);
            }
        }
    }
    // Per-lane disposition in the owner's words, plus tab hygiene on the
    // Herdr auto-restored shells (item 5 restore-in-place): a lane that came
    // back in its recorded pane is resumed-in-place, one left alone is
    // parked, one that could not come back failed. An orphan bare shell
    // whose tab names a parked lane is labelled "parked: NAME"; one that
    // duplicates a revived lane's tab is closed. Registered panes and
    // running programs are never touched.
    if !options.dry_run {
        if let Some(rows) = report["agents"].as_array_mut() {
            for row in rows.iter_mut() {
                let disposition = match row["action"].as_str().unwrap_or_default() {
                    "revived" | "promoted" => "resumed-in-place",
                    "revived-fresh" => "resumed-fresh",
                    "excluded" | "skipped" => "parked",
                    _ => "failed",
                };
                row["disposition"] = json!(disposition);
            }
        }
        let mut tidy = Vec::new();
        if let Ok(panes) = client.panes() {
            let claimed: std::collections::BTreeSet<String> = names
                .iter()
                .filter_map(|name| manager.census_record(name).ok())
                .filter_map(|status| status["pane_id"].as_str().map(str::to_owned))
                .collect();
            let cwd_of = |name: &str| {
                manager
                    .census_record(name)
                    .ok()
                    .and_then(|s| s["cwd"].as_str().map(str::to_owned))
            };
            let pane_of = |name: &str| {
                manager
                    .census_record(name)
                    .ok()
                    .and_then(|s| s["pane_id"].as_str().map(str::to_owned))
            };
            for pane in panes {
                if claimed.contains(&pane.pane_id) {
                    continue;
                }
                if !client.pane_is_idle_shell(&pane.pane_id).unwrap_or(false) {
                    continue;
                }
                let Ok(info) = client.pane_info(&pane.pane_id) else {
                    continue;
                };
                let label = info
                    .tab_id
                    .as_deref()
                    .and_then(|tab| client.tab_label(tab).ok())
                    .unwrap_or_default();
                if label.starts_with("parked:") {
                    continue;
                }
                // Parked labelling is only for lanes that did not come
                // back (excluded/skipped); a duplicate of a lane that DID
                // resume in place must be closed below, not labelled.
                let disposition_of = |name: &str| {
                    report["agents"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|row| row["name"].as_str() == Some(name))
                        .and_then(|row| row["disposition"].as_str().map(str::to_owned))
                };
                let parked_lane = names.iter().find(|name| {
                    label == **name
                        && cwd_of(name).as_deref() == Some(info.cwd.as_str())
                        && disposition_of(name).as_deref() == Some("parked")
                });
                if let (Some(name), Some(tab)) = (parked_lane, info.tab_id.as_deref()) {
                    if client.rename_tab(tab, &format!("parked: {name}")).is_ok() {
                        tidy.push(json!({
                            "pane_id": pane.pane_id, "tab_id": tab, "cwd": info.cwd,
                            "action": "labelled-parked", "lane": name,
                        }));
                    }
                    continue;
                }
                let duplicate = names.iter().find(|name| {
                    label == **name
                        && pane_of(name).is_some_and(|pane| claimed.contains(&pane))
                        && matches!(
                            disposition_of(name).as_deref(),
                            Some("resumed-in-place" | "resumed-fresh")
                        )
                });
                if duplicate.is_some() && client.close_pane(&pane.pane_id).is_ok() {
                    tidy.push(json!({
                        "pane_id": pane.pane_id, "tab_id": info.tab_id, "cwd": info.cwd,
                        "action": "closed-duplicate", "lane": duplicate,
                    }));
                }
            }
        }
        report["tidy"] = json!(tidy);
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
    // One tab-hygiene run at the end of restore (#1971): canonical order
    // and bare-shell cultivation must not wait for a watch daemon that
    // may not be running. It shares the standalone grace clocks with
    // `agentctl tabs --apply`, so this pass starts the clocks a later
    // timer pass continues; a failure is recorded, never a restore
    // failure.
    report["tab_hygiene"] = if options.dry_run {
        json!({"action": "skipped", "reason": "dry run"})
    } else if !config.duties.tab_hygiene {
        json!({"action": "skipped", "reason": "tab_hygiene duty off"})
    } else {
        // Guarded socket (#1971 nit): only the Herdr serving this
        // restore's binary may have its tabs moved.
        let socket = crate::tab_order::socket_for(&options.herdr_bin);
        match tabs_apply(client, registry, config, host, socket.as_deref()) {
            Ok(applied) => applied,
            Err(error) => json!({"applied": false, "detail": error.to_string()}),
        }
    };
    // #1130: a refused slot-owner transfer counts as a named refusal too,
    // the coordinator's included, even though the lane itself revived.
    let transfer_refused =
        |row: &Value| row["action"] == "refused" || row["slot_owner"]["action"] == "refused";
    let mut refused_lanes: Vec<Value> = Vec::new();
    if transfer_refused(&report["coordinator"]) && report["coordinator"]["action"] != "refused" {
        if let Some(name) = report["coordinator"]["name"].as_str() {
            refused_lanes.push(json!(name));
        }
    }
    refused_lanes.extend(
        report["agents"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|row| transfer_refused(row))
                    .filter_map(|row| row["name"].as_str().map(|name| json!(name)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );
    let refused = refused_lanes.len();
    report["refused"] = json!(refused);
    // The summary names every refused lane, so a boot log shows at a
    // glance which lanes still need a hand-bound session (#948).
    report["refused_lanes"] = json!(refused_lanes);
    // Loud: every agent that lost its conversation, the coordinator included.
    let fresh_starts: Vec<Value> = std::iter::once(&report["coordinator"])
        .chain(report["agents"].as_array().into_iter().flatten())
        .filter(|row| row["action"] == "revived-fresh")
        .map(|row| row["warning"].clone())
        .collect();
    report["fresh_starts"] = json!(fresh_starts);
    // Lanes carried forward from the killed attempt's partial report
    // (#1934), named so the boot log shows the restart resumed.
    report["resumed_lanes"] = json!(report["agents"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter(|row| row["resumed"] == true)
                .filter_map(|row| row["name"].as_str().map(|name| json!(name)))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default());
    report["in_progress"] = json!(false);
    report["complete"] =
        json!(herdr_ready && coordinator_ok && launched_survived && !options.dry_run);
    // A dry run never completes; this says whether the same run for real would.
    report["would_complete"] =
        json!(herdr_ready && coordinator_ok && launched_survived && refused == 0);
    report["notice"] = Value::Null;
    // Stamped before the briefs render it (#1131 gives each brief a finished_at).
    report["finished_at"] = json!(host.now());
    // The standard per-lane resume brief (#1131), queued before the
    // coordinator's notice so the notice can name the unbriefed lanes.
    report["resume_briefs"] = if !options.dry_run && herdr_ready {
        crate::restore_notice::resume_briefs(client, registry, config, &report, &boot_id)
    } else {
        Value::Null
    };
    let summary = config
        .coordinator
        .as_deref()
        .map(|_| notice_text(&report, host.boot_time(), &path));
    if !options.dry_run && herdr_ready && coordinator_ok {
        if let (Some(name), Some(text)) = (config.coordinator.as_deref(), summary.clone()) {
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
                // #1372: a later boot's notice replaces an undelivered earlier one.
                Some("restore-notice"),
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
    // The "I'm back" summary to every bridge registry and each revived agent's
    // catch-up prompt, queued without blocking (#746 restore-im-back).
    report["im_back"] = if !options.dry_run && herdr_ready && coordinator_ok {
        json!(crate::restore_notice::after_restore(
            client,
            registry,
            config,
            &report,
            summary.as_deref(),
            &boot_id,
        ))
    } else {
        Value::Null
    };
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
    /// A queued prompt pending longer than this is a finding, with its age (#804).
    pub queue_stale: Duration,
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

/// A duty switched off by config/flag (#1145): never healthy, never repaired,
/// never alerted (`ok` null, excluded from the health `ok` roll-up).
fn skipped(duty: &str) -> Value {
    let extra = if duty == "status_requests" {
        "; status_requests is owned by the notifier (watch-agents) in Phase 1 and gates nothing watch itself does yet"
    } else {
        ""
    };
    json!({"check": "duty", "subject": duty, "ok": Value::Null, "skipped": "skipped-by-config",
        "detail": format!("skipped-by-config: duty {duty} is off{extra}"), "repair": Value::Null})
}
fn ensure_skipped(findings: &mut Vec<Value>, duties: &WatchDuties) {
    for duty in DUTY_NAMES {
        if !duties.enabled(duty)
            && !findings
                .iter()
                .any(|r| r["check"] == "duty" && r["subject"] == duty)
        {
            findings.push(skipped(duty));
        }
    }
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
    let mut argv = strings(&[
        &systemctl_program(),
        "--user",
        "show",
        unit,
        "--timestamp=unix",
    ]);
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

/// Whether a timer problem is its schedule itself or only its freshness clock.
///
/// A broken schedule (not loaded, no next elapse) is repaired by restarting the
/// timer. A merely stale timer is repaired by starting its service once: restarting a timer
/// whose trigger counts from its service's last run resets that trigger, so the timer never
/// elapses on its own and the watch would restart it on every pass (#961). A timer an
/// operator stopped is never repaired at all: it is reported once per stop episode
/// (#1277), unless its entry sets `repair_stopped`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerProblemKind {
    ScheduleBroken,
    Stale,
    StoppedByOperator,
}

/// A timer problem the service manager confirmed, with the repair it calls for.
struct TimerProblem {
    detail: String,
    kind: TimerProblemKind,
}

/// A timer check that may not get an answer: `Ok(None)` healthy, `Ok(Some(problem))` a
/// confirmed problem, `Err(reason)` could not check.
type TimerCheck = std::result::Result<Option<TimerProblem>, String>;

/// A `@<unix-seconds>` property from a `--timestamp=unix` answer.
fn unix_timestamp(state: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    state
        .get(key)
        .and_then(|value| value.strip_prefix('@'))
        .and_then(|value| value.parse::<f64>().ok())
}

/// The service a timer triggers, if it has one.
fn triggered_service(
    host: &dyn Host,
    timer: &TimerWatch,
) -> std::result::Result<Option<String>, String> {
    let triggers = show(host, &timer.unit, &["Triggers"])?;
    Ok(triggers
        .get("Triggers")
        .filter(|value| unit_name(value))
        .cloned())
}

/// Why a timer is not keeping its schedule.
///
/// Freshness is the triggered service's last successful run, not `LastTriggerUSec` alone:
/// a monotonic-only timer's trigger never moves when the watch keeps restarting it, but
/// its service's `ExecMainExitTimestamp` with `Result=success` says when the work last
/// actually happened. The service is only consulted when the trigger clock already looks
/// stale, so a healthy timer costs the same queries as before.
fn timer_problem(host: &dyn Host, timer: &TimerWatch) -> TimerCheck {
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
        let confirmed = confirm_inactive_timer(host, timer, active)?;
        // An operator's `systemctl stop` leaves a loaded timer inactive, and restarting
        // it would undo that stop on every pass (#1277). Only an entry that opted into
        // repair, a timer in a failed state, or one that is not loaded at all takes the
        // schedule-broken path; a plain stop is reported, never started.
        if confirmed.active == "inactive" && confirmed.loaded && !timer.repair_stopped {
            let unit_file = show(host, &timer.unit, &["UnitFileState"])?["UnitFileState"].clone();
            return Ok(Some(TimerProblem {
                detail: format!(
                    "timer is inactive (UnitFileState={}); stopped by operator, not started",
                    if unit_file.is_empty() {
                        "unknown"
                    } else {
                        unit_file.as_str()
                    }
                ),
                kind: TimerProblemKind::StoppedByOperator,
            }));
        }
        return Ok(Some(TimerProblem {
            detail: confirmed.detail,
            kind: TimerProblemKind::ScheduleBroken,
        }));
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
        return Ok(Some(TimerProblem {
            detail: "timer is active but will never fire again (no next elapse)".to_owned(),
            kind: TimerProblemKind::ScheduleBroken,
        }));
    }
    let now = host.now();
    let max_age = timer.max_age_seconds as f64;
    let last = unix_timestamp(&state, "LastTriggerUSec");
    let trigger_stale = match last {
        Some(last) => now - last > max_age,
        None => host
            .boot_time()
            .is_some_and(|boot| now - boot as f64 > max_age),
    };
    if !trigger_stale {
        return Ok(None);
    }
    let trigger_detail = match last {
        Some(last) => format!(
            "last fired {:.0} seconds ago, more than {}",
            now - last,
            timer.max_age_seconds
        ),
        None => "has not fired since boot".to_owned(),
    };
    let Some(service) = triggered_service(host, timer)? else {
        return Ok(Some(TimerProblem {
            detail: trigger_detail,
            kind: TimerProblemKind::Stale,
        }));
    };
    let service_state = show(
        host,
        &service,
        &["ActiveState", "Result", "ExecMainExitTimestamp"],
    )?;
    // The work is happening now; a second start would claim a repair that is not needed.
    if matches!(
        service_state["ActiveState"].as_str(),
        "active" | "activating"
    ) {
        return Ok(None);
    }
    let result = service_state["Result"].as_str();
    // A failed service is reported by `service_failure`, never repaired and never a
    // freshness witness.
    if !matches!(result, "" | "success") {
        return Ok(None);
    }
    Ok(Some(TimerProblem {
        detail: match unix_timestamp(&service_state, "ExecMainExitTimestamp") {
            Some(exit) if result == "success" && now - exit <= max_age => return Ok(None),
            Some(exit) if result == "success" => format!(
                "{service} last succeeded {:.0} seconds ago, more than {} (timer {trigger_detail})",
                now - exit,
                timer.max_age_seconds
            ),
            _ => format!("{service} has no successful run on record (timer {trigger_detail})"),
        },
        kind: TimerProblemKind::Stale,
    }))
}

/// A confirmed inactive timer: what the service manager said on the second look.
struct ConfirmedInactive {
    detail: String,
    loaded: bool,
    active: String,
}

/// Look again, after a pause, before calling a timer inactive or unknown.
///
/// A reloading service manager can briefly answer for a timer it has not loaded yet, so the state
/// must hold [`SYSTEMCTL_RETRY_DELAY`] later, and a timer the manager calls not loaded must also be
/// missing from `list-timers --all`.
fn confirm_inactive_timer(
    host: &dyn Host,
    timer: &TimerWatch,
    first: &str,
) -> std::result::Result<ConfirmedInactive, String> {
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
        return Ok(ConfirmedInactive {
            detail: format!("timer is {active}"),
            loaded: true,
            active: active.to_owned(),
        });
    }
    let command = "systemctl --user list-timers --all";
    let listed = host.run(
        &strings(&[
            &systemctl_program(),
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
    Ok(ConfirmedInactive {
        detail: format!(
            "timer is not loaded (LoadState={}) and {command} does not list it",
            if load.is_empty() { "unknown" } else { load }
        ),
        loaded: false,
        active: active.to_owned(),
    })
}

/// Why the service a timer triggers failed on its last run.
///
/// A timer can keep a perfect schedule while its service fails on every run. Restarting either
/// cannot repair that, so this is reported, never repaired. `Result=` already honours the
/// service's `SuccessExitStatus=`.
fn service_failure(host: &dyn Host, timer: &TimerWatch) -> Check {
    let Some(service) = triggered_service(host, timer)? else {
        return Ok(None);
    };
    let state = show(host, &service, &["ActiveState", "Result", "ExecMainStatus"])?;
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
    timer_repaired_at: &mut BTreeMap<String, f64>,
    timer_stopped_noted: &mut std::collections::BTreeSet<String>,
) -> Vec<Value> {
    let mut findings = Vec::new();
    let duties = &config.duties;
    // `herdr_up` is still probed when the herdr duty is off: queue_drain and
    // the coordinator check need to know, but nothing is started or reported.
    let herdr_up = client.panes().is_ok();
    if !duties.herdr {
        findings.push(skipped("herdr"));
    } else if herdr_up {
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
    if !duties.unit_repair {
        findings.push(skipped("unit_repair"));
    } else {
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
                    // Seen keeping its schedule: any stop episode for it has ended.
                    timer_stopped_noted.remove(&timer.unit);
                    findings.push(match service_failure(host, timer) {
                        Err(reason) => unchecked("timer", &timer.unit, reason, Value::Null),
                        Ok(Some(failure)) => {
                            finding("timer", &timer.unit, false, failure, Value::Null)
                        }
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
                findings.push(finding(
                    "timer",
                    &timer.unit,
                    false,
                    problem.detail.clone(),
                    Value::Null,
                ));
                continue;
            }
            // A timer an operator stopped is never started (#1277): report it once per
            // stop episode, then carry the cooldown suppression until it is seen active
            // again, so neither stderr nor the coordinator alert repeats.
            if problem.kind == TimerProblemKind::StoppedByOperator {
                let mut row = finding(
                    "timer",
                    &timer.unit,
                    false,
                    problem.detail.clone(),
                    Value::Null,
                );
                row["operator_stopped"] = json!(true);
                if !timer_stopped_noted.insert(timer.unit.clone()) {
                    row["cooldown"] = json!(true);
                }
                findings.push(row);
                continue;
            }
            timer_stopped_noted.remove(&timer.unit);
            // One repair per timer per max_age window (#961): after a start, the service's
            // success timestamp has a full window to move before the timer is kicked again, and
            // a timer that has never run gets exactly one start, not one per pass.
            if let Some(last_repair) = timer_repaired_at
                .get(&timer.unit)
                .filter(|last| host.now() - *last < timer.max_age_seconds as f64)
            {
                let mut row = finding(
                "timer",
                &timer.unit,
                false,
                format!(
                    "{}; repair on cooldown (last repaired {:.0} seconds ago, one repair per {} second window)",
                    problem.detail,
                    host.now() - last_repair,
                    timer.max_age_seconds
                ),
                Value::Null,
            );
                row["cooldown"] = json!(true);
                findings.push(row);
                continue;
            }
            timer_repaired_at.insert(timer.unit.clone(), host.now());
            let mut repair: Vec<String> = Vec::new();
            match problem.kind {
                TimerProblemKind::ScheduleBroken => {
                    let _ = systemctl(host, "restart", &timer.unit);
                    repair.push(format!("systemctl --user restart {}", timer.unit));
                }
                TimerProblemKind::Stale => match triggered_service(host, timer) {
                    Ok(Some(service)) => {
                        let _ = systemctl(host, "start", &service);
                        repair.push(format!("systemctl --user start {service}"));
                    }
                    _ => {
                        let _ = systemctl(host, "restart", &timer.unit);
                        repair.push(format!("systemctl --user restart {}", timer.unit));
                    }
                },
                // Handled above: a stopped timer is reported, never started (#1277).
                TimerProblemKind::StoppedByOperator => {}
            }
            let mut remaining = timer_problem(host, timer);
            if problem.kind == TimerProblemKind::ScheduleBroken && matches!(remaining, Ok(Some(_)))
            {
                // A timer whose only trigger counts from its service's last run has no base until
                // that service runs once in this service manager.
                if let Ok(triggers) = show(host, &timer.unit, &["Triggers"]) {
                    if let Some(service) = triggers.get("Triggers").filter(|value| unit_name(value))
                    {
                        let _ = systemctl(host, "start", service);
                        repair.push(format!("systemctl --user start {service}"));
                        remaining = timer_problem(host, timer);
                    }
                }
            }
            let remaining = match remaining {
                Ok(None) => service_failure(host, timer).map(|failure| {
                    failure.map(|detail| TimerProblem {
                        detail,
                        kind: TimerProblemKind::Stale,
                    })
                }),
                other => other,
            };
            findings.push(match remaining {
                Ok(None) => finding(
                    "timer",
                    &timer.unit,
                    true,
                    format!("{}; repaired", problem.detail),
                    json!({"actions": repair}),
                ),
                Ok(Some(still)) => finding(
                    "timer",
                    &timer.unit,
                    false,
                    still.detail,
                    json!({"actions": repair}),
                ),
                Err(reason) => unchecked(
                    "timer",
                    &timer.unit,
                    format!(
                        "{}; repaired with {}, then {reason}",
                        problem.detail,
                        repair.join(" && ")
                    ),
                    json!({"actions": repair}),
                ),
            });
        }
    }
    let project = registry.parent().unwrap_or(registry);
    // Any wrkslots command remounts, so a check-only pass leaves slot images alone.
    if !duties.slot_images {
        findings.push(skipped("slot_images"));
    } else if let Some(wrkslots) = options.wrkslots.as_deref().filter(|_| !options.check_only) {
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
    if !duties.restore_check {
        findings.push(skipped("restore_check"));
    } else if let Ok(boot_id) = host.boot_id() {
        // #1405: a completed report from an earlier epoch of this boot
        // (before a user-manager restart) does not count as restored.
        let epoch = RestoreEpoch {
            boot_id: boot_id.clone(),
            user_manager: host.user_manager_id(),
        };
        let report_state = boot_report_state(registry, config, &epoch, host.user_manager_started());
        let early = host
            .boot_time()
            .is_some_and(|boot| host.now() - (boot as f64) < RESTORE_GRACE_SECONDS as f64);
        if config.coordinator.is_some() {
            // An unprovable pre-epoch report is said distinctly and never
            // alarmed: alarming every pass until someone forces a restore
            // (and its notices) helps nobody (#1426).
            let (ok, detail, missing): (bool, String, bool) = match &report_state {
                BootReportState::Completed(_) => {
                    (true, "this boot's restore completed".to_owned(), false)
                }
                BootReportState::Incomplete((path, report)) => (
                    false,
                    format!(
                        "restore report for this boot is incomplete ({}): {}",
                        path.display(),
                        incomplete_reason(report)
                    ),
                    false,
                ),
                BootReportState::PredatesKeying(path) => (
                    true,
                    format!(
                        "report predates epoch keying ({}); cannot prove it for this epoch",
                        path.display()
                    ),
                    false,
                ),
                BootReportState::Missing if early => (
                    true,
                    "boot is recent; restore may still run".to_owned(),
                    false,
                ),
                BootReportState::Missing => (
                    false,
                    "no completed restore report for this boot".to_owned(),
                    true,
                ),
            };
            let mut row = finding("restore", &boot_id, ok, &detail, Value::Null);
            if missing {
                row["missing_restore_report"] = json!(true);
            }
            findings.push(row);
        }
    }
    // Queued prompts must not sit parked (#804 agentctl-queue-only-drain): a
    // queue-only prompt is typed by this pass — at once for an idle agent, at the
    // next tool boundary for a working Muse or Claude Code agent — and one
    // still pending past the stale threshold is a finding naming its age.
    // The re-pin inside is record maintenance, not a duty; only typing/stale
    // reporting (the drain loop) is gated by queue_drain.
    if !duties.queue_drain {
        findings.push(skipped("queue_drain"));
    }
    if herdr_up {
        if let Ok(manager) = ManagedAgents::new(client, registry) {
            // R1b on the watch pass too: a harness hand-restarted in its
            // recorded pane with its recorded session is re-pinned here,
            // not only when someone sends to it.
            if !options.check_only {
                for name in crate::subagents::restore_agent_names(registry) {
                    if manager.auto_repin_resumed(&name) {
                        findings.push(finding(
                            "repin",
                            &name,
                            true,
                            "re-pinned a hand-restarted harness in its recorded pane",
                            Value::Null,
                        ));
                    }
                }
            }
            let now_millis = (host.now() * 1000.0) as u64;
            for row in if duties.queue_drain {
                manager.drain_pending(options.queue_stale, options.check_only, now_millis)
            } else {
                Vec::new()
            } {
                let name = row["name"].as_str().unwrap_or("?").to_owned();
                let age = row["oldest_seconds"]
                    .as_u64()
                    .map(|seconds| format!("; oldest {seconds}s"))
                    .unwrap_or_default();
                let remaining = row["remaining"].as_u64().unwrap_or(0);
                let stale = row["stale"] == true;
                findings.push(finding(
                    "queue",
                    &name,
                    remaining == 0 || !stale,
                    if options.check_only {
                        format!("{} prompt(s) queued{age}", row["queued"])
                    } else {
                        format!(
                            "{} prompt(s) queued{age}; typed {} this pass, {remaining} still pending{}",
                            row["queued"],
                            row["typed"],
                            if stale { " (past the stale threshold)" } else { "" }
                        )
                    },
                    row,
                ));
            }
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
    // Muse's plugin-hooks trust dialog (#1932): a managed Muse lane of
    // this registry parked at the exact dialog is answered per the
    // registry's policy; the manager re-reads the pane and re-verifies
    // the dialog before any keypress, and other registries' and
    // unmanaged panes are never touched.
    if herdr_up {
        if let Ok(manager) = ManagedAgents::new(client, registry) {
            for name in agent_queue::registry::names(registry).unwrap_or_default() {
                if options.check_only {
                    continue;
                }
                match manager.answer_hooks_trust_if_dialog(&name) {
                    Ok(Some(detail)) => {
                        eprintln!("agentctl watch: hooks trust: {detail}");
                        findings.push(finding("hooks_trust", &name, true, detail, Value::Null));
                    }
                    Ok(None) => {}
                    Err(error) => findings.push(finding(
                        "hooks_trust",
                        &name,
                        false,
                        error.to_string(),
                        Value::Null,
                    )),
                }
            }
        }
    }
    // Half-duty detection (#1145, config-level only in Phase 1: unit presence
    // is not probed beyond what the blocks above already query).
    if !(duties.unit_repair && duties.notify) {
        let missing: Vec<&str> = ["unit_repair", "notify"]
            .into_iter()
            .filter(|d| !duties.enabled(d))
            .collect();
        findings.push(json!({"check": "duties", "subject": "repair-notify-pair", "ok": Value::Null,
            "warning": "half-duty",
            "detail": format!("repair-notify pair incomplete: missing {} (config-level check; unit presence not probed); remedy: set the missing duty true under duties in {WATCH_CONFIG_FILE} or pass --duty NAME=on", missing.join(", ")),
            "repair": Value::Null}));
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

/// Report a missing restore report once per boot in this watch process
/// (#2018): the first failing row for a boot stays failing, later rows for
/// the same boot become a suppressed non-alerting row. In memory only, so
/// a watch restart may report it once more.
fn apply_restore_missing_once(
    findings: &mut [Value],
    noted: &mut std::collections::BTreeSet<String>,
) {
    for row in findings.iter_mut() {
        if row["check"] != "restore" || row["missing_restore_report"] != true {
            continue;
        }
        let boot = row["subject"].as_str().unwrap_or("").to_owned();
        if noted.insert(boot) {
            continue;
        }
        row["ok"] = Value::Null;
        row["detail"] =
            json!("no completed restore report for this boot (already reported for this boot)");
        row["suppressed"] = json!("reported-once-this-boot");
        row["skipped"] = json!("already-reported-this-boot");
    }
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
        // A stopped timer's later passes in the same episode are not new reports
        // (#1277): only its first, un-cooldowned row may alert.
        .filter(|row| row["operator_stopped"] != true || row["cooldown"] != true)
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
            None,
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

/// Run the watch: one pass with `once`, otherwise one pass per interval until termination is
/// requested (`Host::terminated`), when it answers the last pass's health. A pass in progress
/// finishes, its waits cut short; no other starts (#909 watch-sigterm-stop).
/// One claimed pane in a hygiene pass: agent name, tab, launch profile, and
/// the record's `role` field.
type ClaimedTab = (String, Option<String>, Option<String>, Option<String>);

/// One viewer tab claimed in a hygiene pass (#1225 workspace-goal-reconciler,
/// Phase 1): the agent it views, its launch profile, and its record's `role`
/// field. A headless agent has no pane, so its `tail --follow` or console tab
/// is claimed by tab alone and ranked with the agent it views.
type ViewerTab = (String, Option<String>, Option<String>);

/// The ranked tab entries of one workspace for [`hygiene_pass`]: each tab in
/// `order` with the name and canonical role it sorts by. A tab claimed by an
/// agent's pane ranks as that agent; a viewer tab with no pane claim ranks as
/// the agent it views; anything else is unmanaged and sorts last.
fn ranked_entries(
    order: &[String],
    claimed: &BTreeMap<String, ClaimedTab>,
    viewer_tabs: &BTreeMap<String, ViewerTab>,
    labels: &BTreeMap<String, String>,
    roles: &BTreeMap<String, String>,
    bridge_targets: &std::collections::BTreeSet<String>,
    designated: Option<&str>,
) -> Vec<(String, String, u8)> {
    let role = |name: &str, profile: Option<&str>, record_role: Option<&str>| {
        crate::tab_order::role_for(
            name,
            profile,
            profile
                .and_then(|profile| roles.get(profile))
                .map(String::as_str),
            record_role,
            bridge_targets,
            designated,
        )
    };
    order
        .iter()
        .map(|tab| {
            let lane = claimed
                .values()
                .find(|(_, lane_tab, _, _)| lane_tab.as_deref() == Some(tab.as_str()));
            match lane {
                Some((name, _, profile, record_role)) => (
                    tab.clone(),
                    name.clone(),
                    role(name, profile.as_deref(), record_role.as_deref()),
                ),
                None => match viewer_tabs.get(tab) {
                    Some((name, profile, record_role)) => (
                        tab.clone(),
                        name.clone(),
                        role(name, profile.as_deref(), record_role.as_deref()),
                    ),
                    None => (
                        tab.clone(),
                        labels.get(tab).cloned().unwrap_or_default(),
                        crate::tab_order::ROLE_UNMANAGED,
                    ),
                },
            }
        })
        .collect()
}

/// Each launch profile's `role` in `<registry>/profiles.json`, by profile
/// name; empty when the file is absent or unreadable, so ranking falls back
/// to profile names alone.
fn profile_roles(registry: &Path) -> BTreeMap<String, String> {
    let Ok(text) = fs::read_to_string(registry.join("profiles.json")) else {
        return BTreeMap::new();
    };
    let Ok(document) = serde_json::from_str::<Value>(&text) else {
        return BTreeMap::new();
    };
    document["profiles"]
        .as_object()
        .map(|profiles| {
            profiles
                .iter()
                .filter_map(|(name, profile)| {
                    Some((name.clone(), profile["role"].as_str()?.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Soft budget for the expensive part of one pane census (#1971
/// agentctl-tabs-scheduled-hygiene): the idle-shell proof and screen read
/// of unclaimed panes. Each Herdr call is already bounded on its own
/// (30 s control timeout, 5 s tab-socket timeout), but a census multiplies
/// those bounds by the pane count — the 77-record `tabs --dry-run` that
/// appeared to hang for over a minute was spending seconds per pane on
/// stable-identity observation inside the idle-shell proof, for claimed
/// panes whose verdict never uses it. Claimed panes (a record's pane, or
/// a tab a record names as its viewer) therefore skip the proof and the
/// screen read entirely, one `pane get` serves both cwd and foreground,
/// and unclaimed panes past the budget are reported unprobed —
/// `idle_shell: false`, composer holding text — so they are kept, never
/// closed on missing evidence.
const CENSUS_BUDGET: Duration = Duration::from_secs(60);

/// Build the pane facts of one census; the second return is true when
/// the census budget cut the expensive probes short. A pane counts as
/// claimed only on a verified claim ([`crate::tabs::claim_verified`],
/// workspace or terminal) or when its tab is a record's viewer tab.
fn census_pane_facts<A: ManagedApi + ?Sized>(
    client: &A,
    panes: &[crate::client::Pane],
    labels_by_tab: &BTreeMap<String, String>,
    agents: &[crate::tabs::AgentFacts],
) -> (Vec<crate::tabs::PaneFacts>, bool) {
    let started = Instant::now();
    let mut facts: Vec<crate::tabs::PaneFacts> = Vec::with_capacity(panes.len());
    let mut probe_order: Vec<usize> = Vec::new();
    for pane in panes {
        let info = client.pane_info(&pane.pane_id).ok();
        let mut fact = crate::tabs::PaneFacts {
            cwd: info
                .as_ref()
                .map(|info| info.cwd.clone())
                .unwrap_or_default(),
            pane_id: pane.pane_id.clone(),
            tab_id: pane.tab_id.clone(),
            workspace_id: pane.workspace_id.clone(),
            label: labels_by_tab.get(&pane.tab_id).cloned(),
            idle_shell: false,
            // An unreadable or unprobed screen counts as holding text
            // (fail-closed); a claimed pane's composer is never read.
            composer_text: false,
            foreground: info
                .as_ref()
                .map(|info| info.agent.clone().unwrap_or_else(|| info.status.clone()))
                .unwrap_or_else(|| "unknown".to_owned()),
            terminal_id: info.and_then(|info| info.terminal_id),
        };
        let claimed = agents.iter().any(|agent| {
            crate::tabs::claim_verified(&fact, agent)
                || crate::tabs::viewer_claim_verified(&fact, agent)
        });
        fact.composer_text = !claimed;
        if !claimed {
            probe_order.push(facts.len());
        }
        facts.push(fact);
    }
    // Rotate the probe start (#1971 nit): when the budget cuts a census
    // short, the unprobed tail is a different tail each pass instead of
    // the same panes starving forever.
    if !probe_order.is_empty() {
        let rotation = (unix_now() as usize) % probe_order.len();
        probe_order.rotate_left(rotation);
    }
    let mut truncated = false;
    for index in probe_order {
        if started.elapsed() > CENSUS_BUDGET {
            truncated = true;
            break;
        }
        let pane_id = facts[index].pane_id.clone();
        facts[index].idle_shell = client.pane_is_idle_shell(&pane_id).unwrap_or(false);
        facts[index].composer_text = match client
            .read_screen_with_runtime(&pane_id, &crate::agent::SystemRuntime::default())
        {
            Ok(screen) => crate::tabs::composer_holds_text(&screen),
            Err(_) => true,
        };
    }
    (facts, truncated)
}

/// The `agentctl tabs` listing (#1793): every tab of every managed
/// workspace with its owner record, foreground process, idle time, and
/// verdict. Read-only; with `dry_run` it also names the actions a hygiene
/// pass would take (grace judged from now, so an idle shell shows its grace
/// verdict rather than a close).
pub fn tabs_listing<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    socket: Option<&Path>,
) -> Result<Value> {
    let manager = crate::subagents::ManagedAgents::new(client, registry)
        .map_err(|error| fail(error.to_string()))?;
    let names = registered_names(registry).unwrap_or_default();
    let panes = client.panes().map_err(|error| fail(error.to_string()))?;
    let mut labels_by_tab: BTreeMap<String, String> = BTreeMap::new();
    let mut workspaces: Vec<&str> = panes.iter().map(|p| p.workspace_id.as_str()).collect();
    workspaces.sort_unstable();
    workspaces.dedup();
    for workspace in &workspaces {
        if let Ok(found) = client.tab_labels(workspace) {
            labels_by_tab.extend(found);
        }
    }
    let agent_facts: Vec<crate::tabs::AgentFacts> = names
        .iter()
        .filter_map(|name| {
            let status = manager.census_record(name).ok()?;
            Some(crate::tabs::AgentFacts {
                cwd: status["cwd"].as_str().unwrap_or_default().to_owned(),
                name: name.clone(),
                pane_id: status["pane_id"].as_str().map(str::to_owned),
                viewer_tab_id: status["viewer_tab_id"].as_str().map(str::to_owned),
                lifecycle: status["lifecycle"].as_str().unwrap_or_default().to_owned(),
                headless: status["mode"].as_str() == Some("headless"),
                viewer_is_tail: status["viewer_kind"].as_str() == Some("tail"),
                live_proven: false,
                workspace_id: status["workspace_id"].as_str().map(str::to_owned),
                terminal_id: status["terminal_id"].as_str().map(str::to_owned),
            })
        })
        .collect();
    let (pane_facts, census_truncated) =
        census_pane_facts(client, &panes, &labels_by_tab, &agent_facts);
    let order_tabs: Vec<(String, String)> = workspaces
        .iter()
        .filter_map(|workspace| {
            socket
                .and_then(|socket| crate::tab_order::list_tabs(socket, workspace).ok())
                .map(|tabs| {
                    tabs.into_iter()
                        .map(|tab| (workspace.to_string(), tab))
                        .collect::<Vec<_>>()
                })
        })
        .flatten()
        .collect();
    // The listing keeps no idle clock of its own, so it previews every
    // unclaimed idle shell as though its grace had already elapsed: the
    // verdicts and `would_do` name what a watch pass closes once each such
    // shell has stayed idle for `bare_shell_grace_seconds`. Without this an
    // empty clock and a hard-coded composer made `--dry-run` unable to name
    // any close at all.
    let grace = config.bare_shell_grace_seconds as f64;
    let assumed_since: BTreeMap<String, f64> = pane_facts
        .iter()
        .filter(|pane| pane.idle_shell && crate::tabs::claim_of(pane, &agent_facts).is_none())
        .map(|pane| (pane.pane_id.clone(), 0.0))
        .collect();
    // Same assumption for orphan tabs (#1918): previewed as long-seen.
    let live_tabs: std::collections::BTreeSet<&str> =
        pane_facts.iter().map(|pane| pane.tab_id.as_str()).collect();
    let assumed_tab_since: BTreeMap<String, f64> = order_tabs
        .iter()
        .map(|(_, tab)| tab.as_str())
        .filter(|tab| !live_tabs.contains(tab))
        .map(|tab| (tab.to_owned(), 0.0))
        .collect();
    let (rows, actions) = crate::tabs::plan(
        &pane_facts,
        &order_tabs,
        &agent_facts,
        &assumed_since,
        &assumed_tab_since,
        grace,
        grace,
        workspaces.first().copied(),
    );
    Ok(json!({
        "grace_assumed_elapsed": true,
        "census_truncated": census_truncated,
        "bare_shell_grace_seconds": config.bare_shell_grace_seconds,
        "tabs": rows.iter().map(|row| json!({
            "workspace_id": row.workspace_id, "tab_id": row.tab_id,
            "label": row.label, "owner": row.owner,
            "foreground": row.foreground, "idle_seconds": row.idle_seconds,
            "verdict": row.verdict,
        })).collect::<Vec<_>>(),
        "would_do": actions.iter().map(|action| format!("{action:?}")).collect::<Vec<_>>(),
    }))
}

/// The standalone hygiene state (#1971): the grace clocks a one-shot
/// `agentctl tabs --apply` (or the hygiene run at the end of `restore`)
/// keeps between invocations, in one fixed registry file — schema
/// `agentctl-tab-hygiene/v1`, `{bare_shell_since, orphan_tab_since,
/// closed_bare_shells}`. It holds no agent records and no record fields;
/// a watch daemon keeps the same clocks in its own health file and the
/// two never read each other's. An unreadable state reads as empty,
/// which only restarts the grace clocks: the fail-safe direction.
pub const TAB_HYGIENE_STATE_FILE: &str = "tab-hygiene.json";

/// Load the standalone hygiene state's grace clocks and closure log.
fn load_tab_hygiene_state(
    registry: &Path,
) -> (BTreeMap<String, f64>, BTreeMap<String, f64>, Vec<Value>) {
    let Some(state) = read_report(&registry.join(TAB_HYGIENE_STATE_FILE)) else {
        return (BTreeMap::new(), BTreeMap::new(), Vec::new());
    };
    (
        serde_json::from_value(state["bare_shell_since"].clone()).unwrap_or_default(),
        serde_json::from_value(state["orphan_tab_since"].clone()).unwrap_or_default(),
        state["closed_bare_shells"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    )
}

/// Longest a standalone grace clock may age before it is dropped
/// (#1971 cold review): about two timer passes. Herdr restores tabs
/// after a reboot as plain shells under the same pane ids, so a clock
/// from before the boot — or simply long stale — must never make the
/// first pass after it close those shells at once.
const TAB_HYGIENE_CLOCK_MAX_AGE: f64 = 600.0;

/// Run one hygiene pass with the standalone persisted grace clocks and
/// save them back (#1971): what `agentctl tabs --apply` and the run at
/// the end of `restore` both do, so a scheduled timer pass and a boot
/// restore share one grace history instead of each restarting it. The
/// whole pass runs under a registry flock
/// (`.tab-hygiene.lock`), so a restore-end pass, a timer pass and any
/// other standalone pass cannot interleave and overwrite each other's
/// state; a pass that cannot take the lock is skipped, never queued.
/// Grace clocks recorded under another boot id, or older than
/// [`TAB_HYGIENE_CLOCK_MAX_AGE`], are dropped before the pass. Viewer
/// creation is suppressed in these standalone passes until #1973
/// mu-headlesstab settles viewer ownership; ordering and closes are
/// unaffected.
///
/// # Errors
///
/// The lock or the updated state cannot be written.
pub fn tabs_apply<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    host: &dyn Host,
    socket: Option<&Path>,
) -> Result<Value> {
    let lock_path = registry.join(".tab-hygiene.lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| fail(format!("cannot open the tab-hygiene lock: {error}")))?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(json!({
            "applied": false,
            "skipped": "another tab-hygiene pass holds the lock",
            "state": registry.join(TAB_HYGIENE_STATE_FILE),
        }));
    }
    let (mut bare_since, mut tab_since, mut closed) = load_tab_hygiene_state(registry);
    // Drop stale clocks (#1971 cold review): another boot, or a
    // checked_at more than about two passes ago.
    let previous = read_report(&registry.join(TAB_HYGIENE_STATE_FILE));
    let boot_id = host.boot_id().ok();
    let clocks_fresh = previous.as_ref().is_some_and(|state| {
        state["boot_id"].as_str() == boot_id.as_deref()
            && state["checked_at"]
                .as_f64()
                .is_some_and(|at| host.now() - at <= TAB_HYGIENE_CLOCK_MAX_AGE && at <= host.now())
    });
    let clocks_reset = !clocks_fresh && (!bare_since.is_empty() || !tab_since.is_empty());
    if !clocks_fresh {
        bare_since.clear();
        tab_since.clear();
    }
    let findings = hygiene_pass(
        client,
        registry,
        config,
        host,
        &mut bare_since,
        &mut tab_since,
        &mut closed,
        socket,
        true,
    );
    let state = json!({
        "schema": "agentctl-tab-hygiene/v1",
        "checked_at": host.now(),
        "boot_id": boot_id,
        "bare_shell_since": bare_since,
        "orphan_tab_since": tab_since,
        "closed_bare_shells": closed,
    });
    write_report(&registry.join(TAB_HYGIENE_STATE_FILE), &state)?;
    Ok(json!({
        "applied": true,
        "clocks_reset": clocks_reset,
        "findings": findings,
        "closed_bare_shells": state["closed_bare_shells"],
        "state": registry.join(TAB_HYGIENE_STATE_FILE),
    }))
}

/// One hygiene pass: canonical tab order in managed workspaces, and
/// closing unclaimed bare shells idle past the grace period. Returns
/// findings; every close is logged with pane, tab, cwd, and why.
///
/// Canonical order (#1004 liaison-first-tab-order, see
/// [`crate::tab_order::role_for`]): a chat bridge's target first, then an
/// explicitly marked liaison, then the designated coordinator, then other
/// coordinators, then lanes by name, then unmanaged tabs. Bridge targets
/// come from the bridges registered for discovery
/// ([`crate::chat_lookup::candidates`]); a registry with none and no
/// liaison mark keeps the designated coordinator first.
/// Whether a fresh herdr pane listing still shows no pane in `tab_id`
/// (#1918 close-time re-check): a pane that appeared after the census
/// vetoes the orphan close.
fn orphan_still_empty(panes_now: &[crate::client::Pane], tab_id: &str) -> bool {
    !panes_now.iter().any(|pane| pane.tab_id == tab_id)
}

#[allow(clippy::too_many_arguments)]
fn hygiene_pass<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    host: &dyn Host,
    bare_since: &mut BTreeMap<String, f64>,
    tab_since: &mut BTreeMap<String, f64>,
    closed: &mut Vec<Value>,
    socket: Option<&Path>,
    suppress_viewers: bool,
) -> Vec<Value> {
    let findings = Vec::new();
    let manager = match crate::subagents::ManagedAgents::new(client, registry) {
        Ok(manager) => manager,
        Err(error) => {
            return vec![finding(
                "hygiene",
                "registry",
                false,
                error.to_string(),
                Value::Null,
            )];
        }
    };
    let names = registered_names(registry).unwrap_or_default();
    let bridge_targets: std::collections::BTreeSet<String> =
        crate::chat_lookup::candidates(registry)
            .into_iter()
            .map(|candidate| candidate.agent)
            .collect();
    let roles = profile_roles(registry);
    let mut claimed: BTreeMap<String, ClaimedTab> = BTreeMap::new();
    let mut viewer_tabs: BTreeMap<String, ViewerTab> = BTreeMap::new();
    for name in &names {
        if let Ok(status) = manager.census_record(name) {
            if let Some(pane) = status["pane_id"].as_str() {
                claimed.insert(
                    pane.to_owned(),
                    (
                        name.clone(),
                        status["tab_id"].as_str().map(str::to_owned),
                        status["profile"].as_str().map(str::to_owned),
                        status["role"].as_str().map(str::to_owned),
                    ),
                );
            }
            // A headless record may name a viewer tab (#1225 Phase 1): that
            // tab ranks with the agent, and no pane id is claimed or changed.
            if let Some(viewer) = status["viewer_tab_id"].as_str() {
                viewer_tabs.insert(
                    viewer.to_owned(),
                    (
                        name.clone(),
                        status["profile"].as_str().map(str::to_owned),
                        status["role"].as_str().map(str::to_owned),
                    ),
                );
            }
        }
    }
    let Ok(panes) = client.panes() else {
        return findings;
    };
    // #1918 (a): snapshot every workspace's tab order BEFORE the pane
    // census below (per-pane info, idle state and screen; over 90 s with
    // ~110 panes). The planner judges orphans against this snapshot only,
    // so a tab created during the census is not in it and cannot be
    // called an orphan this pass.
    let order_snapshot: Vec<(String, String)> = {
        let mut workspaces: Vec<&str> = panes.iter().map(|p| p.workspace_id.as_str()).collect();
        workspaces.sort_unstable();
        workspaces.dedup();
        workspaces
            .iter()
            .filter_map(|workspace| {
                socket
                    .and_then(|socket| crate::tab_order::list_tabs(socket, workspace).ok())
                    .map(|tabs| {
                        tabs.into_iter()
                            .map(|tab| (workspace.to_string(), tab))
                            .collect::<Vec<_>>()
                    })
            })
            .flatten()
            .collect()
    };
    // #1793: a workspace is also managed when a pane's tab label names a
    // registered agent (a duplicate tab in a second workspace), so its bare
    // shells and order are cultivated instead of invisible.
    let labels_by_tab: BTreeMap<String, String> = {
        let mut labels = BTreeMap::new();
        let mut workspaces: Vec<&str> = panes.iter().map(|p| p.workspace_id.as_str()).collect();
        workspaces.sort_unstable();
        workspaces.dedup();
        for workspace in workspaces {
            if let Ok(found) = client.tab_labels(workspace) {
                labels.extend(found);
            }
        }
        labels
    };
    let agent_facts_early: Vec<crate::tabs::AgentFacts> = names
        .iter()
        .filter_map(|name| {
            let status = manager.census_record(name).ok()?;
            Some(crate::tabs::AgentFacts {
                name: name.clone(),
                pane_id: status["pane_id"].as_str().map(str::to_owned),
                viewer_tab_id: status["viewer_tab_id"].as_str().map(str::to_owned),
                lifecycle: status["lifecycle"].as_str().unwrap_or_default().to_owned(),
                headless: status["mode"].as_str() == Some("headless"),
                viewer_is_tail: status["viewer_kind"].as_str() == Some("tail"),
                cwd: status["cwd"].as_str().unwrap_or_default().to_owned(),
                live_proven: false,
                workspace_id: status["workspace_id"].as_str().map(str::to_owned),
                terminal_id: status["terminal_id"].as_str().map(str::to_owned),
            })
        })
        .collect();
    let (pane_facts, census_truncated) =
        census_pane_facts(client, &panes, &labels_by_tab, &agent_facts_early);
    // Ordering claims count only verified claims (#1971 cold review):
    // a stale record naming another team's pane must not rename or
    // reorder that team's tabs.
    claimed.retain(|pane_id, _| {
        pane_facts.iter().any(|fact| {
            &fact.pane_id == pane_id && crate::tabs::claim_of(fact, &agent_facts_early).is_some()
        })
    });
    let mut findings = findings;
    if census_truncated {
        findings.push(finding(
            "hygiene",
            "census",
            false,
            "pane census budget reached; unprobed panes were kept, not closed",
            Value::Null,
        ));
    }
    let managed_workspaces = crate::tabs::managed_workspaces(&pane_facts, &agent_facts_early);
    // Canonical tab order, through the guarded Herdr socket when the
    // caller's Herdr binary is the one serving it (#1971 nit).
    if let Some(socket) = socket {
        for workspace in &managed_workspaces {
            let Ok(order) = crate::tab_order::list_tabs(socket, workspace) else {
                continue;
            };
            let labels = client.tab_labels(workspace).unwrap_or_default();
            let entries = ranked_entries(
                &order,
                &claimed,
                &viewer_tabs,
                &labels,
                &roles,
                &bridge_targets,
                config.coordinator.as_deref(),
            );
            let desired = crate::tab_order::canonical_tabs(&entries);
            let mut current = order;
            for (index, tab) in desired.iter().enumerate() {
                if current.get(index) != Some(tab)
                    && crate::tab_order::move_tab(socket, tab, index).is_ok()
                {
                    current.remove(
                        current
                            .iter()
                            .position(|candidate| candidate == tab)
                            .unwrap(),
                    );
                    current.insert(index, tab.clone());
                }
            }
        }
    }
    // Cultivation (#1793): the pure planner decides; this pass only executes.
    let now = host.now();
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    for fact in &pane_facts {
        if fact.idle_shell && !claimed.contains_key(&fact.pane_id) {
            seen.insert(fact.pane_id.clone());
            bare_since.entry(fact.pane_id.clone()).or_insert(now);
        }
    }
    bare_since.retain(|pane, _| seen.contains(pane));
    let agent_facts: Vec<crate::tabs::AgentFacts> = names
        .iter()
        .filter_map(|name| {
            let status = manager.census_record(name).ok()?;
            let identity = ["harness_identity", "custom_process_identity"]
                .iter()
                .find_map(|key| {
                    serde_json::from_value::<crate::client::CustomProcessIdentity>(
                        status[*key].clone(),
                    )
                    .ok()
                });
            // Promotion proof (#1793 check (2)): the recorded pane is live,
            // its foreground is not a bare shell, Herdr recognises an agent
            // in it, and the recorded process identity is alive in the
            // kernel (pid + start time + executable). A label never promotes.
            // Only a `launch_failed` record can be promoted, so the proof
            // (an idle-shell probe per pane, #1971) is paid only for those;
            // every other record reads `live_proven: false`, which the
            // planner uses for nothing else.
            let live_proven = status["lifecycle"].as_str() == Some("launch_failed")
                && status["pane_id"].as_str().is_some_and(|pane| {
                    panes.iter().any(|candidate| candidate.pane_id == pane)
                        && !client.pane_is_idle_shell(pane).unwrap_or(true)
                        && client
                            .pane_info(pane)
                            .is_ok_and(|info| info.agent.is_some())
                })
                && identity.as_ref().is_some_and(|identity| {
                    client.process_liveness(identity) == crate::client::ProcessLiveness::Alive
                });
            Some(crate::tabs::AgentFacts {
                cwd: status["cwd"].as_str().unwrap_or_default().to_owned(),
                name: name.clone(),
                pane_id: status["pane_id"].as_str().map(str::to_owned),
                viewer_tab_id: status["viewer_tab_id"].as_str().map(str::to_owned),
                lifecycle: status["lifecycle"].as_str().unwrap_or_default().to_owned(),
                headless: status["mode"].as_str() == Some("headless"),
                viewer_is_tail: status["viewer_kind"].as_str() == Some("tail"),
                live_proven,
                workspace_id: status["workspace_id"].as_str().map(str::to_owned),
                terminal_id: status["terminal_id"].as_str().map(str::to_owned),
            })
        })
        .collect();
    // #1918: orphans are judged against the pre-census snapshot only,
    // restricted to managed workspaces (never a fresh post-census read).
    let order_tabs: Vec<(String, String)> = order_snapshot
        .into_iter()
        .filter(|(workspace, _)| managed_workspaces.contains(workspace))
        .collect();
    // #1918 (b): stamp first-seen for each current orphan; forget tabs
    // that gained a pane or left the order, so the clock restarts if a
    // tab id is ever reused for a genuinely new empty tab.
    {
        let live: std::collections::BTreeSet<&str> =
            pane_facts.iter().map(|fact| fact.tab_id.as_str()).collect();
        let orphans: std::collections::BTreeSet<String> = order_tabs
            .iter()
            .map(|(_, tab)| tab.as_str())
            .filter(|tab| !live.contains(tab))
            .map(str::to_owned)
            .collect();
        for tab in &orphans {
            tab_since.entry(tab.clone()).or_insert(now);
        }
        tab_since.retain(|tab, _| orphans.contains(tab));
    }
    let viewer_workspace = config
        .coordinator
        .as_deref()
        .and_then(|name| agent_facts.iter().find(|agent| agent.name == name))
        .and_then(|agent| {
            agent
                .viewer_tab_id
                .as_deref()
                .and_then(|tab| tab.split(':').next().map(str::to_owned))
                .or_else(|| {
                    agent.pane_id.as_deref().and_then(|pane| {
                        panes
                            .iter()
                            .find(|candidate| candidate.pane_id == pane)
                            .map(|candidate| candidate.workspace_id.clone())
                    })
                })
        })
        .or_else(|| managed_workspaces.iter().next().cloned());
    let (_, actions) = crate::tabs::plan(
        &pane_facts,
        &order_tabs,
        &agent_facts,
        bare_since,
        tab_since,
        now,
        config.bare_shell_grace_seconds as f64,
        viewer_workspace.as_deref(),
    );
    for action in actions {
        match action {
            crate::tabs::TabAction::ClosePane {
                pane_id,
                tab_id,
                why,
            } => {
                if !config.close_bare_shells {
                    continue;
                }
                let info = client.pane_info(&pane_id).ok();
                if client.close_pane(&pane_id).is_ok() {
                    closed.push(json!({
                        "pane_id": pane_id, "tab_id": tab_id,
                        "cwd": info.as_ref().map(|info| info.cwd.clone()),
                        "why": why, "closed_at": now,
                    }));
                    bare_since.remove(&pane_id);
                }
            }
            crate::tabs::TabAction::CloseTab { tab_id, why } => {
                // #1918 (c): an orphan close re-lists the tab's panes from
                // herdr at close time; any pane that appeared since the
                // census vetoes the close. A failed re-list also vetoes
                // (fail-closed). Stale-viewer closes keep their own path:
                // their tab holds a live pane by design.
                if suppress_viewers && why.starts_with("stale tail viewer") {
                    continue;
                }
                if !config.close_bare_shells {
                    continue;
                }
                if why == "orphan tab, no live pane" {
                    let still_empty = client
                        .panes()
                        .is_ok_and(|panes_now| orphan_still_empty(&panes_now, &tab_id));
                    if !still_empty {
                        continue;
                    }
                }
                if client.close_tab(&tab_id).is_ok() {
                    closed.push(json!({"tab_id": tab_id, "why": why, "closed_at": now}));
                    tab_since.remove(&tab_id);
                }
            }
            crate::tabs::TabAction::RenameTab { tab_id, to } => {
                let _ = client.rename_tab(&tab_id, &to);
            }
            crate::tabs::TabAction::Promote { agent } => {
                let _ = manager.promote_launch_failed(&agent);
            }
            crate::tabs::TabAction::CreateViewer {
                agent,
                workspace_id,
            } => {
                // Suppressed in standalone passes (#1971 cold review)
                // until #1973 mu-headlesstab settles viewer ownership;
                // that fix has landed as ensure_headless_viewer_with_close,
                // so the watch path uses it and standalone passes keep
                // suppressing creation until coordb lifts the hold.
                if !suppress_viewers {
                    let _ = manager.ensure_headless_viewer_with_close(
                        &agent,
                        &workspace_id,
                        config.close_bare_shells,
                    );
                }
            }
        }
    }
    findings
}

/// Watch the team's boot health until terminated, writing `health.json`.
pub fn watch<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    host: &dyn Host,
    config: &RestoreConfig,
    options: &WatchOptions,
    socket: Option<&Path>,
) -> Result<Value> {
    // Heartbeat from daemon start (#1914), before any state is read, so a block
    // in startup or in the first pass still shows an advancing heartbeat.
    let progress = PassProgress::new(watch_progress_path(&options.state_dir));
    let path = health_path(options);
    let mut alerted: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["alerted"].clone()).ok())
        .unwrap_or_default();
    let mut bare_since: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["bare_shell_since"].clone()).ok())
        .unwrap_or_default();
    // #1918: orphan-tab first-seen clock, persisted like bare_shell_since
    // so the grace survives a watch restart. Absent in older health files.
    let mut tab_since: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["orphan_tab_since"].clone()).ok())
        .unwrap_or_default();
    let mut timer_repaired_at: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["timer_repaired_at"].clone()).ok())
        .unwrap_or_default();
    // Timers already reported as stopped by the operator in this stop episode
    // (#1277): persisted so a watch restart does not report the same stop again.
    let mut timer_stopped_noted: std::collections::BTreeSet<String> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["timer_stopped_noted"].clone()).ok())
        .unwrap_or_default();
    // Units whose cooldown finding has already gone to stderr this episode (#961): a
    // cooldown is logged once when it starts, not on every pass it spans.
    let mut cooldown_noted: std::collections::BTreeSet<String> = Default::default();
    // #2018: boots whose missing restore report this process already
    // reported; in memory only, never persisted to health.json.
    let mut restore_missing_noted: std::collections::BTreeSet<String> = Default::default();
    let mut closed_bare_shells: Vec<Value> = Vec::new();
    let mut pass: u64 = 0;
    loop {
        progress.begin(host.now());
        // Every wrkslots command loads the whole slot registry, so slot images are checked on
        // the first pass and then every tenth.
        let pass_options = WatchOptions {
            wrkslots: options.wrkslots.clone().filter(|_| pass % 10 == 0),
            ..options.clone()
        };
        pass += 1;
        let mut findings = watch_once(
            client,
            registry,
            host,
            config,
            &pass_options,
            &mut timer_repaired_at,
            &mut timer_stopped_noted,
        );
        apply_restore_missing_once(&mut findings, &mut restore_missing_noted);
        // Workspace hygiene and canonical tab order:
        // close unclaimed bare shells idle past the grace period, and keep
        // managed workspaces' tabs in canonical order. Best-effort; a
        // failure is a finding, never a watch failure.
        if !pass_options.check_only && config.duties.tab_hygiene {
            findings.extend(hygiene_pass(
                client,
                registry,
                config,
                host,
                &mut bare_since,
                &mut tab_since,
                &mut closed_bare_shells,
                socket,
                false,
            ));
        }
        ensure_skipped(&mut findings, &config.duties);
        let notice = if !config.duties.notify {
            json!({"sent": false, "reason": "notify duty off (skipped-by-config)"})
        } else if options.check_only {
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
        for row in findings
            .iter()
            .filter(|row| row["check"] == "timer" && row["cooldown"] != true)
        {
            cooldown_noted.remove(row["subject"].as_str().unwrap_or(""));
        }
        for row in findings.iter().filter(|row| row["ok"] == false) {
            if row["cooldown"] == true
                && !cooldown_noted.insert(row["subject"].as_str().unwrap_or("").to_owned())
            {
                continue;
            }
            // The first stopped-by-operator row is the episode's one report (#1277);
            // noting it here keeps its later cooldown rows off stderr too.
            if row["operator_stopped"] == true {
                cooldown_noted.insert(row["subject"].as_str().unwrap_or("").to_owned());
            }
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
            "duties": config.duties.as_json(),
            "ok": findings.iter().filter(|row| row.get("skipped").is_none()).all(|row| row["ok"] == true),
            "warnings": warnings,
            "findings": findings,
            "notice": notice,
            "alerted": alerted,
            "timer_repaired_at": timer_repaired_at,
            "timer_stopped_noted": timer_stopped_noted,
            "bare_shell_since": bare_since,
            "orphan_tab_since": tab_since,
            "closed_bare_shells": closed_bare_shells,
        });
        if let Some(path) = path.as_deref() {
            if let Err(error) = write_report(path, &health) {
                eprintln!("agentctl watch: {error}");
            }
        }
        if options.once {
            return Ok(health);
        }
        if !host.terminated() {
            host.sleep(options.interval);
        }
        if host.terminated() {
            eprintln!("agentctl watch: stopping: termination requested");
            return Ok(health);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn pass_progress_heartbeats_from_daemon_start_before_the_first_pass() {
        // #1914: the file exists in `starting` phase from daemon start, and its
        // heartbeat advances while startup or a lock wait blocks the first pass.
        let directory = std::env::temp_dir().join(format!(
            "agentctl-pass-progress-start-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join(PASS_PROGRESS_FILE);
        let progress = PassProgress::new(Some(path.clone()));
        let read = || -> Value { serde_json::from_slice(&fs::read(&path).unwrap()).unwrap() };
        let first = read();
        assert_eq!(first["schema"], PASS_PROGRESS_SCHEMA);
        assert_eq!(first["phase"], "starting");
        assert!(first["daemon_started_at"].as_f64().unwrap_or(0.0) > 0.0);
        assert_eq!(first["pass_started_at"], first["daemon_started_at"]);
        assert_eq!(first["heartbeat_at"], first["daemon_started_at"]);
        let began_at = first["heartbeat_at"].as_f64().unwrap();
        // Condition-based wait: the heartbeat thread must advance the stamp
        // before any pass begins.
        let mut advanced = false;
        for _ in 0..40 {
            if read()["heartbeat_at"].as_f64().unwrap_or(0.0) > began_at {
                advanced = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(advanced, "heartbeat_at never advanced: {}", read());
        assert_eq!(read()["phase"], "starting");
        let pass_at = unix_now();
        progress.begin(pass_at);
        let in_pass = read();
        assert_eq!(in_pass["phase"], "pass");
        assert_eq!(in_pass["pass_started_at"], pass_at);
        assert_eq!(in_pass["daemon_started_at"], first["daemon_started_at"]);
        drop(progress);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_tab_whose_relist_shows_a_pane_is_not_closed() {
        // #1918 (c): the census saw no pane in w1:t9, so the planner
        // (past grace) emitted an orphan close; by close time a lane's
        // pane has appeared in the tab. The re-list vetoes the close.
        let appeared = vec![crate::client::Pane {
            pane_id: "w1:p9".into(),
            tab_id: "w1:t9".into(),
            workspace_id: "w1".into(),
        }];
        assert!(!orphan_still_empty(&appeared, "w1:t9"));
        // Still genuinely empty: no pane, or only other tabs' panes.
        assert!(orphan_still_empty(&[], "w1:t9"));
        let other = vec![crate::client::Pane {
            pane_id: "w1:p1".into(),
            tab_id: "w1:t1".into(),
            workspace_id: "w1".into(),
        }];
        assert!(orphan_still_empty(&other, "w1:t9"));
    }

    /// Counting fake for the census tests (#1971): every Herdr-shaped
    /// call the census makes is recorded, so a test can prove which
    /// panes paid for the expensive idle-shell proof and screen read.
    struct CensusFake {
        panes: Vec<crate::client::Pane>,
        pane_info_calls: std::sync::Mutex<Vec<String>>,
        idle_calls: std::sync::Mutex<Vec<String>>,
        read_calls: std::sync::Mutex<Vec<String>>,
    }

    impl CensusFake {
        fn new(panes: Vec<crate::client::Pane>) -> Self {
            Self {
                panes,
                pane_info_calls: std::sync::Mutex::new(Vec::new()),
                idle_calls: std::sync::Mutex::new(Vec::new()),
                read_calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl crate::agent::AgentApi for CensusFake {
        fn panes(&self) -> crate::error::Result<Vec<crate::client::Pane>> {
            Ok(self.panes.clone())
        }
        fn pane_info(&self, pane_id: &str) -> crate::error::Result<crate::client::AgentPaneInfo> {
            self.pane_info_calls
                .lock()
                .unwrap()
                .push(pane_id.to_owned());
            let pane = self
                .panes
                .iter()
                .find(|pane| pane.pane_id == pane_id)
                .unwrap();
            Ok(crate::client::AgentPaneInfo {
                pane_id: pane_id.to_owned(),
                workspace_id: pane.workspace_id.clone(),
                cwd: "/work/slot".to_owned(),
                agent: Some("muse".to_owned()),
                status: "idle".to_owned(),
                session_agent: None,
                session_value: None,
                scroll: None,
                terminal_id: None,
                tab_id: Some(pane.tab_id.clone()),
                reported_status: None,
            })
        }
        fn workspace_label(&self, _workspace: &str) -> crate::error::Result<String> {
            Ok("w".to_owned())
        }
        fn run(&self, _pane: &str, _text: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn wait_agent_status(
            &self,
            _pane: &str,
            _status: &str,
            _timeout_ms: u64,
        ) -> crate::error::Result<()> {
            Ok(())
        }
        fn read(
            &self,
            pane_id: &str,
            _source: &str,
            _lines: Option<usize>,
        ) -> crate::error::Result<String> {
            self.read_calls.lock().unwrap().push(pane_id.to_owned());
            Ok("$ ".to_owned())
        }
    }

    impl crate::subagents::ManagedApi for CensusFake {
        fn workspace_id_for_label(&self, _label: &str) -> crate::error::Result<Option<String>> {
            Ok(None)
        }
        fn create_workspace(
            &self,
            _label: &str,
            _cwd: &str,
            _environment: &[String],
        ) -> crate::error::Result<(String, String, String)> {
            Err(crate::error::AdapterError::unavailable("fake"))
        }
        fn create_tab(
            &self,
            _workspace: &str,
            _label: &str,
            _cwd: &str,
            _environment: &[String],
        ) -> crate::error::Result<String> {
            Err(crate::error::AdapterError::unavailable("fake"))
        }
        fn create_tab_with_pane(
            &self,
            _workspace: &str,
            _label: &str,
            _cwd: &str,
            _environment: &[String],
        ) -> crate::error::Result<(String, String)> {
            Err(crate::error::AdapterError::unavailable("fake"))
        }
        fn close_pane(&self, _pane: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn focus_pane(&self, _pane: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn rename_tab(&self, _tab: &str, _label: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn start_agent(
            &self,
            _name: &str,
            _harness: &str,
            _pane: &str,
            _args: &[String],
            _timeout: Duration,
        ) -> crate::error::Result<()> {
            Ok(())
        }
        fn agent_pane(&self, _name: &str) -> crate::error::Result<String> {
            Err(crate::error::AdapterError::unavailable("fake"))
        }
        fn report_agent_session(
            &self,
            _name: &str,
            _pane: &str,
            _kind: &str,
            _session: &str,
        ) -> crate::error::Result<()> {
            Ok(())
        }
        fn send_keys(&self, _pane: &str, _key: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn close_tab(&self, _tab: &str) -> crate::error::Result<()> {
            Ok(())
        }
        fn pane_is_idle_shell(&self, pane: &str) -> crate::error::Result<bool> {
            self.idle_calls.lock().unwrap().push(pane.to_owned());
            Ok(true)
        }
    }

    #[test]
    fn census_probes_only_unclaimed_panes_and_gets_each_pane_once() {
        // #1971: a claimed pane's verdict never uses the idle-shell
        // proof or its screen, so the census must not pay the
        // stable-identity observation (seconds on a dead or exec-ing
        // shell) for it; one `pane get` serves cwd and foreground.
        let pane = |id: &str, tab: &str| crate::client::Pane {
            pane_id: id.to_owned(),
            tab_id: tab.to_owned(),
            workspace_id: "w1".to_owned(),
        };
        let fake = CensusFake::new(vec![
            pane("w1:p1", "w1:t1"), // claimed by a record's pane
            pane("w1:p2", "w1:t2"), // in a record's viewer tab
            pane("w1:p3", "w1:t3"), // unclaimed: probed
        ]);
        let agents = vec![crate::tabs::AgentFacts {
            name: "agent-a".to_owned(),
            pane_id: Some("w1:p1".to_owned()),
            viewer_tab_id: Some("w1:t2".to_owned()),
            lifecycle: "running".to_owned(),
            headless: false,
            viewer_is_tail: false,
            cwd: "/work/slot".to_owned(),
            live_proven: false,
            workspace_id: Some("w1".to_owned()),
            terminal_id: None,
        }];
        let (facts, truncated) =
            census_pane_facts(&fake, &fake.panes.clone(), &BTreeMap::new(), &agents);
        assert!(!truncated);
        assert_eq!(
            fake.pane_info_calls.lock().unwrap().len(),
            3,
            "one pane get per pane"
        );
        assert_eq!(
            *fake.idle_calls.lock().unwrap(),
            vec!["w1:p3".to_owned()],
            "idle proof only for the unclaimed pane"
        );
        assert_eq!(
            *fake.read_calls.lock().unwrap(),
            vec!["w1:p3".to_owned()],
            "screen read only for the unclaimed pane"
        );
        let unclaimed = facts.iter().find(|fact| fact.pane_id == "w1:p3").unwrap();
        assert!(unclaimed.idle_shell && !unclaimed.composer_text);
        for fact in facts.iter().filter(|fact| fact.pane_id != "w1:p3") {
            assert!(!fact.idle_shell, "claimed panes are never closable here");
            assert_eq!(fact.foreground, "muse");
            assert_eq!(fact.cwd, "/work/slot");
        }
    }

    #[test]
    fn tabs_apply_persists_and_reloads_its_grace_state() {
        // #1971: a one-shot pass writes tab-hygiene.json and the next
        // pass reads the same clocks back, so a timer-driven pass
        // continues a grace a restore pass started.
        let directory = std::env::temp_dir().join(format!(
            "agentctl-tab-hygiene-state-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let registry = directory.join("registry");
        fs::create_dir_all(&registry).unwrap();
        let fake = CensusFake::new(Vec::new());
        let config = RestoreConfig::load(&registry).unwrap();
        let applied = tabs_apply(&fake, &registry, &config, &SystemHost, None).unwrap();
        assert_eq!(applied["applied"], true);
        let state = read_report(&registry.join(TAB_HYGIENE_STATE_FILE)).unwrap();
        assert_eq!(state["schema"], "agentctl-tab-hygiene/v1");
        // A clock stamped by hand survives a reload round-trip.
        let mut state = state;
        state["bare_shell_since"] = json!({"w1:p9": 42.0});
        write_report(&registry.join(TAB_HYGIENE_STATE_FILE), &state).unwrap();
        let (bare, _tabs, _closed) = load_tab_hygiene_state(&registry);
        assert_eq!(bare.get("w1:p9"), Some(&42.0));
        // A clock from another boot is dropped before the next pass
        // (#1971 cold review): Herdr restores the same pane ids after a
        // reboot, so a pre-boot clock must not close them at once.
        state["boot_id"] = json!("some-earlier-boot");
        write_report(&registry.join(TAB_HYGIENE_STATE_FILE), &state).unwrap();
        let applied = tabs_apply(&fake, &registry, &config, &SystemHost, None).unwrap();
        assert_eq!(applied["clocks_reset"], true);
        let state = read_report(&registry.join(TAB_HYGIENE_STATE_FILE)).unwrap();
        assert_eq!(state["bare_shell_since"], json!({}));
        assert!(state["boot_id"].is_string());
        // A pass that cannot take the flock is skipped, never queued.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(registry.join(".tab-hygiene.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        let applied = tabs_apply(&fake, &registry, &config, &SystemHost, None).unwrap();
        assert_eq!(applied["applied"], false);
        drop(lock);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn pass_progress_records_a_pass_start_and_keeps_its_heartbeat_advancing() {
        // #1900: the file services health reads while a long first pass runs.
        let directory = std::env::temp_dir().join(format!(
            "agentctl-pass-progress-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = directory.join(PASS_PROGRESS_FILE);
        let progress = PassProgress::new(Some(path.clone()));
        let began = unix_now();
        progress.begin(began);
        let read = || -> Value { serde_json::from_slice(&fs::read(&path).unwrap()).unwrap() };
        let first = read();
        assert_eq!(first["schema"], PASS_PROGRESS_SCHEMA);
        assert_eq!(first["pass_started_at"], began);
        assert_eq!(first["heartbeat_at"], began);
        // Condition-based wait: the heartbeat thread must advance the stamp.
        let mut advanced = false;
        for _ in 0..40 {
            if read()["heartbeat_at"].as_f64().unwrap_or(0.0) > began {
                advanced = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        assert!(advanced, "heartbeat_at never advanced: {}", read());
        drop(progress);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn initial_pid_namespace_reads_nspid_levels() {
        assert!(initial_pid_namespace_status("Pid:\t42\nNSpid:\t42\n"));
        assert!(!initial_pid_namespace_status("Pid:\t7\nNSpid:\t4242\t7\n"));
        assert!(!initial_pid_namespace_status("Pid:\t7\n"));
    }

    #[test]
    fn epoch_matching_requires_the_same_boot_and_manager() {
        // #1405: replay is per epoch, not per boot.
        let epoch = RestoreEpoch {
            boot_id: "boot-a".to_owned(),
            user_manager: Some("invocation:m1".to_owned()),
        };
        let recorded = json!({"boot_id": "boot-a", "epoch": epoch.as_json(), "complete": true});
        assert!(epoch.matches_report(&recorded));
        let restarted = RestoreEpoch {
            user_manager: Some("invocation:m2".to_owned()),
            ..epoch.clone()
        };
        assert!(!restarted.matches_report(&recorded));
        let rebooted = RestoreEpoch {
            boot_id: "boot-b".to_owned(),
            ..epoch.clone()
        };
        assert!(!rebooted.matches_report(&recorded));
        // A pre-epoch report matches only an epoch whose manager is also
        // unknown; a manager-less epoch still matches its own reports.
        let legacy = json!({"boot_id": "boot-a", "complete": true});
        assert!(!epoch.matches_report(&legacy));
        let managerless = RestoreEpoch {
            boot_id: "boot-a".to_owned(),
            user_manager: None,
        };
        assert!(managerless.matches_report(&legacy));
        assert!(managerless.matches_report(&json!({
            "boot_id": "boot-a", "epoch": managerless.as_json(), "complete": true
        })));
    }

    #[test]
    fn boot_report_read_prefers_the_current_location_and_falls_back_to_legacy() {
        // #975/#1359: a same-boot report written by an older build at a
        // legacy location still counts, but the current location wins.
        let directory =
            std::env::temp_dir().join(format!("restore-reports-{}", std::process::id()));
        let registry = directory.join("registry");
        fs::create_dir_all(registry.join("restore-reports")).unwrap();
        let config = RestoreConfig::load(&registry).unwrap();
        let boot = "11111111-2222-3333-4444-555555555555";
        assert!(read_boot_report(&registry, &config, boot).is_none());
        let legacy = registry
            .join("restore-reports")
            .join(format!("{boot}.json"));
        fs::write(&legacy, "{\"complete\": true, \"where\": \"legacy\"}").unwrap();
        let (path, report) = read_boot_report(&registry, &config, boot).unwrap();
        assert_eq!(path, legacy);
        assert_eq!(report["where"], "legacy");
        let current = reports_directory(&registry);
        fs::create_dir_all(&current).unwrap();
        let primary = current.join(format!("{boot}.json"));
        fs::write(&primary, "{\"complete\": false, \"where\": \"primary\"}").unwrap();
        let (path, report) = read_boot_report(&registry, &config, boot).unwrap();
        assert_eq!(path, primary);
        assert_eq!(report["where"], "primary");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn completed_boot_report_finds_legacy_only_and_is_never_shadowed() {
        // The watch restore check and restore's replay share this pick: a
        // completed report left at either legacy location counts, and an
        // incomplete or earlier-epoch report at the current location
        // never shadows it.
        let directory =
            std::env::temp_dir().join(format!("restore-completed-{}", std::process::id()));
        let registry = directory.join("registry");
        fs::create_dir_all(&registry).unwrap();
        let config = RestoreConfig::load(&registry).unwrap();
        let boot = "11111111-2222-3333-4444-555555555555";
        let epoch = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: Some("invocation:m1".to_owned()),
        };
        let complete = format!(
            "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": true}}",
            epoch.as_json()
        );
        let file = format!("{boot}.json");
        // Only under agents/restore-reports/: found.
        let agents = registry.join("agents/restore-reports");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join(&file), &complete).unwrap();
        let (path, _) = completed_boot_report(&registry, &config, &epoch, None).unwrap();
        assert_eq!(path, agents.join(&file));
        // A partial report at the current location does not shadow it.
        let primary = reports_directory(&registry);
        fs::create_dir_all(&primary).unwrap();
        fs::write(
            primary.join(&file),
            format!(
                "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": false}}",
                epoch.as_json()
            ),
        )
        .unwrap();
        let (path, _) = completed_boot_report(&registry, &config, &epoch, None).unwrap();
        assert_eq!(path, agents.join(&file));
        // Nor does a completed report from an earlier manager epoch: the
        // current epoch's report under restore-reports/ is the pick.
        let earlier = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: Some("invocation:m0".to_owned()),
        };
        fs::write(
            primary.join(&file),
            format!(
                "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": true}}",
                earlier.as_json()
            ),
        )
        .unwrap();
        fs::remove_file(agents.join(&file)).unwrap();
        let middle = registry.join("restore-reports");
        fs::create_dir_all(&middle).unwrap();
        fs::write(middle.join(&file), &complete).unwrap();
        let (path, _) = completed_boot_report(&registry, &config, &epoch, None).unwrap();
        assert_eq!(path, middle.join(&file));
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn an_incomplete_report_is_not_missing_in_either_layout() {
        // #2018: a report that exists but did not complete (the drill's
        // coordinator-refused boot) must not collapse into Missing.
        let directory =
            std::env::temp_dir().join(format!("restore-incomplete-{}", std::process::id()));
        let registry = directory.join("registry");
        fs::create_dir_all(&registry).unwrap();
        let mut config = RestoreConfig::load(&registry).unwrap();
        let boot = "11111111-2222-3333-4444-555555555555";
        let epoch = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: Some("invocation:m1".to_owned()),
        };
        let incomplete = format!(
            "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": false, \
             \"coordinator\": {{\"action\": \"refused\", \"reason\": \"no pane\"}}}}",
            epoch.as_json()
        );
        let file = format!("{boot}.json");
        assert_eq!(
            boot_report_state(&registry, &config, &epoch, None),
            BootReportState::Missing
        );
        // At the current location.
        let primary = reports_directory(&registry);
        fs::create_dir_all(&primary).unwrap();
        fs::write(primary.join(&file), &incomplete).unwrap();
        match boot_report_state(&registry, &config, &epoch, None) {
            BootReportState::Incomplete((path, report)) => {
                assert_eq!(path, primary.join(&file));
                assert_eq!(incomplete_reason(&report), "coordinator refused: no pane");
            }
            other => panic!("expected Incomplete, got {other:?}"),
        }
        assert!(completed_boot_report(&registry, &config, &epoch, None).is_none());
        // Only at the legacy agents/ location: still Incomplete, same path.
        fs::remove_file(primary.join(&file)).unwrap();
        let agents = registry.join("agents/restore-reports");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join(&file), &incomplete).unwrap();
        match boot_report_state(&registry, &config, &epoch, None) {
            BootReportState::Incomplete((path, _)) => assert_eq!(path, agents.join(&file)),
            other => panic!("expected Incomplete, got {other:?}"),
        }
        // An explicit report_dir is the one location: no fallback reads.
        let explicit = directory.join("explicit");
        fs::create_dir_all(&explicit).unwrap();
        config.report_dir = Some(explicit.clone());
        assert_eq!(
            boot_report_state(&registry, &config, &epoch, None),
            BootReportState::Missing
        );
        fs::write(explicit.join(&file), &incomplete).unwrap();
        match boot_report_state(&registry, &config, &epoch, None) {
            BootReportState::Incomplete((path, _)) => assert_eq!(path, explicit.join(&file)),
            other => panic!("expected Incomplete, got {other:?}"),
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_missing_restore_report_is_reported_once_per_boot() {
        // #2018: first pass for a boot stays failing; later passes in the
        // same watch process are suppressed and never alert.
        let row = || {
            json!({"check": "restore", "subject": "boot-a", "ok": false,
                   "detail": "no completed restore report for this boot",
                   "missing_restore_report": true, "repair": Value::Null})
        };
        let mut noted = std::collections::BTreeSet::new();
        let mut first = vec![row()];
        apply_restore_missing_once(&mut first, &mut noted);
        assert_eq!(first[0]["ok"], json!(false));
        let mut second = vec![row()];
        apply_restore_missing_once(&mut second, &mut noted);
        assert_eq!(second[0]["ok"], Value::Null);
        assert_eq!(second[0]["suppressed"], "reported-once-this-boot");
        // A different boot reports again.
        let mut other = vec![json!({"check": "restore", "subject": "boot-b", "ok": false,
                   "detail": "no completed restore report for this boot",
                   "missing_restore_report": true, "repair": Value::Null})];
        apply_restore_missing_once(&mut other, &mut noted);
        assert_eq!(other[0]["ok"], json!(false));
        // Incomplete rows are never suppressed by this rule.
        let mut incomplete = vec![json!({"check": "restore", "subject": "boot-a", "ok": false,
                   "detail": "restore report for this boot is incomplete", "repair": Value::Null})];
        apply_restore_missing_once(&mut incomplete, &mut noted);
        assert_eq!(incomplete[0]["ok"], json!(false));
    }

    #[test]
    fn completed_boot_report_proves_legacy_report_by_manager_start() {
        // A pre-epoch report (no manager identity) finished after the
        // current manager started was written under this manager: it
        // counts. Finished before it started, a restart has happened
        // since: it does not. With no start to compare, watch can say
        // predates-keying distinctly instead of alarming.
        let directory =
            std::env::temp_dir().join(format!("restore-legacy-epoch-{}", std::process::id()));
        let registry = directory.join("registry");
        let primary = reports_directory(&registry);
        fs::create_dir_all(&primary).unwrap();
        let config = RestoreConfig::load(&registry).unwrap();
        let boot = "11111111-2222-3333-4444-555555555555";
        let epoch = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: Some("invocation:m1".to_owned()),
        };
        let file = primary.join(format!("{boot}.json"));
        fs::write(
            &file,
            format!("{{\"boot_id\": \"{boot}\", \"complete\": true, \"finished_at\": 2000.0}}"),
        )
        .unwrap();
        assert!(matches!(
            boot_report_state(&registry, &config, &epoch, Some(1000.0)),
            BootReportState::Completed(_)
        ));
        assert_eq!(
            boot_report_state(&registry, &config, &epoch, Some(3000.0)),
            BootReportState::Missing
        );
        assert!(matches!(
            boot_report_state(&registry, &config, &epoch, None),
            BootReportState::PredatesKeying(_)
        ));
        // A report carrying its own manager identity is judged strictly,
        // however its finish time compares.
        let other = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: Some("invocation:m0".to_owned()),
        };
        fs::write(
            &file,
            format!(
                "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": true, \"finished_at\": 2000.0}}",
                other.as_json()
            ),
        )
        .unwrap();
        assert_eq!(
            boot_report_state(&registry, &config, &epoch, Some(1000.0)),
            BootReportState::Missing
        );
        // Exact managerless equality is still a match without any manager
        // start proof, in both recorded forms: a current report carrying
        // `"user_manager": null`, and a report with no epoch object at all.
        // Restore replay depends on this when manager identity is unknown
        // on both sides.
        let managerless = RestoreEpoch {
            boot_id: boot.to_owned(),
            user_manager: None,
        };
        fs::write(
            &file,
            format!(
                "{{\"boot_id\": \"{boot}\", \"epoch\": {}, \"complete\": true, \"finished_at\": 2000.0}}",
                managerless.as_json()
            ),
        )
        .unwrap();
        let report = read_report(&file).unwrap();
        assert_eq!(
            managerless.legacy_match(&report, None),
            LegacyMatch::Matches
        );
        assert!(matches!(
            boot_report_state(&registry, &config, &managerless, None),
            BootReportState::Completed(_)
        ));
        fs::write(
            &file,
            format!("{{\"boot_id\": \"{boot}\", \"complete\": true, \"finished_at\": 2000.0}}"),
        )
        .unwrap();
        let report = read_report(&file).unwrap();
        assert_eq!(
            managerless.legacy_match(&report, None),
            LegacyMatch::Matches
        );
        assert!(matches!(
            boot_report_state(&registry, &config, &managerless, None),
            BootReportState::Completed(_)
        ));
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_layout_sentinel_is_never_an_agent() {
        // #1793: agents/agent.json is the layout-2 sentinel, not an agent;
        // registered_names must list only real agents/<name>/agent.json.
        let directory =
            std::env::temp_dir().join(format!("restore-sentinel-{}", std::process::id()));
        let registry = directory.join("registry");
        let agents = registry.join("agents");
        fs::create_dir_all(agents.join("real-one")).unwrap();
        fs::write(agents.join("agent.json"), "\"sentinel\"").unwrap();
        fs::write(agents.join("real-one/agent.json"), "{}").unwrap();
        assert_eq!(
            registered_names(&registry).unwrap(),
            vec!["real-one".to_owned()]
        );
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn liaison_first_then_the_coordinators_viewer_then_lanes() {
        // #1793 owner ruling: liaison first, the designated coordinator's
        // viewer second, then lanes by name, unmanaged last.
        let order = vec![
            "w1:lane".to_owned(),
            "w1:coord-view".to_owned(),
            "w1:liaison".to_owned(),
            "w1:other".to_owned(),
        ];
        let mut claimed: BTreeMap<String, ClaimedTab> = BTreeMap::new();
        claimed.insert(
            "w1:p1".to_owned(),
            ("lane-a".to_owned(), Some("w1:lane".to_owned()), None, None),
        );
        claimed.insert(
            "w1:p2".to_owned(),
            (
                "liaison".to_owned(),
                Some("w1:liaison".to_owned()),
                None,
                None,
            ),
        );
        let mut viewer_tabs: BTreeMap<String, ViewerTab> = BTreeMap::new();
        viewer_tabs.insert("w1:coord-view".to_owned(), ("coord".to_owned(), None, None));
        let labels: BTreeMap<String, String> = BTreeMap::new();
        let roles: BTreeMap<String, String> = BTreeMap::new();
        let entries = ranked_entries(
            &order,
            &claimed,
            &viewer_tabs,
            &labels,
            &roles,
            &std::collections::BTreeSet::from(["liaison".to_owned()]),
            Some("coord"),
        );
        assert_eq!(
            crate::tab_order::canonical_tabs(&entries),
            vec![
                "w1:liaison".to_owned(),
                "w1:coord-view".to_owned(),
                "w1:lane".to_owned(),
                "w1:other".to_owned()
            ]
        );
    }

    #[test]
    fn a_headless_coordinators_viewer_tab_stays_first_through_hygiene_ranking() {
        // #1225 Phase 1: a headless designated coordinator has no pane, so
        // hygiene claims its tail tab only as a viewer. It must rank first,
        // a headless lane's viewer must rank as that lane, an unnamed tab
        // last, and ranking must be idempotent with no pane ids involved.
        let order = vec![
            "w1:t4".to_owned(),
            "w1:t3".to_owned(),
            "w1:t2".to_owned(),
            "w1:t1".to_owned(),
        ];
        let mut claimed: BTreeMap<String, ClaimedTab> = BTreeMap::new();
        claimed.insert(
            "w1:p9".to_owned(),
            (
                "lane-b".to_owned(),
                Some("w1:t3".to_owned()),
                Some("muse-worker".to_owned()),
                None,
            ),
        );
        let mut viewer_tabs: BTreeMap<String, ViewerTab> = BTreeMap::new();
        viewer_tabs.insert("w1:t1".to_owned(), ("coord".to_owned(), None, None));
        viewer_tabs.insert(
            "w1:t2".to_owned(),
            ("lane-a".to_owned(), Some("muse-worker".to_owned()), None),
        );
        let labels: BTreeMap<String, String> = BTreeMap::new();
        let roles: BTreeMap<String, String> = BTreeMap::new();
        let bridge: std::collections::BTreeSet<String> = Default::default();
        let entries = ranked_entries(
            &order,
            &claimed,
            &viewer_tabs,
            &labels,
            &roles,
            &bridge,
            Some("coord"),
        );
        let desired = crate::tab_order::canonical_tabs(&entries);
        assert_eq!(desired, vec!["w1:t1", "w1:t2", "w1:t3", "w1:t4"]);
        // A second pass over the desired order changes nothing, and the
        // viewer claim never invents a pane for the headless agents.
        let reordered_entries = ranked_entries(
            &desired,
            &claimed,
            &viewer_tabs,
            &labels,
            &roles,
            &bridge,
            Some("coord"),
        );
        assert_eq!(
            crate::tab_order::canonical_tabs(&reordered_entries),
            desired
        );
        assert_eq!(claimed.len(), 1, "no pane ids were added or changed");
        assert!(claimed.contains_key("w1:p9"));
        // Without a named viewer tab the same tabs rank unmanaged and last.
        let unnamed = ranked_entries(
            &order,
            &claimed,
            &BTreeMap::new(),
            &labels,
            &roles,
            &bridge,
            Some("coord"),
        );
        assert_eq!(
            crate::tab_order::canonical_tabs(&unnamed),
            vec!["w1:t3", "w1:t4", "w1:t2", "w1:t1"]
        );
    }

    #[test]
    fn profile_roles_reads_each_profiles_role_and_tolerates_absence() {
        let directory = std::env::temp_dir().join(format!("restore-roles-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        assert!(profile_roles(&directory).is_empty());
        fs::write(
            directory.join("profiles.json"),
            r#"{"schema": "agentctl-profiles/v1", "profiles": {"coordinator": {"harness": "claude", "mode": "interactive", "role": "liaison"}, "worker": {"harness": "claude", "mode": "interactive"}}}"#,
        )
        .unwrap();
        let roles = profile_roles(&directory);
        assert_eq!(
            roles.get("coordinator").map(String::as_str),
            Some("liaison")
        );
        assert!(!roles.contains_key("worker"));
        fs::remove_dir_all(&directory).unwrap();
    }

    struct BusyOnceHost {
        status_calls: RefCell<u32>,
    }
    impl Host for BusyOnceHost {
        fn boot_id(&self) -> Result<String> {
            Ok("boot".to_owned())
        }
        fn boot_time(&self) -> Option<u64> {
            Some(0)
        }
        fn now(&self) -> f64 {
            0.0
        }
        fn sleep(&self, _duration: Duration) {}
        fn run(&self, argv: &[String], _cwd: Option<&Path>, _t: Duration) -> CommandOutcome {
            let joined = argv.join(" ");
            if joined.contains(" status ") {
                let mut calls = self.status_calls.borrow_mut();
                *calls += 1;
                if *calls == 1 {
                    return CommandOutcome {
                        status: Some(3),
                        stdout: String::new(),
                        stderr: "REFUSED: state lock is busy".to_owned(),
                    };
                }
                return CommandOutcome {
                    status: Some(0),
                    stdout:
                        "{\"active\":[{\"slot\":\"s1\",\"generation\":3,\"agent\":\"slot-agent\"}]}"
                            .to_owned(),
                    stderr: String::new(),
                };
            }
            CommandOutcome {
                status: Some(0),
                stdout: "transferred".to_owned(),
                stderr: String::new(),
            }
        }
    }

    #[test]
    fn verify_slot_bind_refuses_a_lane_with_no_project_row() {
        // #2089: a lane recorded under a project with no image row is
        // held, never waved through as not image-backed; a failed row
        // for an image-backed project holds it too, while a successful
        // row listing no slot (a plain worktree) still verifies.
        let host = BusyOnceHost {
            status_calls: RefCell::new(0),
        };
        let agent = SlotAgent {
            name: "beta".to_owned(),
            slot: "slot-b".to_owned(),
            project: PathBuf::from("/projects/b"),
            cwd: None,
            running: true,
        };
        let missing = verify_slot_bind(&host, &agent, None).unwrap_err();
        assert!(
            missing.contains("no wrkslots image status row"),
            "{missing}"
        );
        let failed =
            json!({"action": "failed", "status": 3, "detail": "boom", "warnings": [], "slots": []});
        let refused = verify_slot_bind(&host, &agent, Some(&failed)).unwrap_err();
        assert!(refused.contains("did not run successfully"), "{refused}");
        let plain = json!({"action": "ran", "status": 0, "warnings": [], "slots": []});
        let verified = verify_slot_bind(&host, &agent, Some(&plain)).unwrap();
        assert_eq!(verified["action"], "verified");
        assert!(
            verified["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("not image-backed"),
            "{verified}"
        );
    }

    #[test]
    fn eden_refuses_a_slot_recorded_under_no_covered_project() {
        // #2089: a slot whose project has no covered image row is
        // refused and never passed to the verb, which starts processes.
        let host = BusyOnceHost {
            status_calls: RefCell::new(0),
        };
        let agent = SlotAgent {
            name: "beta".to_owned(),
            slot: "slot-b".to_owned(),
            project: PathBuf::from("/projects/b"),
            cwd: None,
            running: true,
        };
        let covered =
            vec![json!({"action": "ran", "project": "/projects/a", "slots": [], "warnings": []})];
        let rows =
            restart_eden_daemons(&host, Some(Path::new("/opt/wrkslots")), &[agent], &covered);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["status"], "refused");
        assert!(
            rows[0]["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("not among the projects"),
            "{rows:?}"
        );
        assert_eq!(*host.status_calls.borrow(), 0, "the verb was not run");
    }

    #[test]
    fn bind_retries_a_busy_status_lock() {
        let host = BusyOnceHost {
            status_calls: RefCell::new(0),
        };
        let row = bind_slot_owner(
            &host,
            Path::new("/wrkslots"),
            Path::new("/project"),
            "s1",
            "worker",
            4242,
        )
        .unwrap();
        assert_eq!(row["generation"], 3);
        assert_eq!(*host.status_calls.borrow(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn bind_reads_a_large_status_document_with_stderr_noise() {
        use std::os::unix::fs::PermissionsExt;
        // #1322 slot-bind-status-parse: a status document past the old
        // 16 KiB capture tail (and past the 64 KiB pipe buffer) must still
        // bind, with the soak banners on stderr kept out of the parse.
        for target in [20_000usize, 200_000] {
            let directory = std::env::temp_dir().join(format!(
                "restore-big-status-{}-{target}",
                std::process::id()
            ));
            fs::create_dir_all(&directory).unwrap();
            let mut rows = vec![json!({
                "slot": "edone",
                "generation": 7,
                "agent": "mu-edone",
                "owner": {"pid": 4242, "kind": "process"},
            })];
            let mut document = json!({
                "machine": "test",
                "active": rows.clone(),
                "archive": [],
                "holds": [],
            });
            while serde_json::to_string_pretty(&document).unwrap().len() < target {
                rows.push(json!({
                    "slot": format!("filler-{}", rows.len()),
                    "generation": 1,
                    "agent": "filler",
                    "note": "x".repeat(512),
                }));
                document["active"] = json!(rows);
            }
            let payload = serde_json::to_string_pretty(&document).unwrap();
            assert!(
                payload.len() >= target,
                "payload is {} bytes",
                payload.len()
            );
            fs::write(directory.join("status.json"), &payload).unwrap();
            let script = directory.join("wrkslots");
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\necho '[wrkslots] status: rust (soak)' >&2\necho '[wrkslotsd] rust cache=verified source=test binary=test' >&2\ncase \" $* \" in *\" transfer-owner \"*) echo transferred; exit 0;; esac\ncat \"{}\"\n",
                    directory.join("status.json").display()
                ),
            )
            .unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let row = bind_slot_owner(&SystemHost, &script, &directory, "edone", "mu-edone", 4242)
                .unwrap();
            assert_eq!(row["generation"], 7);
            assert_eq!(row["agent"], "mu-edone");
            fs::remove_dir_all(&directory).unwrap();
        }
    }

    #[test]
    fn unknown_keys_are_ignored_at_every_level_but_duplicates_are_refused() {
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
        assert_eq!(config.unknown_paths(), ["a_future", "zz_future"]);
        // A key a newer agentctl added inside a timer is read around too (#809
        // chat-bridge-json-forward-compat), and named by its path.
        let config = load(
            r#"{"schema": "agentctl-restore/v1", "timers": [{"unit": "t.timer", "max_age_seconds": 60, "surprise": 1}], "z": 1}"#,
        )
        .unwrap();
        assert_eq!(config.unknown_paths(), ["timers[0].surprise", "z"]);
        for refused in [
            r#"{"schema": "agentctl-restore/v1", "units": ["a.service"], "units": ["b.service"]}"#,
            r#"{"schema": "agentctl-restore/v1", "also_registered": [{"registry": "/r", "name": "a", "name": "b"}]}"#,
            r#"{"schema": "agentctl-restore/v1", "x": 1, "x": 2}"#,
        ] {
            assert!(load(refused).is_err(), "{refused}");
        }
        let error = load(
            "{\"schema\": \"agentctl-restore/v1\",\n \"timers\": [{\"unit\": \"t.timer\", \"max_age_seconds\": \"sixty\"}]}",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("line 2"), "{error}");
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn scheduled_entries_load_and_are_validated() {
        let directory =
            std::env::temp_dir().join(format!("restore-scheduled-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        let load = |text: &str| {
            fs::write(&path, text).unwrap();
            RestoreConfig::load(&directory)
        };
        let config = load(
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "nightly-backup", "on_calendar": "Mon 06:00", "command": ["/home/me/bin/backup", "--quiet"]}]}"##,
        )
        .unwrap();
        assert_eq!(config.scheduled.len(), 1);
        assert_eq!(config.scheduled[0].name, "nightly-backup");
        assert_eq!(config.scheduled[0].command.len(), 2);
        // #1220: a daily 05:30 in a named timezone, with the turnover
        // entry's timeout, environment, ordering and success codes.
        let config = load(
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "turnover", "on_calendar": "*-*-* 05:30:00 America/New_York", "command": ["/home/me/bin/agentctl", "services", "turnover"], "timeout_start_seconds": 7200, "environment": {"PATH": "/home/me/bin:/usr/bin:/bin", "HOME": "/home/me", "AGENTCTL_AGENT": "turnover-timer"}, "after": ["au-mirror-sync.service"], "success_exit_status": [0, 74]}]}"##,
        )
        .unwrap();
        assert_eq!(
            config.scheduled[0].on_calendar,
            "*-*-* 05:30:00 America/New_York"
        );
        assert_eq!(config.scheduled[0].timeout_start_seconds, Some(7200));
        assert_eq!(config.scheduled[0].after, ["au-mirror-sync.service"]);
        assert!(load(
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily", "command": ["/bin/true"], "timeout_start_seconds": 0}]}"##,
        )
        .is_err());
        for refused in [
            // A duplicate name would render one pair over another.
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily", "command": ["/bin/true"]}, {"name": "a", "on_calendar": "hourly", "command": ["/bin/true"]}]}"##,
            // A schedule is one literal calendar line, never a command separator.
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily; rm -rf /", "command": ["/bin/true"]}]}"##,
            // The program must be absolute so the unit does not depend on PATH.
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily", "command": ["true"]}]}"##,
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily", "command": []}]}"##,
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "Bad Name", "on_calendar": "daily", "command": ["/bin/true"]}]}"##,
        ] {
            assert!(load(refused).is_err(), "{refused}");
        }
        // A key a newer agentctl added inside an entry is read around and named, as in the rest
        // of the file (#809 chat-bridge-json-forward-compat).
        let config = load(
            r##"{"schema": "agentctl-restore/v1", "scheduled": [{"name": "a", "on_calendar": "daily", "command": ["/bin/true"], "surprise": 1}]}"##,
        )
        .unwrap();
        assert_eq!(config.unknown_paths(), ["scheduled[0].surprise"]);
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn watch_json_loads_precedence_conflict_and_fallback() {
        let dir = std::env::temp_dir().join(format!("watch-json-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // Fallback: settings only in restore.json.
        fs::write(dir.join("restore.json"), r#"{"schema":"agentctl-restore/v1","coordinator":"coordinator","herdr_unit":"a.service","timers":[{"unit":"t.timer","max_age_seconds":60}],"duties":{"notify":false}}"#).unwrap();
        let cfg = RestoreConfig::load_for_watch(&dir).unwrap();
        assert_eq!(cfg.herdr_unit.as_deref(), Some("a.service"));
        assert!(!cfg.duties.notify && cfg.duties.unit_repair);
        assert_eq!(
            RestoreConfig::load(&dir).unwrap().herdr_unit.as_deref(),
            Some("a.service")
        );
        // Precedence: watch.json disjoint keys win; restore keys stay.
        fs::write(dir.join("watch.json"), r#"{"schema":"agentctl-watch/v1","bare_shell_grace_seconds":7,"close_bare_shells":false}"#).unwrap();
        let cfg = RestoreConfig::load_for_watch(&dir).unwrap();
        assert_eq!(cfg.bare_shell_grace_seconds, 7);
        assert!(!cfg.close_bare_shells);
        assert_eq!(
            cfg.herdr_unit.as_deref(),
            Some("a.service"),
            "restore fallback per key"
        );
        // Restore command ignores watch.json.
        assert_eq!(
            RestoreConfig::load(&dir).unwrap().bare_shell_grace_seconds,
            600
        );
        // Both-files refusal naming both paths.
        fs::write(
            dir.join("watch.json"),
            r#"{"schema":"agentctl-watch/v1","herdr_unit":"b.service"}"#,
        )
        .unwrap();
        let err = RestoreConfig::load_for_watch(&dir).unwrap_err().to_string();
        assert!(
            err.contains("watch.json")
                && err.contains("restore.json")
                && err.contains("herdr_unit"),
            "{err}"
        );
        // Unknown duty refused in either file; unknown flag refused.
        fs::remove_file(dir.join("watch.json")).unwrap();
        fs::write(
            dir.join("restore.json"),
            r#"{"schema":"agentctl-restore/v1","duties":{"bogus":true}}"#,
        )
        .unwrap();
        assert!(RestoreConfig::load(&dir)
            .unwrap_err()
            .to_string()
            .contains("bogus"));
        fs::write(
            dir.join("restore.json"),
            r#"{"schema":"agentctl-restore/v1"}"#,
        )
        .unwrap();
        fs::write(
            dir.join("watch.json"),
            r#"{"schema":"agentctl-watch/v1","duties":{"nope":false}}"#,
        )
        .unwrap();
        assert!(RestoreConfig::load_for_watch(&dir)
            .unwrap_err()
            .to_string()
            .contains("nope"));
        assert!(parse_duty_override("bogus=off").is_err());
        assert!(parse_duty_override("unit_repair=maybe").is_err());
        assert_eq!(
            parse_duty_override("unit_repair=off").unwrap(),
            ("unit_repair".to_owned(), false)
        );
        let mut duties = WatchDuties::default();
        duties.apply_override("unit_repair=off").unwrap();
        assert!(!duties.unit_repair && duties.notify);
        fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod seam_tests {
    use super::resolve_program;

    #[test]
    fn resolve_program_refuses_under_a_test_harness() {
        assert_eq!(
            resolve_program(None, true, "systemctl"),
            "/nonexistent/agentctl-test-harness-refusal"
        );
        assert_eq!(
            resolve_program(Some("/fake/systemctl"), true, "systemctl"),
            "/fake/systemctl"
        );
        assert_eq!(resolve_program(None, false, "systemctl"), "systemctl");
        assert_eq!(
            resolve_program(Some(""), true, "systemd-run"),
            "/nonexistent/agentctl-test-harness-refusal"
        );
    }
}
