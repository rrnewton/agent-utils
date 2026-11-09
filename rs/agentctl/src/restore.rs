//! Bring a coordinator team back after the host restarts, in one idempotent command.
//!
//! `agentctl restore` runs these steps in order and writes what each one did to a JSON report
//! named for the boot:
//!
//! 1. Hold `<registry>/.restore.lock`, so a boot unit and a manual run never overlap: the
//!    second waits for the first, then finds its completed report.
//! 2. Replay this boot's recorded report when it already says the restore completed.
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
    /// Keys a newer agentctl added, kept when this record is written back (#809
    /// chat-bridge-json-forward-compat).
    #[serde(flatten)]
    pub unknown: crate::forward_compat::KeptKeys,
}

/// Whether `value` is a calendar schedule systemd can take literally: one line of
/// letters, digits, and the punctuation calendar syntax uses, with no specifier,
/// quote, or command separator.
pub(crate) fn calendar_schedule(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" -,.:/*".contains(&byte))
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
    /// Directory for per-boot reports (default `REGISTRY/restore-reports`).
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

/// The host facts and effects restore and watch use, injectable for tests.
pub trait Host {
    /// `/proc/sys/kernel/random/boot_id`.
    fn boot_id(&self) -> Result<String>;
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
        // A plain sleep resumes after the signal that sets the termination flag, so the watch
        // slept out its interval through SIGTERM (#909 watch-sigterm-stop).
        crate::agent::sleep_unless_terminated(duration);
    }

    fn terminated(&self) -> bool {
        crate::agent::termination_requested()
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
    /// The Herdr binary whose `--version` the version pin checks.
    pub herdr_bin: PathBuf,
}

/// The XDG state report directory: `$XDG_STATE_HOME/agentctl/restore`, else under
/// the home. New reports default to the registry's own `restore-reports/` instead
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

fn report_path(registry: &Path, config: &RestoreConfig, boot_id: &str) -> PathBuf {
    let directory = config
        .report_dir
        .clone()
        .unwrap_or_else(|| registry.join("restore-reports"));
    directory.join(format!("{boot_id}.json"))
}

/// Where a report for this boot may already live under the pre-#975 default.
/// An explicit `report_dir` names the one location, so there is no fallback read.
fn legacy_report_path(config: &RestoreConfig, boot_id: &str) -> Option<PathBuf> {
    if config.report_dir.is_some() {
        return None;
    }
    default_report_dir().map(|directory| directory.join(format!("{boot_id}.json")))
}

fn read_boot_report_from(primary: &Path, legacy: Option<&Path>) -> Option<(PathBuf, Value)> {
    if let Some(report) = read_report(primary) {
        return Some((primary.to_path_buf(), report));
    }
    let legacy = legacy?;
    read_report(legacy).map(|report| (legacy.to_path_buf(), report))
}

