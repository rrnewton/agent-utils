//! Pure sandbox settings and slice arithmetic (P9, mu-rsimg,
//! unwired until dispatch).
//!
//! Mirrors the decision half of Python `sandbox.py`: limit
//! validation, `effective_cpu_quota`, systemd unit-name escaping
//! (natively, matching `systemd-escape`), slice naming from an
//! injected cgroup text, and slice limit properties. Nothing here
//! runs `systemctl`, enters a namespace, or touches a cgroup; the
//! exec half lives in a later writer step.

use serde_json::{json, Value};

pub(crate) const DEFAULT_TASKS_MAX: i64 = 8192;
pub(crate) const ISOLATIONS: [&str; 3] = ["userns", "cgroup", "root"];
pub(crate) const HOME_MODES: [&str; 2] = ["ro", "hidden"];
pub(crate) const COORDINATOR_SCOPES: [&str; 2] = ["worktrees", "project"];
pub(crate) const LIMIT_KEYS: [&str; 8] = [
    "memory_max",
    "memory_high",
    "cpu_quota",
    "tasks_max",
    "io_weight",
    "all_slots_memory_max",
    "all_slots_cpu_quota",
    "slot_cpu_quotas",
];

/// systemd limits for one slot's slice, mirroring Python
/// `SandboxLimits` with its defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SandboxLimits {
    pub(crate) memory_max: Option<String>,
    pub(crate) memory_high: Option<String>,
    pub(crate) cpu_quota: Option<String>,
    pub(crate) tasks_max: Option<i64>,
    pub(crate) io_weight: Option<i64>,
    pub(crate) all_slots_memory_max: Option<String>,
    pub(crate) all_slots_cpu_quota: Option<String>,
    pub(crate) slot_cpu_quotas: Vec<(String, String)>,
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            memory_max: None,
            memory_high: None,
            cpu_quota: None,
            tasks_max: Some(DEFAULT_TASKS_MAX),
            io_weight: None,
            all_slots_memory_max: None,
            all_slots_cpu_quota: None,
            slot_cpu_quotas: Vec::new(),
        }
    }
}

fn limit_value_ok(value: &str) -> bool {
    // Python `_LIMIT_RE`: ^(infinity|\d+(\.\d+)?[KMGT]?|\d+(\.\d+)?%)$
    if value == "infinity" {
        return true;
    }
    let digits = |text: &str| -> bool {
        let mut parts = text.splitn(2, '.');
        let whole = parts.next().unwrap_or_default();
        if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        match parts.next() {
            None => true,
            Some(frac) => !frac.is_empty() && frac.bytes().all(|byte| byte.is_ascii_digit()),
        }
    };
    if let Some(number) = value.strip_suffix('%') {
        return digits(number);
    }
    let number = value.strip_suffix(['K', 'M', 'G', 'T']).unwrap_or(value);
    digits(number)
}

fn optional_str(value: Option<&Value>, label: &str) -> Result<Option<String>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) if limit_value_ok(text) => Ok(Some(text.clone())),
        Some(other) => Err(format!(
            "{label} must look like 32G, 800%, or infinity, not {}",
            match other {
                Value::String(text) => format!("'{text}'"),
                other => other.to_string(),
            }
        )),
    }
}

fn optional_int(value: Option<&Value>, label: &str) -> Result<Option<i64>, String> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => match value.as_i64() {
            Some(number) if number >= 1 && value.is_i64() => Ok(Some(number)),
            _ => Err(format!("{label} must be a positive integer or null")),
        },
    }
}

