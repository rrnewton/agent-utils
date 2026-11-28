//! `services transients` (#2441): a read-only record of every live transient unit (scopes and
//! services made through the systemd API) and a later check that each one is still known to
//! systemd by name while its cgroup still runs.
//!
//! A user-manager daemon-reload has been seen to make systemd report a live transient scope as
//! dead, which blinds anything that finds a unit by name (a meter that stops or reads a unit by
//! its name). An operator who must reload (the one-time move of a unit file to link form) saves
//! the list before the reload and checks it after. The check compares the two views such a meter
//! depends on: systemd's state for the unit name, and the kernel's cgroup with its accounting
//! files. A unit whose cgroup still has processes but whose name systemd no longer reports as
//! active, or whose cgroup changed or stopped accounting, is LOST: exactly the failure above.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};

use super::host::Host;

/// The record's schema.
pub const SCHEMA: &str = "agentctl-services-transients/v1";

/// The `systemctl show` properties the record needs.
pub const PROPERTIES: &str = "Id,Transient,LoadState,ActiveState,SubState,ControlGroup,MainPID";

/// What systemd says about one unit, as `systemctl show` key/value pairs.
pub type ShowFields = BTreeMap<String, String>;

/// One transient unit: systemd's view and the kernel's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transient {
    /// The unit name.
    pub unit: String,
    /// systemd's `ActiveState`.
    pub active_state: String,
    /// systemd's `SubState`.
    pub sub_state: String,
    /// The unit's cgroup, relative to the cgroup root.
    pub control_group: String,
    /// `MainPID` (0: none, as for a scope).
    pub main_pid: u32,
    /// `cgroup.events` says `populated 1`; `None` when the cgroup cannot be read.
    pub populated: Option<bool>,
    /// Processes in `cgroup.procs` (this level only).
    pub processes: Option<usize>,
    /// `cpu.stat` `usage_usec`: the CPU accounting a meter reads.
    pub cpu_usage_usec: Option<u64>,
    /// `memory.current`.
    pub memory_current: Option<u64>,
}

impl Transient {
    fn to_json(&self) -> Value {
        json!({
            "unit": self.unit,
            "active_state": self.active_state,
            "sub_state": self.sub_state,
            "control_group": self.control_group,
            "main_pid": self.main_pid,
            "populated": self.populated,
            "processes": self.processes,
            "cpu_usage_usec": self.cpu_usage_usec,
            "memory_current": self.memory_current,
        })
    }

    fn from_json(value: &Value) -> Result<Self, String> {
        let text = |name: &str| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("a saved unit has no {name}"))
        };
        Ok(Self {
            unit: text("unit")?,
            active_state: text("active_state")?,
            sub_state: text("sub_state")?,
            control_group: text("control_group")?,
            main_pid: value["main_pid"].as_u64().unwrap_or(0) as u32,
            populated: value["populated"].as_bool(),
            processes: value["processes"].as_u64().map(|count| count as usize),
            cpu_usage_usec: value["cpu_usage_usec"].as_u64(),
            memory_current: value["memory_current"].as_u64(),
        })
    }
}

/// Read every loaded transient scope and service: systemd's view, then its cgroup files under
/// `cgroup_root`.
pub fn collect(host: &dyn Host, cgroup_root: &Path) -> Result<Vec<Transient>, String> {
    let mut transients = Vec::new();
    for fields in host.transient_units()? {
        if fields.get("Transient").map(String::as_str) != Some("yes") {
            continue;
        }
        let field = |name: &str| fields.get(name).cloned().unwrap_or_default();
        let control_group = field("ControlGroup");
        let mut transient = Transient {
            unit: field("Id"),
            active_state: field("ActiveState"),
            sub_state: field("SubState"),
            main_pid: field("MainPID").parse().unwrap_or(0),
            control_group: control_group.clone(),
            populated: None,
            processes: None,
            cpu_usage_usec: None,
            memory_current: None,
        };
        read_cgroup(host, cgroup_root, &mut transient);
        transients.push(transient);
    }
    transients.sort_by(|left, right| left.unit.cmp(&right.unit));
    Ok(transients)
}