/// This boot's existing report and where it was found: the current location
/// first, then the XDG state one, so a same-boot report from an older build
/// still satisfies the already-restored and watch checks.
fn read_boot_report(
    registry: &Path,
    config: &RestoreConfig,
    boot_id: &str,
) -> Option<(PathBuf, Value)> {
    read_boot_report_from(
        &report_path(registry, config, boot_id),
        legacy_report_path(config, boot_id).as_deref(),
    )
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
fn slot_row(host: &dyn Host, wrkslots: &Path, project: &Path, slot: &str) -> Result<Value> {
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
    let document: Value = serde_json::from_str(stdout)
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
/// mounted at the location the lane will start in (#1130 D3).
///
/// The evidence is the `wrkslots image status` run already in the report
/// plus one `findmnt` on the mounted location. A slot the image status does
/// not list is not image-backed, so there is no bind to verify. The slot is
/// resolved from its recorded name and project (see `slot_agents`), never
/// from the stored working directory; the directory is only checked to be
/// the mounted slot itself, not a bare mount point or a stale original.
fn verify_slot_bind(
    host: &dyn Host,
    agent: &SlotAgent,
    images: &Value,
) -> std::result::Result<Value, String> {
    let refuse = |detail: String| {
        format!(
            "slot bind not verified for {} (slot {}): {detail}",
            agent.name, agent.slot
        )
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

/// The recorded lifecycle of one agent, for registry-derived desired state (#1129).
///
/// An unreadable, unparseable, or lifecycle-less record is undecidable: the caller refuses
/// it with this evidence instead of silently excluding it, because derivation cannot tell
/// whether it was running at shutdown.
fn derived_lifecycle(registry: &Path, name: &str) -> std::result::Result<String, String> {
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
        format!(
            "Desired state: {}.",
            match report["desired_state_source"].as_str() {
                Some("pinned-list") => "pinned-list (restore.json agents, plus the coordinator)",
                _ => "registry-derived (every record running at shutdown; restore.json exclude left alone)",
            }
        ),
        format!("Revived: {}.", list("revived", &pane)),
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
    let coordinator = config.coordinator.clone().or_else(|| {
        let text = fs::read_to_string(registry.join("coordinator.json")).ok()?;
        serde_json::from_str::<Value>(&text).ok()?["name"]
            .as_str()
            .map(str::to_owned)
    });
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
    let path = report_path(registry, config, &boot_id);
    if !options.force && !options.dry_run {
        if let Some((previous_path, previous)) = read_boot_report(registry, config, &boot_id) {
            if previous["complete"] == true {
                // #1132: replay the recorded outcomes (rows, refusals and
                // completion fields) labelled as a replay, so a rerun can
                // never read as a fresh success for a boot that refused
                // lanes. The caller maps this to the recorded exit class.
                let mut replayed = previous;
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
                    object.insert("action".to_owned(), json!("already-restored"));
                    object.insert("replayed".to_owned(), json!(true));
                    object.insert("report".to_owned(), json!(previous_path));
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
        "boot_time": host.boot_time(),
        "previous_boots": boots.iter().filter(|boot| **boot != boot_id).collect::<Vec<_>>(),
        "restart_detected": boots.iter().any(|boot| *boot != boot_id),
        "started_at": host.now(),
        "dry_run": options.dry_run,
        // #1129: why each lane was in or out of the desired set.
        "desired_state_source": if config.agents.is_some() { "pinned-list" } else { "registry-derived" },
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
                    Some(verify_slot_bind(host, agent, &report["slot_images"]))
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
            if let Some(desired) = config.agents.as_ref() {
                if !desired.iter().any(|wanted| wanted == name) {
                    agents.push(json!({"name": name, "action": "excluded", "reason": "not listed in restore.json agents (desired state)"}));
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
                        continue;
                    }
                    Ok(_) => {}
                }
            }
            // #1130 D3: never revive a slot lane into a slot whose bind
            // mount is missing; refuse that lane and carry on with the rest.
            let bind = match boxed.iter().find(|agent| agent.name == *name) {
                Some(agent) if options.wrkslots.is_some() && !options.dry_run => {
                    Some(verify_slot_bind(host, agent, &report["slot_images"]))
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
                continue;
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
                .filter_map(|name| manager.status(name).ok())
                .filter_map(|status| status["pane_id"].as_str().map(str::to_owned))
                .collect();
            let cwd_of = |name: &str| {
                manager
                    .status(name)
                    .ok()
                    .and_then(|s| s["cwd"].as_str().map(str::to_owned))
            };
            let pane_of = |name: &str| {
                manager
                    .status(name)
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

/// Whether a timer problem is its schedule itself or only its freshness clock.
///
/// A broken schedule (inactive, not loaded, no next elapse) is repaired by restarting the
/// timer. A merely stale timer is repaired by starting its service once: restarting a timer
/// whose trigger counts from its service's last run resets that trigger, so the timer never
/// elapses on its own and the watch would restart it on every pass (#961).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TimerProblemKind {
    ScheduleBroken,
    Stale,
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
        return Ok(
            confirm_inactive_timer(host, timer, active)?.map(|detail| TimerProblem {
                detail,
                kind: TimerProblemKind::ScheduleBroken,
            }),
        );
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
        let complete = read_boot_report(registry, config, &boot_id)
            .is_some_and(|(_path, report)| report["complete"] == true);
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
fn hygiene_pass<A: ManagedApi + ?Sized>(
    client: &A,
    registry: &Path,
    config: &RestoreConfig,
    host: &dyn Host,
    bare_since: &mut BTreeMap<String, f64>,
    closed: &mut Vec<Value>,
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
        if let Ok(status) = manager.status(name) {
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
    let managed_workspaces: std::collections::BTreeSet<String> = panes
        .iter()
        .filter(|pane| {
            claimed.contains_key(&pane.pane_id) || viewer_tabs.contains_key(&pane.tab_id)
        })
        .map(|pane| pane.workspace_id.clone())
        .collect();
    // Canonical tab order, through the Herdr socket when reachable.
    if let Some(socket) = crate::tab_order::socket_path().filter(|path| path.exists()) {
        for workspace in &managed_workspaces {
            let Ok(order) = crate::tab_order::list_tabs(&socket, workspace) else {
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
                    && crate::tab_order::move_tab(&socket, tab, index).is_ok()
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
    // Bare shells: only in managed workspaces, only when unclaimed.
    let now = host.now();
    let mut seen: std::collections::BTreeSet<String> = Default::default();
    for pane in &panes {
        if claimed.contains_key(&pane.pane_id)
            || viewer_tabs.contains_key(&pane.tab_id)
            || !managed_workspaces.contains(&pane.workspace_id)
        {
            continue;
        }
        if !client.pane_is_idle_shell(&pane.pane_id).unwrap_or(false) {
            continue;
        }
        seen.insert(pane.pane_id.clone());
        let since = *bare_since.entry(pane.pane_id.clone()).or_insert(now);
        if config.close_bare_shells && now - since >= config.bare_shell_grace_seconds as f64 {
            let info = client.pane_info(&pane.pane_id).ok();
            if client.close_pane(&pane.pane_id).is_ok() {
                let row = json!({
                    "pane_id": pane.pane_id,
                    "tab_id": pane.tab_id,
                    "cwd": info.as_ref().map(|info| info.cwd.clone()),
                    "why": format!("bare shell idle {}s past {}s grace with no registered agent",
                                   now - since, config.bare_shell_grace_seconds),
                    "closed_at": now,
                });
                eprintln!(
                    "agentctl watch: closed bare shell {} ({})",
                    pane.pane_id, row["why"]
                );
                closed.push(row);
                bare_since.remove(&pane.pane_id);
            }
        }
    }
    bare_since.retain(|pane, _| seen.contains(pane));
    findings
}

/// Watch the team's boot health until terminated, writing `health.json`.
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
    let mut bare_since: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["bare_shell_since"].clone()).ok())
        .unwrap_or_default();
    let mut timer_repaired_at: BTreeMap<String, f64> = path
        .as_deref()
        .and_then(read_report)
        .and_then(|previous| serde_json::from_value(previous["timer_repaired_at"].clone()).ok())
        .unwrap_or_default();
    // Units whose cooldown finding has already gone to stderr this episode (#961): a
    // cooldown is logged once when it starts, not on every pass it spans.
    let mut cooldown_noted: std::collections::BTreeSet<String> = Default::default();
    let mut closed_bare_shells: Vec<Value> = Vec::new();
    let mut pass: u64 = 0;
    loop {
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
        );
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
                &mut closed_bare_shells,
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
            "bare_shell_since": bare_since,
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
    fn boot_report_read_prefers_the_registry_location_and_falls_back_to_legacy() {
        // #975: a same-boot report written by an older build under the XDG
        // default still counts, but the registry location wins when both exist.
        let directory =
            std::env::temp_dir().join(format!("restore-reports-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let primary = directory.join("primary.json");
        let legacy = directory.join("legacy.json");
        assert!(read_boot_report_from(&primary, Some(&legacy)).is_none());
        fs::write(&legacy, "{\"complete\": true, \"where\": \"legacy\"}").unwrap();
        let (path, report) = read_boot_report_from(&primary, Some(&legacy)).unwrap();
        assert_eq!(path, legacy);
        assert_eq!(report["where"], "legacy");
        fs::write(&primary, "{\"complete\": false, \"where\": \"primary\"}").unwrap();
        let (path, report) = read_boot_report_from(&primary, Some(&legacy)).unwrap();
        assert_eq!(path, primary);
        assert_eq!(report["where"], "primary");
        fs::remove_dir_all(&directory).unwrap();
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