/// Validate a `configuration.sandbox.limits` mapping, mirroring
/// Python `limits_from_obj` with its error strings.
pub(crate) fn limits_from_obj(raw: &Value) -> Result<SandboxLimits, String> {
    let Some(map) = raw.as_object() else {
        return Err("configuration.sandbox.limits must be an object".to_string());
    };
    let mut unknown: Vec<&String> = map
        .keys()
        .filter(|key| !LIMIT_KEYS.contains(&key.as_str()))
        .collect();
    unknown.sort();
    if !unknown.is_empty() {
        let names: Vec<&str> = unknown.iter().map(|key| key.as_str()).collect();
        return Err(format!(
            "configuration.sandbox.limits has unknown keys: {}",
            names.join(", ")
        ));
    }
    let label = "configuration.sandbox.limits.";
    let quotas_label = "configuration.sandbox.limits.slot_cpu_quotas";
    let mut slot_cpu_quotas = Vec::new();
    if let Some(value) = map.get("slot_cpu_quotas") {
        if !value.is_null() {
            let Some(items) = value.as_array() else {
                return Err(format!(
                    "{quotas_label} must be a list of {{slot_prefix, cpu_quota}} objects"
                ));
            };
            for item in items {
                let Some(entry) = item.as_object() else {
                    return Err(format!(
                        "{quotas_label} entries must be {{slot_prefix, cpu_quota}} objects"
                    ));
                };
                if entry.len() != 2
                    || !entry.contains_key("slot_prefix")
                    || !entry.contains_key("cpu_quota")
                {
                    return Err(format!(
                        "{quotas_label} entries must be {{slot_prefix, cpu_quota}} objects"
                    ));
                }
                let prefix = entry
                    .get("slot_prefix")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if prefix.is_empty() {
                    return Err(format!(
                        "{quotas_label} slot_prefix must be a non-empty string"
                    ));
                }
                let quota =
                    optional_str(entry.get("cpu_quota"), &format!("{quotas_label}.cpu_quota"))?
                        .ok_or_else(|| format!("{quotas_label} cpu_quota must not be null"))?;
                slot_cpu_quotas.push((prefix.to_string(), quota));
            }
        }
    }
    Ok(SandboxLimits {
        memory_max: optional_str(map.get("memory_max"), &format!("{label}memory_max"))?,
        memory_high: optional_str(map.get("memory_high"), &format!("{label}memory_high"))?,
        cpu_quota: optional_str(map.get("cpu_quota"), &format!("{label}cpu_quota"))?,
        tasks_max: match map.get("tasks_max") {
            None => Some(DEFAULT_TASKS_MAX),
            other => optional_int(other, &format!("{label}tasks_max"))?,
        },
        io_weight: optional_int(map.get("io_weight"), &format!("{label}io_weight"))?,
        all_slots_memory_max: optional_str(
            map.get("all_slots_memory_max"),
            &format!("{label}all_slots_memory_max"),
        )?,
        all_slots_cpu_quota: optional_str(
            map.get("all_slots_cpu_quota"),
            &format!("{label}all_slots_cpu_quota"),
        )?,
        slot_cpu_quotas,
    })
}

/// The limits as a JSON object, mirroring the `limits` half of
/// Python `settings_to_obj`.
pub(crate) fn limits_to_obj(limits: &SandboxLimits) -> Value {
    json!({
        "memory_max": limits.memory_max,
        "memory_high": limits.memory_high,
        "cpu_quota": limits.cpu_quota,
        "tasks_max": limits.tasks_max,
        "io_weight": limits.io_weight,
        "all_slots_memory_max": limits.all_slots_memory_max,
        "all_slots_cpu_quota": limits.all_slots_cpu_quota,
        "slot_cpu_quotas": limits.slot_cpu_quotas.iter().map(|(prefix, quota)| {
            json!({"slot_prefix": prefix, "cpu_quota": quota})
        }).collect::<Vec<_>>(),
    })
}

/// First matching prefix override wins, mirroring Python
/// `effective_cpu_quota`.
pub(crate) fn effective_cpu_quota<'a>(
    limits: &'a SandboxLimits,
    slot: Option<&str>,
) -> Option<&'a str> {
    if let Some(slot) = slot {
        for (prefix, quota) in &limits.slot_cpu_quotas {
            if slot.starts_with(prefix.as_str()) {
                return Some(quota);
            }
        }
    }
    limits.cpu_quota.as_deref()
}