/// The cgroup directory of a `ControlGroup` value.
fn cgroup_dir(cgroup_root: &Path, control_group: &str) -> Option<PathBuf> {
    let relative = control_group.trim_start_matches('/');
    (!control_group.is_empty()).then(|| cgroup_root.join(relative))
}

fn read_cgroup(host: &dyn Host, cgroup_root: &Path, transient: &mut Transient) {
    let Some(directory) = cgroup_dir(cgroup_root, &transient.control_group) else {
        return;
    };
    let read = |name: &str| {
        host.read_file(&directory.join(name))
            .and_then(|bytes| String::from_utf8(bytes).ok())
    };
    transient.populated = read("cgroup.events").and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("populated "))
            .map(|value| value.trim() == "1")
    });
    transient.processes = read("cgroup.procs").map(|text| text.lines().count());
    transient.cpu_usage_usec = read("cpu.stat").and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("usage_usec "))
            .and_then(|value| value.trim().parse().ok())
    });
    transient.memory_current = read("memory.current").and_then(|text| text.trim().parse().ok());
}

/// The record `--save` writes.
#[must_use]
pub fn record(transients: &[Transient], taken_at: f64) -> Value {
    json!({
        "schema": SCHEMA,
        "taken_at": taken_at,
        "units": transients.iter().map(Transient::to_json).collect::<Vec<_>>(),
    })
}

/// Write the record to `path`, which must not exist yet.
pub fn save(path: &Path, document: &Value) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("{}: {error}", path.display()))
}

/// Read a record `--save` wrote.
pub fn load(path: &Path) -> Result<Vec<Transient>, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("{}: not JSON: {error}", path.display()))?;
    if document["schema"] != SCHEMA {
        return Err(format!(
            "{}: schema {} is not {SCHEMA}",
            path.display(),
            document["schema"]
        ));
    }
    document["units"]
        .as_array()
        .ok_or_else(|| format!("{}: no units list", path.display()))?
        .iter()
        .map(Transient::from_json)
        .collect()
}

/// systemd states that still count as alive for a unit found by name.
fn alive_state(state: &str) -> bool {
    matches!(state, "active" | "reloading" | "activating" | "refreshing")
}

/// The verdict on one saved unit.
fn verdict(
    before: &Transient,
    now: Option<&Transient>,
    kernel: &Transient,
) -> (&'static str, String) {
    // `kernel` is the saved unit's cgroup read now, whether or not systemd still names it.
    if kernel.populated != Some(true) {
        return (
            "ended",
            "its cgroup has no processes now (its work ended, or it was stopped)".to_owned(),
        );
    }
    let Some(now) = now else {
        return (
            "lost",
            "its cgroup still has processes, but systemd no longer lists the unit by name"
                .to_owned(),
        );
    };
    if !alive_state(&now.active_state) {
        return (
            "lost",
            format!(
                "its cgroup still has processes, but systemd reports it {}/{}",
                now.active_state, now.sub_state
            ),
        );
    }
    if now.control_group != before.control_group {
        return (
            "lost",
            format!(
                "systemd now names cgroup {} instead of {}",
                now.control_group, before.control_group
            ),
        );
    }
    match (before.cpu_usage_usec, kernel.cpu_usage_usec) {
        (Some(_), None) => {
            return ("lost", "its cpu.stat can no longer be read".to_owned());
        }
        (Some(old), Some(new)) if new < old => {
            return (
                "lost",
                format!("its cpu.stat usage_usec went back from {old} to {new} (a new cgroup?)"),
            );
        }
        _ => {}
    }
    (
        "kept",
        format!(
            "{}/{}, cgroup populated, cpu usage {} -> {} usec",
            now.active_state,
            now.sub_state,
            before
                .cpu_usage_usec
                .map_or_else(|| "?".to_owned(), |value| value.to_string()),
            kernel
                .cpu_usage_usec
                .map_or_else(|| "?".to_owned(), |value| value.to_string())
        ),
    )
}

/// The outcome of `--check`.
pub struct CheckReport {
    /// The JSON report (schema `agentctl-services-transients-check/v1`).
    pub document: Value,
    /// The text report.
    pub text: String,
    /// 0: every saved unit was kept or ended; 1: a unit was lost.
    pub exit_code: i32,
}

/// Compare the saved record with what systemd and the kernel say now.
pub fn check(
    host: &dyn Host,
    cgroup_root: &Path,
    saved: &[Transient],
) -> Result<CheckReport, String> {
    let current = collect(host, cgroup_root)?;
    let by_name: BTreeMap<&str, &Transient> = current
        .iter()
        .map(|transient| (transient.unit.as_str(), transient))
        .collect();
    let mut rows = Vec::new();
    let mut text = String::new();
    let mut lost = 0;
    for before in saved {
        let now = by_name.get(before.unit.as_str()).copied();
        // The kernel's view of the cgroup the unit had, read now.
        let mut kernel = before.clone();
        kernel.populated = None;
        kernel.processes = None;
        kernel.cpu_usage_usec = None;
        kernel.memory_current = None;
        read_cgroup(host, cgroup_root, &mut kernel);
        let (verdict, detail) = verdict(before, now, &kernel);
        if verdict == "lost" {
            lost += 1;
        }
        text.push_str(&format!(
            "{}: {} ({}): {detail}\n",
            before.unit,
            verdict.to_uppercase(),
            before.control_group
        ));
        rows.push(json!({
            "unit": before.unit,
            "verdict": verdict,
            "detail": detail,
            "before": before.to_json(),
            "now": now.map(Transient::to_json),
            "cgroup_now": {
                "populated": kernel.populated,
                "processes": kernel.processes,
                "cpu_usage_usec": kernel.cpu_usage_usec,
                "memory_current": kernel.memory_current,
            },
        }));
    }
    let new_units: Vec<&str> = current
        .iter()
        .filter(|transient| !saved.iter().any(|before| before.unit == transient.unit))
        .map(|transient| transient.unit.as_str())
        .collect();
    if !new_units.is_empty() {
        text.push_str(&format!(
            "{} transient unit(s) started since the record: {}\n",
            new_units.len(),
            new_units.join(", ")
        ));
    }
    text.push_str(&if lost == 0 {
        format!(
            "ok: none of the {} recorded transient unit(s) was lost\n",
            saved.len()
        )
    } else {
        format!(
            "LOST: {lost} of {} recorded transient unit(s)\n",
            saved.len()
        )
    });
    let exit_code = i32::from(lost > 0);
    Ok(CheckReport {
        document: json!({
            "schema": "agentctl-services-transients-check/v1",
            "units": rows,
            "new_units": new_units,
            "lost": lost,
            "exit_code": exit_code,
        }),
        text,
        exit_code,
    })
}

/// The text listing for a plain run or `--save`.
#[must_use]
pub fn listing(transients: &[Transient]) -> String {
    let mut text = String::new();
    for transient in transients {
        text.push_str(&format!(
            "{} {}/{} cgroup {} populated {} processes {} cpu_usage_usec {}\n",
            transient.unit,
            transient.active_state,
            transient.sub_state,
            transient.control_group,
            transient
                .populated
                .map_or_else(|| "?".to_owned(), |value| value.to_string()),
            transient
                .processes
                .map_or_else(|| "?".to_owned(), |value| value.to_string()),
            transient
                .cpu_usage_usec
                .map_or_else(|| "?".to_owned(), |value| value.to_string()),
        ));
    }
    text.push_str(&format!("{} transient unit(s)\n", transients.len()));
    text
}

/// Seconds since the epoch for the record.
#[must_use]
pub fn now_seconds(host: &dyn Host) -> f64 {
    host.now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}