/// Escape a string for a systemd unit name, matching the
/// `systemd-escape` binary Python shells out to: `/` becomes `-`,
/// a leading `.` is escaped, and every byte outside
/// `[A-Za-z0-9:._]` (with `-` itself escaped) becomes `\xHH`.
pub(crate) fn systemd_escape(value: &str) -> String {
    let mut out = String::new();
    for (index, byte) in value.bytes().enumerate() {
        match byte {
            b'/' => out.push('-'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b':' | b'_' => {
                out.push(byte as char);
            }
            b'.' if index != 0 => out.push('.'),
            _ => out.push_str(&format!("\\x{byte:02x}")),
        }
    }
    out
}

/// The innermost user-manager slice enclosing the caller, from an
/// injected `/proc/self/cgroup` text, mirroring Python
/// `parent_slice`. `None` text (unreadable file) yields `None`.
pub(crate) fn parent_slice_from_text(text: Option<&str>) -> Option<String> {
    let text = text?;
    for line in text.lines() {
        if !line.starts_with("0::") {
            continue;
        }
        if !line.contains("user@") {
            return None;
        }
        let parts: Vec<&str> = line[3..].split('/').collect();
        let at = parts.iter().position(|part| part.starts_with("user@"))?;
        let slices: Vec<&str> = parts[at + 1..]
            .iter()
            .filter(|part| part.ends_with(".slice"))
            .copied()
            .collect();
        return slices.last().map(|slice| slice.to_string());
    }
    None
}

/// The all-slots slice, mirroring Python `root_slice`. `parent` is
/// the enclosing slice, `Some("")` for none, or `None` text to
/// rediscover from the injected cgroup text.
pub(crate) fn root_slice(parent: Option<&str>, cgroup_text: Option<&str>) -> String {
    let enclosing = match parent {
        Some(parent) => parent.to_string(),
        None => parent_slice_from_text(cgroup_text).unwrap_or_default(),
    };
    if enclosing.is_empty() {
        "wrkslots.slice".to_string()
    } else {
        format!("{}-wrkslots.slice", enclosing.trim_end_matches(".slice"))
    }
}

/// One slot's (or coordinator box's) slice, mirroring Python
/// `slice_name`.
pub(crate) fn slice_name(
    slot_type: &str,
    slot: &str,
    parent: Option<&str>,
    cgroup_text: Option<&str>,
) -> String {
    let root = root_slice(parent, cgroup_text);
    let base = root.trim_end_matches(".slice");
    if slot_type == "box" {
        return format!("{base}-box-{}.slice", systemd_escape(slot));
    }
    let label = if slot_type == "agent" {
        slot.to_string()
    } else {
        format!("{slot_type}:{slot}")
    };
    format!("{base}-{}.slice", systemd_escape(&label))
}

/// systemd properties for the per-slot slice, mirroring Python
/// `slice_limit_properties`.
pub(crate) fn slice_limit_properties(limits: &SandboxLimits, slot: Option<&str>) -> Vec<String> {
    let mut properties = Vec::new();
    if let Some(memory_max) = &limits.memory_max {
        properties.push(format!("MemoryMax={memory_max}"));
        properties.push("MemorySwapMax=0".to_string());
    }
    if let Some(memory_high) = &limits.memory_high {
        properties.push(format!("MemoryHigh={memory_high}"));
    }
    if let Some(cpu_quota) = effective_cpu_quota(limits, slot) {
        properties.push(format!("CPUQuota={cpu_quota}"));
    }
    if let Some(tasks_max) = limits.tasks_max {
        properties.push(format!("TasksMax={tasks_max}"));
    }
    if let Some(io_weight) = limits.io_weight {
        properties.push(format!("IOWeight={io_weight}"));
    }
    properties
}

/// systemd properties for the all-slots slice, mirroring the
/// `root_limits` half of Python `apply_slice_limits`.
pub(crate) fn all_slots_properties(limits: &SandboxLimits) -> Vec<String> {
    let mut properties = Vec::new();
    if let Some(memory_max) = &limits.all_slots_memory_max {
        properties.push(format!("MemoryMax={memory_max}"));
        properties.push("MemorySwapMax=0".to_string());
    }
    if let Some(cpu_quota) = &limits.all_slots_cpu_quota {
        properties.push(format!("CPUQuota={cpu_quota}"));
    }
    properties
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_matches_systemd_escape() {
        // Observed from /usr/bin/systemd-escape on this host.
        assert_eq!(systemd_escape("kvm-foo"), "kvm\\x2dfoo");
        assert_eq!(systemd_escape("agent:slot-a"), "agent:slot\\x2da");
        assert_eq!(systemd_escape("a/b"), "a-b");
        assert_eq!(systemd_escape(".hidden"), "\\x2ehidden");
        assert_eq!(systemd_escape("a_b"), "a_b");
        assert_eq!(systemd_escape("a@b"), "a\\x40b");
        assert_eq!(systemd_escape("a+b"), "a\\x2bb");
        assert_eq!(systemd_escape("a b"), "a\\x20b");
    }

    #[test]
    fn slices_and_quotas() {
        assert_eq!(root_slice(Some(""), None), "wrkslots.slice");
        assert_eq!(
            root_slice(Some("user-1000.slice"), None),
            "user-1000-wrkslots.slice"
        );
        assert_eq!(
            slice_name("agent", "kvm-foo", Some(""), None),
            "wrkslots-kvm\\x2dfoo.slice"
        );
        assert_eq!(
            slice_name("box", "coordinator", Some(""), None),
            "wrkslots-box-coordinator.slice"
        );
        assert_eq!(
            slice_name("validate", "slot-a", Some(""), None),
            "wrkslots-validate:slot\\x2da.slice"
        );
        let cgroup = "0::/user.slice/user@1000.service/session.slice\n";
        assert_eq!(
            parent_slice_from_text(Some(cgroup)).as_deref(),
            Some("session.slice")
        );
        assert_eq!(parent_slice_from_text(None), None);

        let limits = limits_from_obj(&json!({
            "cpu_quota": "800%",
            "slot_cpu_quotas": [{"slot_prefix": "wide", "cpu_quota": "3200%"}],
            "memory_max": "32G"
        }))
        .expect("limits");
        assert_eq!(effective_cpu_quota(&limits, Some("wide-1")), Some("3200%"));
        assert_eq!(effective_cpu_quota(&limits, Some("narrow")), Some("800%"));
        assert_eq!(
            slice_limit_properties(&limits, Some("wide-1")),
            [
                "MemoryMax=32G",
                "MemorySwapMax=0",
                "CPUQuota=3200%",
                "TasksMax=8192"
            ]
        );
        assert!(limits_from_obj(&json!({"bogus": 1})).is_err());
        assert!(limits_from_obj(&json!({"memory_max": "lots"})).is_err());
    }
}
